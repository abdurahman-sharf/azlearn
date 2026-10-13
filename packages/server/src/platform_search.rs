//! Unified search (`GET /api/platform/search`, phase 4-2).
//!
//! Six groups — subjects, teachers, courses, posts, live sessions and (students only) exams — each searched with the Arabic
//! folding the answer checker already uses ([`crate::platform::ar_fold`], the SQL function of the same name): أ/إ/آ = ا,
//! ى = ي, diacritics and tatweel ignored, case-insensitive; `%`, `_` and `\` in the query are literal. **Visibility is the
//! one rule of the platform** ([`crate::platform_content::visible`], [`exam_standing`]): a draft, a hidden, an unapproved or
//! a cancelled item is never found, never counted and never quoted in a snippet.
//!
//! A hit carries `matched` (`title` when the title / name matched, otherwise `text`) and `snippet` (up to 140 characters
//! around the first match in the item's text — a description, the first [`BODY_SEARCH_CHARS`] characters of a post body, a
//! teacher's bio — or `null`). Cost is bounded: each group is ONE scan (the total comes from `count(*) OVER ()` in the same
//! query as the page), the post body is searched only up to [`BODY_SEARCH_CHARS`], and a user may search
//! [`SEARCHES_PER_MINUTE`] times a minute (429 `rate_limited`). Without `group`
//! every group returns its best 8; with `group=<one of the six>` only that group's array is filled, paged by
//! `limit` (default 20, at most 50) and `offset`. `counts` always holds the total matches of every group.

use crate::platform::{ar_fold, bad, db_err, fold_like, lock, rate_limit, require_active, Res, User, INSTITUTION_ACTIVE, SUBJECT_ACTIVE};
use crate::platform_content::{exam_standing, visible};
use crate::relay::now_ms;
use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use rusqlite::{params_from_iter, types::Value, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The group names, in the order the response lists them.
pub const GROUPS: [&str; 6] = ["subjects", "teachers", "courses", "posts", "live", "exams"];

/// Query length bounds, in characters (after trimming).
const MIN_QUERY_CHARS: usize = 2;
const MAX_QUERY_CHARS: usize = 60;
/// Hits per group without `group`.
const OVERVIEW_PER_GROUP: i64 = 8;
const GROUP_DEFAULT_LIMIT: i64 = 20;
const GROUP_MAX_LIMIT: i64 = 50;
/// A snippet is at most this many characters, ellipses included.
pub const SNIPPET_MAX_CHARS: usize = 140;
/// How many characters of a post's body are searched (and quoted). The fold runs once per row for EVERY visible post on
/// every search, under the one database lock, and a body can be 20 000 characters: the first 2 000 (the length of every
/// other searched text) keep one search to a small, bounded amount of work however long the articles are.
pub const BODY_SEARCH_CHARS: usize = 2000;
/// Searches per user per minute (fixed window). A person typing with the 300 ms pause of the search page needs a handful;
/// the cap keeps a loop of requests from holding the database lock back-to-back.
pub const SEARCHES_PER_MINUTE: i64 = 40;

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub link: String,
    /// Up to 140 characters around the first match in the item's text, or `null` when the text does not contain the query.
    pub snippet: Option<String>,
    /// `title` when the title / name matched, otherwise `text`.
    pub matched: &'static str,
}

#[derive(Serialize, Debug, PartialEq, Default, Clone)]
pub struct SearchCounts {
    pub subjects: i64,
    pub teachers: i64,
    pub courses: i64,
    pub posts: i64,
    pub live: i64,
    pub exams: i64,
}

#[derive(Serialize, Debug, Default)]
pub struct SearchResults {
    pub subjects: Vec<Hit>,
    pub teachers: Vec<Hit>,
    pub courses: Vec<Hit>,
    pub posts: Vec<Hit>,
    pub live: Vec<Hit>,
    pub exams: Vec<Hit>,
    pub counts: SearchCounts,
}

/// `GET /search` query. Blank values count as absent.
#[derive(Deserialize, Default, Debug)]
pub struct SearchQuery {
    pub q: Option<String>,
    /// One of [`GROUPS`]: only that group's array is filled.
    pub group: Option<String>,
    /// With `group` only: default 20, at most 50.
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ───────── snippets ─────────

/// The text `text` folded the way [`ar_fold`] folds it, one folded character at a time (the same per-character folding,
/// [`crate::platform_exams::fold_char`], then lowercase), each remembering the index (in characters) of the original
/// character it came from — so a match found in the folded form can be cut out of the original. Whitespace runs are one
/// space and leading whitespace is dropped, as in [`ar_fold`]. (Lowercasing a whole string is context-sensitive for
/// exactly one letter, the Greek final sigma; character by character it is not, so for such a word the snippet can be
/// missing — the hit itself is unaffected.)
fn folded_with_origin(text: &str) -> Vec<(char, usize)> {
    let mut out: Vec<(char, usize)> = Vec::with_capacity(text.len() / 2);
    for (i, c) in text.chars().enumerate() {
        if c.is_whitespace() {
            if out.last().map_or(false, |(p, _)| *p != ' ') {
                out.push((' ', i));
            }
        } else if let Some(f) = crate::platform_exams::fold_char(c) {
            out.extend(f.to_lowercase().map(|l| (l, i)));
        }
    }
    out
}

/// Up to [`SNIPPET_MAX_CHARS`] characters of `text` around the first occurrence of `needle` (an already folded, non-empty
/// query), whitespace collapsed to single spaces, with `…` where the text goes on. `None` when `text` does not contain it.
pub fn snippet(text: &str, needle: &str) -> Option<String> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return None;
    }
    let folded = folded_with_origin(text);
    if folded.len() < needle.len() {
        return None;
    }
    let at = (0..=folded.len() - needle.len()).find(|&j| folded[j..j + needle.len()].iter().map(|(c, _)| *c).eq(needle.iter().copied()))?;
    let chars: Vec<char> = text.chars().collect();
    let (m_start, m_end) = (folded[at].1, folded[at + needle.len() - 1].1 + 1);
    // two characters are kept for the ellipses; a third of the room goes before the match
    let room = SNIPPET_MAX_CHARS - 2;
    let m_len = m_end - m_start;
    let before = if m_len >= room { 0 } else { ((room - m_len) / 3).min(m_start) };
    let start = m_start - before;
    let end = (start + room).min(chars.len());
    let mut body = String::new();
    let mut space = true; // collapses runs and trims the left edge
    for c in &chars[start..end] {
        if c.is_whitespace() || c.is_control() {
            if !space {
                body.push(' ');
            }
            space = true;
        } else {
            body.push(*c);
            space = false;
        }
    }
    let body = body.trim_end().to_string();
    if body.is_empty() {
        return None;
    }
    Some(format!("{}{}{}", if start > 0 { "…" } else { "" }, body, if end < chars.len() { "…" } else { "" }))
}

// ───────── the groups ─────────

/// One searchable group: which rows are shown to the viewer at all (`from_where`, visibility included) and which fields are
/// searched. SQL fragments are literals of this module (never request text); the search text only ever travels as a bound
/// parameter (`?1`, a [`fold_like`] pattern).
struct Spec {
    /// `FROM … WHERE <visibility>`; `?2` is the viewer's id when `needs_viewer`.
    from_where: String,
    /// Columns: id, title, subtitle, link tail (a column, usually the id), the searchable text (or `NULL`).
    cols: String,
    title_cond: String,
    text_cond: Option<String>,
    order: &'static str,
    needs_viewer: bool,
    /// Link of a hit from its id and link-tail column.
    link: fn(&str, &str, bool) -> String,
}

fn folded_like_col(col: &str) -> String {
    format!("ar_fold(COALESCE({col}, '')) LIKE ?1 ESCAPE '\\'")
}

fn spec(group: &str, viewer: &User, now: i64) -> Option<Spec> {
    Some(match group {
        "subjects" => Spec {
            from_where: format!("FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE {SUBJECT_ACTIVE} AND {INSTITUTION_ACTIVE}"),
            cols: "s.id, s.name_ar, i.name_ar, s.id, NULL".to_string(),
            title_cond: format!("({} OR {})", folded_like_col("s.name_ar"), folded_like_col("s.name_en")),
            text_cond: None,
            order: "s.name_ar, s.id",
            needs_viewer: false,
            link: |id, _, _| format!("/platform/subjects/{id}"),
        },
        "teachers" => Spec {
            from_where: "FROM users u WHERE u.role = 'teacher' AND u.status = 'active'".to_string(),
            cols: "u.id, u.full_name, COALESCE(substr(u.bio, 1, 80), ''), u.id, u.bio".to_string(),
            title_cond: folded_like_col("u.full_name"),
            text_cond: Some(folded_like_col("u.bio")),
            order: "u.full_name, u.id",
            needs_viewer: false,
            link: |id, _, _| format!("/platform/teachers/{id}"),
        },
        "courses" => Spec {
            from_where: format!("FROM courses c JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id WHERE {}", visible("c", "published")),
            cols: "c.id, c.title, s.name_ar, c.id, c.description".to_string(),
            title_cond: folded_like_col("c.title"),
            text_cond: Some(folded_like_col("c.description")),
            order: "c.updated_at DESC, c.id",
            needs_viewer: false,
            link: |id, _, _| format!("/platform/courses/{id}"),
        },
        "posts" => Spec {
            from_where: format!("FROM posts p JOIN users u ON u.id = p.teacher_id JOIN subjects s ON s.id = p.subject_id WHERE {}", visible("p", "published")),
            // only the start of the body is folded, compared and quoted (see BODY_SEARCH_CHARS)
            cols: format!("p.id, p.title, s.name_ar, p.id, substr(p.body, 1, {BODY_SEARCH_CHARS})"),
            title_cond: folded_like_col("p.title"),
            text_cond: Some(folded_like_col(&format!("substr(p.body, 1, {BODY_SEARCH_CHARS})"))),
            order: "p.updated_at DESC, p.id",
            needs_viewer: false,
            link: |id, _, _| format!("/platform/posts/{id}"),
        },
        "live" => Spec {
            // scheduled, shown to students, and not over yet. (`now` is a number, not request text.)
            from_where: format!(
                "FROM live_sessions l JOIN users u ON u.id = l.teacher_id JOIN subjects s ON s.id = l.subject_id
                 WHERE {} AND l.starts_at + l.duration_min * 60000 > {now}",
                visible("l", "scheduled")
            ),
            // the second link column is the subject: only a student has a live page (`/platform/live/:id`); a teacher or an
            // admin lands on the subject's page, which lists its sessions
            cols: "l.id, l.title, s.name_ar, l.subject_id, l.description".to_string(),
            title_cond: folded_like_col("l.title"),
            text_cond: Some(folded_like_col("l.description")),
            order: "l.starts_at, l.id",
            needs_viewer: false,
            link: |id, subject, student| if student { format!("/platform/live/{id}") } else { format!("/platform/subjects/{subject}") },
        },
        "exams" if viewer.role == "student" => Spec {
            from_where: format!(
                "FROM assessments a LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
                 WHERE a.status = 'published' AND {}
                   AND EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?2 AND e.subject_id = a.subject_id)",
                exam_standing("a")
            ),
            cols: "a.id, a.title, s.name_ar, a.id, NULL".to_string(),
            title_cond: folded_like_col("a.title"),
            text_cond: None,
            order: "a.updated_at DESC, a.id",
            needs_viewer: true,
            link: |id, _, _| format!("/platform/assessments/{id}"),
        },
        _ => return None,
    })
}

impl Spec {
    fn matches(&self) -> String {
        match &self.text_cond {
            Some(t) => format!("({} OR {t})", self.title_cond),
            None => self.title_cond.clone(),
        }
    }

    fn args(&self, pat: &str, viewer: &User) -> Vec<Value> {
        let mut a: Vec<Value> = vec![pat.to_string().into()];
        if self.needs_viewer {
            a.push(viewer.id.clone().into());
        }
        a
    }

    /// Only the number of matches (one scan; no rows are built).
    fn count(&self, conn: &Connection, pat: &str, viewer: &User) -> Res<i64> {
        let sql = format!("SELECT count(*) {} AND {}", self.from_where, self.matches());
        conn.query_row(&sql, params_from_iter(self.args(pat, viewer)), |r| r.get(0)).map_err(db_err)
    }

    /// One page of hits (title matches before text-only matches, then the group's own order) AND the total number of
    /// matches, from ONE scan: the total is `count(*) OVER ()`, computed over the whole match set before `LIMIT` cuts it.
    /// A page past the end has no row to carry the total, so (only then) it is counted separately.
    fn page(&self, conn: &Connection, pat: &str, needle: &str, viewer: &User, limit: i64, offset: i64) -> Res<(i64, Vec<Hit>)> {
        let mut args = self.args(pat, viewer);
        let n = args.len();
        args.push(limit.into());
        args.push(offset.into());
        let sql = format!(
            "SELECT {cols}, {title} AS in_title, count(*) OVER () AS total {from} AND {matches} ORDER BY in_title DESC, {order} LIMIT ?{l} OFFSET ?{o}",
            cols = self.cols,
            title = self.title_cond,
            from = self.from_where,
            matches = self.matches(),
            order = self.order,
            l = n + 1,
            o = n + 2,
        );
        let student = viewer.role == "student";
        let rows = conn
            .prepare(&sql)
            .map_err(db_err)?
            .query_map(params_from_iter(args), |r| {
                let (id, title, subtitle, link_tail, text, in_title, total): (String, String, String, String, Option<String>, bool, i64) =
                    (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?);
                Ok((id, title, subtitle, link_tail, text, in_title, total))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let total = match rows.first() {
            Some(row) => row.6,
            None if offset == 0 => 0,
            None => self.count(conn, pat, viewer)?,
        };
        let hits = rows
            .into_iter()
            .map(|(id, title, subtitle, link_tail, text, in_title, _)| Hit {
                link: (self.link)(&id, &link_tail, student),
                snippet: text.as_deref().and_then(|t| snippet(t, needle)),
                matched: if in_title { "title" } else { "text" },
                id,
                title,
                subtitle,
            })
            .collect();
        Ok((total, hits))
    }
}

fn clean(o: &Option<String>) -> Option<&str> {
    o.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// The search. 400 `invalid_query` unless the trimmed query is 2 to 60 characters, 400 `invalid_group` for a group that is
/// not one of [`GROUPS`]. A query that folds to nothing (only diacritics or tatweel) finds nothing, rather than everything.
pub(crate) fn search(conn: &Connection, viewer: &User, q: &SearchQuery, now: i64) -> Res<SearchResults> {
    let raw = clean(&q.q).unwrap_or("");
    let chars = raw.chars().count();
    if !(MIN_QUERY_CHARS..=MAX_QUERY_CHARS).contains(&chars) {
        return Err(bad("invalid_query"));
    }
    let group = match clean(&q.group) {
        None => None,
        Some(g) => Some(GROUPS.iter().copied().find(|x| *x == g).ok_or_else(|| bad("invalid_group"))?),
    };
    let needle = ar_fold(raw);
    let mut out = SearchResults::default();
    if needle.is_empty() {
        return Ok(out);
    }
    let pat = fold_like(raw);
    let (limit, offset) = match group {
        None => (OVERVIEW_PER_GROUP, 0),
        Some(_) => (q.limit.unwrap_or(GROUP_DEFAULT_LIMIT).clamp(1, GROUP_MAX_LIMIT), q.offset.unwrap_or(0).max(0)),
    };
    for name in GROUPS {
        let Some(spec) = spec(name, viewer, now) else { continue }; // exams for a non-student: nothing, count 0
        // one scan per group: the page and the total together for the group(s) that return hits, the total alone for the rest
        let (total, hits) = if group.map_or(true, |g| g == name) { spec.page(conn, &pat, &needle, viewer, limit, offset)? } else { (spec.count(conn, &pat, viewer)?, Vec::new()) };
        match name {
            "subjects" => (out.counts.subjects, out.subjects) = (total, hits),
            "teachers" => (out.counts.teachers, out.teachers) = (total, hits),
            "courses" => (out.counts.courses, out.courses) = (total, hits),
            "posts" => (out.counts.posts, out.posts) = (total, hits),
            "live" => (out.counts.live, out.live) = (total, hits),
            _ => (out.counts.exams, out.exams) = (total, hits),
        }
    }
    Ok(out)
}

/// Takes one of the viewer's [`SEARCHES_PER_MINUTE`] places; 429 `rate_limited` when they are used up.
pub(crate) fn throttle(conn: &Connection, viewer: &User) -> Res<()> {
    rate_limit(conn, &format!("search:{}", viewer.id), 60_000, SEARCHES_PER_MINUTE)
}

pub async fn search_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<SearchQuery>) -> Res<Json<SearchResults>> {
    let viewer = require_active(&s, &h)?;
    let conn = lock(&s)?;
    throttle(&conn, &viewer)?;
    search(&conn, &viewer, &q, now_ms()).map(Json)
}

/// `GET /api/platform/search` (merged into the app in `main.rs`).
pub fn routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/api/platform/search", axum::routing::get(search_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use rusqlite::params;

    const NOW: i64 = 1_800_000_000_000;
    const H: i64 = 3_600_000;

    struct W {
        conn: Connection,
        teacher: User,
        admin: User,
        /// enrolled in s1
        me: User,
        /// enrolled nowhere
        out: User,
    }

    fn world() -> W {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة صنعاء',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,name_en,is_active,created_at) VALUES ('s1','i1','برمجة الحاسوب','Computer Programming',1,0), ('s2','i1','الإحصاء التطبيقي','Applied Statistics',1,0);",
        )
        .unwrap();
        let user = |email: &str, role: &str, status: &str, name: &str| {
            let u = insert_test_user(&conn, email, role, status);
            conn.execute("UPDATE users SET full_name = ?2 WHERE id = ?1", params![u.id, name]).unwrap();
            User { full_name: name.into(), ..u }
        };
        let teacher = user("t@x.com", "teacher", "active", "د. أحمد الإبراهيمي");
        let admin = user("a@x.com", "admin", "active", "المدير");
        let me = user("me@x.com", "student", "active", "طالب");
        let out = user("out@x.com", "student", "active", "زائر");
        conn.execute("UPDATE users SET bio = 'متخصص في الإحصاء والاحتمالات منذ عشرين عاما' WHERE id = ?1", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at,decided_at) VALUES (?1,'s1','approved',0,0), (?1,'s2','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,'s1',0)", params![me.id]).unwrap();
        W { conn, teacher, admin, me, out }
    }

    fn post(w: &W, id: &str, title: &str, body: &str, status: &str, updated: i64) {
        w.conn
            .execute("INSERT INTO posts(id,teacher_id,subject_id,kind,title,body,status,created_at,updated_at) VALUES (?1,?2,'s1','article',?3,?4,?5,0,?6)", params![id, w.teacher.id, title, body, status, updated])
            .unwrap();
    }

    fn course(w: &W, id: &str, title: &str, description: Option<&str>, status: &str, updated: i64) {
        w.conn
            .execute("INSERT INTO courses(id,teacher_id,subject_id,title,description,status,created_at,updated_at) VALUES (?1,?2,'s1',?3,?4,?5,0,?6)", params![id, w.teacher.id, title, description, status, updated])
            .unwrap();
    }

    fn live(w: &W, id: &str, title: &str, description: Option<&str>, starts: i64, status: &str) {
        w.conn
            .execute(
                "INSERT INTO live_sessions(id,teacher_id,subject_id,title,description,starts_at,duration_min,join_url,status,created_at) VALUES (?1,?2,'s1',?3,?4,?5,60,'https://secret.example/join',?6,0)",
                params![id, w.teacher.id, title, description, starts, status],
            )
            .unwrap();
    }

    fn exam(w: &W, id: &str, subject: &str, title: &str, status: &str, updated: i64) {
        w.conn
            .execute(
                "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,status,created_at,updated_at) VALUES (?1,?2,?2,?3,?4,'[]',1,1,?5,0,?6)",
                params![id, w.teacher.id, subject, title, status, updated],
            )
            .unwrap();
    }

    fn q(text: &str) -> SearchQuery {
        SearchQuery { q: Some(text.into()), ..Default::default() }
    }

    fn find(w: &W, who: &User, text: &str) -> SearchResults {
        search(&w.conn, who, &q(text), NOW).unwrap()
    }

    fn ids(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.id.as_str()).collect()
    }

    fn code<T>(r: Res<T>) -> (axum::http::StatusCode, String) {
        let e = r.err().expect("an error");
        (e.0, serde_json::from_str::<serde_json::Value>(&e.1).unwrap()["error"].as_str().unwrap().to_string())
    }

    // ───────── folding ─────────

    #[test]
    fn arabic_spellings_find_each_other() {
        let w = world();
        course(&w, "c1", "مقدمة في الإحصاء", None, "published", 1);
        course(&w, "c2", "مُقَدِّمَة في الرِّيَاضِيَّات", None, "published", 1);
        course(&w, "c3", "الإحـــصاء بالتطويل", None, "published", 1);
        course(&w, "c4", "مصطفى وأخوه", None, "published", 1);
        course(&w, "c5", "أبحاث آمنة", None, "published", 1);
        let found = |text: &str| -> Vec<String> {
            let mut v: Vec<String> = find(&w, &w.me, text).courses.into_iter().map(|h| h.id).collect();
            v.sort();
            v
        };
        // hamza forms: أ إ آ ا are one letter
        assert_eq!(found("احصاء"), ["c1", "c3"], "the titles have إ (and a tatweel in c3); the query has a plain alef");
        assert_eq!(found("إحصاء"), ["c1", "c3"]);
        assert_eq!(found("ابحاث"), ["c5"]);
        assert_eq!(found("أبحاث"), ["c5"]);
        assert_eq!(found("امنة"), ["c5"], "آ = ا");
        // the diacritics of the text, or of the query, are ignored
        assert_eq!(found("مقدمة"), ["c1", "c2"], "c2 carries tashkeel, the query does not");
        assert_eq!(found("مُقَدِّمَةٌ"), ["c1", "c2"], "and the other way round");
        assert_eq!(found("الرياضيات"), ["c2"]);
        // tatweel in the text
        assert_eq!(found("الاحصاء"), ["c1", "c3"], "c3 spells it with a tatweel");
        assert_eq!(found("الإحصاء"), ["c1", "c3"]);
        // ى = ي
        assert_eq!(found("مصطفي"), ["c4"]);
        assert_eq!(found("مصطفى"), ["c4"]);
        // ة and ه are deliberately NOT unified (as in grading)
        assert!(found("مقدمه").is_empty());
        // Latin text is case-insensitive
        assert_eq!(find(&w, &w.me, "COMPUTER programming").subjects.len(), 1);
        assert_eq!(find(&w, &w.me, "applied STAT").subjects.len(), 1);
    }

    #[test]
    fn like_wildcards_in_the_query_are_literal() {
        let w = world();
        course(&w, "c1", "تحليل_100% كامل", None, "published", 1);
        course(&w, "c2", "تحليل 1000 ثم", None, "published", 2);
        course(&w, "c3", "back\\slash", None, "published", 3);
        assert_eq!(ids(&find(&w, &w.me, "100%").courses), ["c1"], "% is a percent sign, not 'anything'");
        assert_eq!(ids(&find(&w, &w.me, "_100").courses), ["c1"], "_ is an underscore, not 'any letter'");
        assert!(find(&w, &w.me, "%%").courses.is_empty());
        assert!(find(&w, &w.me, "__").courses.is_empty());
        assert_eq!(ids(&find(&w, &w.me, "k\\s").courses), ["c3"], "a backslash is a backslash");
        assert_eq!(find(&w, &w.me, "%%").counts, SearchCounts::default());
    }

    #[test]
    fn the_query_is_validated_and_trimmed() {
        let w = world();
        let bad = |text: Option<&str>| code(search(&w.conn, &w.me, &SearchQuery { q: text.map(String::from), ..Default::default() }, NOW));
        assert_eq!(bad(None), (axum::http::StatusCode::BAD_REQUEST, "invalid_query".to_string()));
        assert_eq!(bad(Some("")).1, "invalid_query");
        assert_eq!(bad(Some("   ")).1, "invalid_query");
        assert_eq!(bad(Some("a")).1, "invalid_query");
        assert_eq!(bad(Some("  a  ")).1, "invalid_query", "trimmed first");
        assert_eq!(bad(Some(&"ب".repeat(61))).1, "invalid_query");
        assert!(search(&w.conn, &w.me, &q("ab"), NOW).is_ok());
        assert!(search(&w.conn, &w.me, &q(&"ب".repeat(60)), NOW).is_ok(), "60 characters (not bytes) is the limit");
        assert!(search(&w.conn, &w.me, &q("  ab  "), NOW).is_ok());
        // a query that folds to nothing (only diacritics / tatweel) finds nothing instead of everything
        course(&w, "c1", "دورة", None, "published", 1);
        let r = find(&w, &w.me, "ـــ");
        assert_eq!((r.courses.len(), r.counts), (0, SearchCounts::default()));
        let r = find(&w, &w.me, "\u{64E}\u{64F}");
        assert_eq!(r.counts, SearchCounts::default());
    }

    // ───────── order ─────────

    #[test]
    fn title_matches_come_before_text_only_matches_then_the_newest_then_the_id() {
        let w = world();
        // text-only match, the newest of all
        course(&w, "text-new", "دورة بايثون", Some("نتعلم الإحصاء من الصفر"), "published", 900);
        // title matches: newer first, equal times by id
        course(&w, "t-old", "الإحصاء 1", None, "published", 100);
        course(&w, "t-mid-b", "الإحصاء 2", None, "published", 500);
        course(&w, "t-mid-a", "الإحصاء 3", None, "published", 500);
        course(&w, "text-old", "دورة الجبر", Some("تمهيد في الاحصاء"), "published", 50);
        let r = find(&w, &w.me, "احصاء");
        assert_eq!(ids(&r.courses), ["t-mid-a", "t-mid-b", "t-old", "text-new", "text-old"]);
        assert_eq!(r.courses.iter().map(|h| h.matched).collect::<Vec<_>>(), ["title", "title", "title", "text", "text"]);
        assert_eq!(r.counts.courses, 5);
    }

    // ───────── snippets ─────────

    #[test]
    fn a_snippet_is_cut_from_the_original_text_around_the_first_match() {
        let w = world();
        let long = format!("{} الإِحْصَاءِ التطبيقي {}", "كلمة ".repeat(60), "وبعدها ".repeat(60));
        post(&w, "p1", "عنوان لا علاقة له", &long, "published", 1);
        let r = find(&w, &w.me, "احصاء");
        let hit = &r.posts[0];
        assert_eq!((hit.id.as_str(), hit.matched), ("p1", "text"));
        let snip = hit.snippet.as_deref().unwrap();
        assert!(snip.chars().count() <= SNIPPET_MAX_CHARS, "{} chars", snip.chars().count());
        assert!(snip.contains("الإِحْصَاءِ"), "the ORIGINAL spelling, diacritics and all: {snip}");
        assert!(snip.starts_with('…') && snip.ends_with('…'), "the text goes on on both sides: {snip}");
        assert_eq!(hit.title, "عنوان لا علاقة له");
        assert_eq!((hit.link.as_str(), hit.subtitle.as_str()), ("/platform/posts/p1", "برمجة الحاسوب"));
    }

    #[test]
    fn snippets_of_short_texts_edges_and_odd_whitespace() {
        // short: the whole text, no ellipsis
        assert_eq!(snippet("درس في الإحصاء", "احصاء").as_deref(), Some("درس في الإحصاء"));
        // at the very start / very end: no ellipsis on that side
        let tail = format!("إحصاء {}", "س ".repeat(200));
        let s = snippet(&tail, "احصاء").unwrap();
        assert!(s.starts_with("إحصاء") && s.ends_with('…') && s.chars().count() <= SNIPPET_MAX_CHARS, "{s}");
        let head = format!("{} إحصاء", "س ".repeat(200));
        let s = snippet(&head, "احصاء").unwrap();
        assert!(s.starts_with('…') && s.ends_with("إحصاء") && s.chars().count() <= SNIPPET_MAX_CHARS, "{s}");
        // newlines, tabs and runs of spaces become single spaces; control characters never survive
        assert_eq!(snippet("سطر\n\n  أول\t\u{7}إحصاء\r\nثاني", "احصاء").as_deref(), Some("سطر أول إحصاء ثاني"));
        // a query longer than the room still yields a bounded snippet
        let needle = "ب".repeat(60);
        let s = snippet(&format!("x{}y", "ب".repeat(300)), &needle).unwrap();
        assert!(s.chars().count() <= SNIPPET_MAX_CHARS, "{}", s.chars().count());
        // absent / empty needle
        assert_eq!(snippet("نص", "غائب"), None);
        assert_eq!(snippet("نص", ""), None);
        assert_eq!(snippet("", "نص"), None);
        // multi-word needles match across the collapsed whitespace
        assert_eq!(snippet("قبل  الإحصاء\n التطبيقي بعد", "احصاء التطبيقي").as_deref(), Some("قبل الإحصاء التطبيقي بعد"));
        // characters outside the BMP do not break the window arithmetic
        let emoji = format!("{} 🙂 إحصاء 🙂 {}", "ا".repeat(200), "ب".repeat(200));
        let s = snippet(&emoji, "احصاء").unwrap();
        assert!(s.contains("إحصاء") && s.chars().count() <= SNIPPET_MAX_CHARS, "{s}");
    }

    #[test]
    fn a_title_only_match_has_no_snippet_and_a_title_plus_text_match_quotes_the_text() {
        let w = world();
        course(&w, "only-title", "الإحصاء", Some("وصف لا يتعلق بشيء"), "published", 1);
        course(&w, "both", "مدخل إلى الإحصاء", Some("يغطي الاحصاء الوصفي"), "published", 2);
        course(&w, "no-description", "إحصاء بلا وصف", None, "published", 3);
        let r = find(&w, &w.me, "احصاء");
        let by = |id: &str| r.courses.iter().find(|h| h.id == id).unwrap();
        assert_eq!((by("only-title").matched, by("only-title").snippet.clone()), ("title", None));
        assert_eq!((by("no-description").matched, by("no-description").snippet.clone()), ("title", None));
        assert_eq!((by("both").matched, by("both").snippet.as_deref()), ("title", Some("يغطي الاحصاء الوصفي")));
    }

    // ───────── what each group searches ─────────

    #[test]
    fn every_group_is_searched_in_the_fields_the_contract_names() {
        let w = world();
        course(&w, "c1", "مقدمة الإحصاء", Some("وصف"), "published", 1);
        post(&w, "p1", "ملخص", "تفصيل عن الإحصاء الوصفي", "published", 1);
        live(&w, "l1", "مراجعة الإحصاء", Some("بث"), NOW + 5 * H, "scheduled");
        exam(&w, "e1", "s1", "امتحان الإحصاء", "published", 1);
        w.conn.execute("UPDATE subjects SET name_en = 'Statistics lab' WHERE id = 's1'", []).unwrap();
        let r = find(&w, &w.me, "احصاء");
        assert_eq!(
            (ids(&r.subjects), ids(&r.teachers), ids(&r.courses), ids(&r.posts), ids(&r.live), ids(&r.exams)),
            (vec!["s2"], vec![w.teacher.id.as_str()], vec!["c1"], vec!["p1"], vec!["l1"], vec!["e1"])
        );
        assert_eq!(r.counts, SearchCounts { subjects: 1, teachers: 1, courses: 1, posts: 1, live: 1, exams: 1 });
        // the teacher matched through the bio only: a text hit with a snippet
        let t = &r.teachers[0];
        assert_eq!(t.matched, "text");
        assert!(t.snippet.as_deref().unwrap().contains("الإحصاء"));
        assert_eq!((t.link.clone(), t.title.as_str()), (format!("/platform/teachers/{}", w.teacher.id), "د. أحمد الإبراهيمي"));
        // the post matched through the body, the live session and the exam through the title
        assert_eq!((r.posts[0].matched, r.live[0].matched, r.exams[0].matched), ("text", "title", "title"));
        assert_eq!(r.exams[0].snippet, None);
        // the subject's English name, the teacher's name
        assert_eq!(ids(&find(&w, &w.me, "statistics LAB").subjects), ["s1"]);
        assert_eq!(ids(&find(&w, &w.me, "احمد").teachers), [w.teacher.id.as_str()], "أحمد is found without the hamza");
        // links
        assert_eq!((r.courses[0].link.as_str(), r.live[0].link.as_str(), r.exams[0].link.as_str(), r.subjects[0].link.as_str()), ("/platform/courses/c1", "/platform/live/l1", "/platform/assessments/e1", "/platform/subjects/s2"));
    }

    #[test]
    fn a_live_hit_leads_a_student_to_the_page_and_the_staff_to_the_subject() {
        let w = world();
        live(&w, "l1", "مراجعة", None, NOW + H, "scheduled");
        assert_eq!(find(&w, &w.me, "مراجعة").live[0].link, "/platform/live/l1");
        for who in [&w.teacher, &w.admin] {
            assert_eq!(find(&w, who, "مراجعة").live[0].link, "/platform/subjects/s1", "{}", who.role);
        }
    }

    // ───────── visibility ─────────

    #[test]
    fn nothing_hidden_is_found_counted_or_quoted() {
        let w = world();
        course(&w, "c-draft", "سري مسودة", Some("وصف سري"), "draft", 1);
        post(&w, "p-draft", "سري مسودة", "نص سري", "draft", 1);
        live(&w, "l-cancelled", "سري ملغي", Some("وصف سري"), NOW + H, "cancelled");
        live(&w, "l-ended", "سري منتهٍ", Some("وصف سري"), NOW - 5 * H, "scheduled");
        exam(&w, "e-draft", "s1", "سري امتحان", "draft", 1);
        exam(&w, "e-closed", "s1", "سري مغلق", "closed", 1);
        exam(&w, "e-other-subject", "s2", "سري آخر", "published", 1);
        let nothing = |w: &W, who: &User, text: &str| {
            let r = find(w, who, text);
            let all = [&r.subjects, &r.teachers, &r.courses, &r.posts, &r.live, &r.exams];
            assert!(all.iter().all(|g| g.is_empty()), "{text}: {r:?}");
            assert_eq!(r.counts, SearchCounts::default(), "{text}: counted");
        };
        for who in [&w.me, &w.out, &w.teacher, &w.admin] {
            nothing(&w, who, "سري");
        }
        // visible items become findable the moment they are published / scheduled
        w.conn.execute("UPDATE courses SET status = 'published'", []).unwrap();
        w.conn.execute("UPDATE posts SET status = 'published'", []).unwrap();
        let r = find(&w, &w.me, "سري");
        assert_eq!((r.courses.len(), r.posts.len(), r.live.len(), r.exams.len()), (1, 1, 0, 0));
        // ... and hidden again by each part of the one visibility rule; no snippet survives either
        for (off, on) in [
            ("UPDATE teacher_subjects SET status = 'rejected'", "UPDATE teacher_subjects SET status = 'approved'"),
            ("UPDATE subjects SET is_active = 0", "UPDATE subjects SET is_active = 1"),
            ("UPDATE institutions SET is_active = 0", "UPDATE institutions SET is_active = 1"),
            ("UPDATE users SET status = 'suspended' WHERE role = 'teacher'", "UPDATE users SET status = 'active' WHERE role = 'teacher'"),
        ] {
            w.conn.execute(off, []).unwrap();
            let r = find(&w, &w.me, "سري");
            assert_eq!((r.courses.len(), r.posts.len(), r.live.len(), r.exams.len()), (0, 0, 0, 0), "{off}");
            assert_eq!(r.counts.courses + r.counts.posts + r.counts.live + r.counts.exams, 0, "{off}: counts");
            w.conn.execute(on, []).unwrap();
        }
    }

    #[test]
    fn a_session_is_found_until_it_ends() {
        let w = world();
        live(&w, "l1", "حصة مباشرة", None, NOW - 30 * 60_000, "scheduled"); // started half an hour ago, lasts 60 minutes
        assert_eq!(ids(&find(&w, &w.me, "حصة").live), ["l1"], "a running session is still worth finding");
        let later = search(&w.conn, &w.me, &q("حصة"), NOW + 30 * 60_000).unwrap();
        assert!(later.live.is_empty() && later.counts.live == 0, "over exactly at its end");
    }

    #[test]
    fn exams_are_searched_for_students_of_the_subject_only() {
        let w = world();
        exam(&w, "e1", "s1", "امتحان الفصل الأول", "published", 1);
        exam(&w, "e2", "s2", "امتحان الفصل الثاني", "published", 2);
        let r = find(&w, &w.me, "امتحان");
        assert_eq!((ids(&r.exams), r.counts.exams), (vec!["e1"], 1), "enrolled in s1 only");
        for who in [&w.out, &w.teacher, &w.admin] {
            let r = find(&w, who, "امتحان");
            assert_eq!((r.exams.len(), r.counts.exams), (0, 0), "{}: exams exist only for students enrolled in their subject", who.role);
        }
        assert_eq!(ids(&search(&w.conn, &w.me, &SearchQuery { q: Some("امتحان".into()), group: Some("exams".into()), ..Default::default() }, NOW).unwrap().exams), ["e1"]);
        // enrolling in s2 adds the second exam
        w.conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,'s2',0)", params![w.me.id]).unwrap();
        assert_eq!(ids(&find(&w, &w.me, "امتحان").exams), ["e2", "e1"], "newest first");
    }

    #[test]
    fn people_are_found_only_while_they_are_active_teachers() {
        let w = world();
        let pending = insert_test_user(&w.conn, "p@x.com", "teacher", "pending");
        w.conn.execute("UPDATE users SET full_name = 'د. أحمد المعلق' WHERE id = ?1", params![pending.id]).unwrap();
        let student = insert_test_user(&w.conn, "s2@x.com", "student", "active");
        w.conn.execute("UPDATE users SET full_name = 'طالب اسمه أحمد' WHERE id = ?1", params![student.id]).unwrap();
        assert_eq!(ids(&find(&w, &w.me, "أحمد").teachers), [w.teacher.id.as_str()]);
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        assert_eq!(find(&w, &w.me, "أحمد").teachers.len(), 1, "the person is still findable; their content is what hides");
        assert!(find(&w, &w.me, "برمجة").subjects.is_empty(), "a subject of a hidden institution is not");
    }

    // ───────── groups, paging and counts ─────────

    #[test]
    fn without_a_group_each_group_returns_eight_and_the_counts_say_how_many_there_are() {
        let w = world();
        for n in 0..30 {
            course(&w, &format!("c{n:02}"), &format!("دورة الإحصاء {n}"), None, "published", n);
            post(&w, &format!("p{n:02}"), &format!("مقال الإحصاء {n}"), "نص", "published", n);
        }
        let r = find(&w, &w.me, "احصاء");
        assert_eq!((r.courses.len(), r.posts.len()), (8, 8));
        assert_eq!((r.counts.courses, r.counts.posts), (30, 30));
        assert_eq!(ids(&r.courses), ["c29", "c28", "c27", "c26", "c25", "c24", "c23", "c22"], "newest first");
        // limit / offset are ignored without a group
        let r = search(&w.conn, &w.me, &SearchQuery { q: Some("احصاء".into()), limit: Some(3), offset: Some(5), ..Default::default() }, NOW).unwrap();
        assert_eq!(r.courses.len(), 8);
    }

    #[test]
    fn with_a_group_only_that_group_is_filled_and_pages_never_repeat() {
        let w = world();
        for n in 0..60 {
            course(&w, &format!("c{n:02}"), &format!("دورة الإحصاء {n}"), None, "published", n);
        }
        post(&w, "p1", "الإحصاء", "نص", "published", 1);
        let by = |group: &str, limit: Option<i64>, offset: Option<i64>| search(&w.conn, &w.me, &SearchQuery { q: Some("احصاء".into()), group: Some(group.into()), limit, offset }, NOW).unwrap();
        let first = by("courses", None, None);
        assert_eq!(first.courses.len(), 20, "default page of 20");
        assert!(first.posts.is_empty() && first.teachers.is_empty() && first.subjects.is_empty() && first.live.is_empty() && first.exams.is_empty());
        assert_eq!((first.counts.courses, first.counts.posts), (60, 1), "the counts are for every group whichever one is asked for");
        assert_eq!(by("courses", Some(1000), None).courses.len(), 50, "50 at most");
        assert_eq!(by("courses", Some(0), None).courses.len(), 1, "never an empty page size");
        assert_eq!(by("courses", Some(-4), Some(-3)).courses.len(), 1);
        let mut seen: Vec<String> = Vec::new();
        for offset in [0, 25, 50] {
            seen.extend(by("courses", Some(25), Some(offset)).courses.into_iter().map(|h| h.id));
        }
        assert_eq!(seen.len(), 60);
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 60, "no repeats across pages");
        assert_eq!(seen[0], "c59");
        let past = by("courses", Some(25), Some(60));
        assert!(past.courses.is_empty(), "past the end");
        assert_eq!(past.counts.courses, 60, "a page past the end still reports the real total");
        assert_eq!(by("courses", Some(25), Some(50)).counts.courses, 60, "and so does the last, partial page");
        assert_eq!(ids(&by("posts", None, None).posts), ["p1"]);
        assert_eq!(by("exams", None, None).exams.len(), 0);
        assert_eq!(code(search(&w.conn, &w.me, &SearchQuery { q: Some("احصاء".into()), group: Some("bogus".into()), ..Default::default() }, NOW)).1, "invalid_group");
        // blank = absent
        assert_eq!(search(&w.conn, &w.me, &SearchQuery { q: Some("احصاء".into()), group: Some("  ".into()), ..Default::default() }, NOW).unwrap().courses.len(), 8);
    }

    #[test]
    fn the_response_always_has_every_key() {
        let w = world();
        let v = serde_json::to_value(find(&w, &w.out, "لا شيء")).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["counts", "courses", "exams", "live", "posts", "subjects", "teachers"]);
        let mut counts: Vec<&str> = v["counts"].as_object().unwrap().keys().map(String::as_str).collect();
        counts.sort();
        assert_eq!(counts, ["courses", "exams", "live", "posts", "subjects", "teachers"]);
        course(&w, "c1", "الإحصاء", Some("وصف"), "published", 1);
        let hit = serde_json::to_value(&find(&w, &w.out, "احصاء").courses[0]).unwrap();
        let mut names: Vec<&str> = hit.as_object().unwrap().keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["id", "link", "matched", "snippet", "subtitle", "title"]);
        // no join link anywhere in the search
        live(&w, "l1", "الإحصاء المباشر", None, NOW + H, "scheduled");
        assert!(!serde_json::to_string(&find(&w, &w.me, "احصاء")).unwrap().contains("secret.example"));
    }

    // ───────── cost (phase 4-2 review) ─────────

    #[test]
    fn only_the_start_of_a_post_body_is_searched_and_quoted() {
        let w = world();
        // ar_fold sees every text the search folds: record the longest one
        let longest = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = longest.clone();
        {
            use rusqlite::functions::FunctionFlags;
            use rusqlite::types::ValueRef;
            w.conn
                .create_scalar_function("ar_fold", 1, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, move |ctx| {
                    Ok(match ctx.get_raw(0) {
                        ValueRef::Text(b) => {
                            let t = String::from_utf8_lossy(b);
                            seen.fetch_max(t.chars().count(), std::sync::atomic::Ordering::Relaxed);
                            Some(ar_fold(&t))
                        }
                        _ => None,
                    })
                })
                .unwrap();
        }
        let filler = "حشو ".repeat(BODY_SEARCH_CHARS); // far longer than the searched part
        post(&w, "early", "مقال أول", &format!("الإحصاء في البداية {filler}"), "published", 3);
        post(&w, "late", "مقال ثان", &format!("{filler} الإحصاء في النهاية"), "published", 2);
        post(&w, "none", "مقال ثالث", &filler, "published", 1);
        let r = find(&w, &w.me, "احصاء");
        assert_eq!(ids(&r.posts), ["early"], "a match beyond the searched start is not found");
        assert_eq!(r.counts.posts, 1);
        assert!(r.posts[0].snippet.as_deref().unwrap().contains("الإحصاء"));
        assert!(longest.load(std::sync::atomic::Ordering::Relaxed) <= BODY_SEARCH_CHARS, "no body longer than {BODY_SEARCH_CHARS} characters is folded");
        // the title of a post is always searched whole, wherever the body matched
        post(&w, "title", "الإحصاء في العنوان", &filler, "published", 4);
        assert_eq!(ids(&find(&w, &w.me, "احصاء").posts), ["title", "early"]);
    }

    #[test]
    fn a_user_may_search_a_limited_number_of_times_a_minute() {
        let w = world();
        for i in 0..SEARCHES_PER_MINUTE {
            throttle(&w.conn, &w.me).unwrap_or_else(|e| panic!("search {i} refused: {e:?}"));
        }
        assert_eq!(code(throttle(&w.conn, &w.me)), (axum::http::StatusCode::TOO_MANY_REQUESTS, "rate_limited".to_string()));
        assert_eq!(code(throttle(&w.conn, &w.me)).1, "rate_limited", "and it stays refused inside the window");
        // each person has their own places
        throttle(&w.conn, &w.out).unwrap();
        throttle(&w.conn, &w.teacher).unwrap();
    }

    #[test]
    fn the_character_by_character_folding_of_snippets_is_the_folding_of_the_search() {
        for text in [
            "أَحْمَد  ـ الإبراهيمي", "  مقدمة\tفي\nالإحصاء  ", "ABC def ÀÉ İstanbul", "ٱلكتاب مصطفى آمنة", "", "   ", "ـ ً ُ", "a ً b", "نص 🙂 مُشَكَّل", "Straße ÉCOLE",
        ] {
            let folded: String = folded_with_origin(text).into_iter().map(|(c, _)| c).collect();
            assert_eq!(folded.trim_end(), ar_fold(text), "{text:?}");
        }
        // every folded character points at a real character of the original, in order
        let text = "أَحْمَد  الإبراهيمي";
        let origins: Vec<usize> = folded_with_origin(text).into_iter().map(|(_, i)| i).collect();
        assert!(origins.windows(2).all(|w| w[0] <= w[1]) && origins.iter().all(|i| *i < text.chars().count()));
    }
}
