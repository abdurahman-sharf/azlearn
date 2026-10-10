//! Teacher content: articles/summaries (with one attachment), video courses (lessons) and
//! live sessions. Content is visible to students only while it is published AND its teacher is
//! an active teacher approved for that subject (revoking the assignment hides it automatically).

use crate::platform::{audit, bad, db_err, like_pattern, lock, new_id, opt_text, require_active, require_role, text, Res, User, INSTITUTION_ACTIVE, SUBJECT_ACTIVE};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  original_name TEXT NOT NULL,
  mime TEXT NOT NULL,
  size INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS posts (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('article','summary')),
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('draft','published')),
  file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_posts_subject ON posts(subject_id, status);
CREATE INDEX IF NOT EXISTS idx_posts_teacher ON posts(teacher_id);
CREATE TABLE IF NOT EXISTS courses (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  status TEXT NOT NULL CHECK (status IN ('draft','published')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_courses_subject ON courses(subject_id, status);
CREATE TABLE IF NOT EXISTS lessons (
  id TEXT PRIMARY KEY,
  course_id TEXT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  section TEXT,
  title TEXT NOT NULL,
  description TEXT,
  video_url TEXT NOT NULL,
  embed_url TEXT,
  video_kind TEXT NOT NULL CHECK (video_kind IN ('youtube','vimeo','file','link')),
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lessons_course ON lessons(course_id, position);
CREATE TABLE IF NOT EXISTS live_sessions (
  id TEXT PRIMARY KEY,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  starts_at INTEGER NOT NULL,
  duration_min INTEGER NOT NULL,
  join_url TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('scheduled','cancelled')),
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_live_subject ON live_sessions(subject_id, starts_at);
";

/// Columns added after the first release (phase 3-4): when a post / course was first announced. `NULL` = never announced
/// (a draft). Idempotent; safe on every start.
///
/// Rows that are published already are **back-filled** with `created_at`: they were announced when they were published,
/// and without a value here unpublishing and publishing one again would announce it a second time. Running the back-fill
/// at every start (not only when the column is new) also covers a crash between adding the column and filling it; in
/// steady state it matches nothing, because every path that publishes claims `published_at` (see [`first_publication`]).
pub fn migrate(conn: &Connection) -> Result<(), String> {
    for table in ["posts", "courses"] {
        crate::platform::add_column_if_missing(conn, table, "published_at", "INTEGER")?;
        conn.execute(&format!("UPDATE {table} SET published_at = created_at WHERE status = 'published' AND published_at IS NULL"), [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_BODY_CHARS: usize = 20_000;
const MAX_URL_CHARS: usize = 500;

// ───────── validation helpers ─────────

fn one_of<'a>(v: &'a str, allowed: &[&str], code: &str) -> Res<&'a str> {
    if allowed.contains(&v) {
        Ok(v)
    } else {
        Err(bad(code))
    }
}

/// Only https links, no whitespace/control characters.
fn https_url(raw: &str) -> Res<url::Url> {
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > MAX_URL_CHARS || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad("invalid_url"));
    }
    let u = url::Url::parse(t).map_err(|_| bad("invalid_url"))?;
    if u.scheme() != "https" || u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() {
        return Err(bad("invalid_url"));
    }
    Ok(u)
}

fn valid_video_id(id: &str, len: Option<usize>) -> bool {
    !id.is_empty()
        && len.map_or(id.len() <= 20, |l| id.len() == l)
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Returns (kind, embed_url). The embed URL is computed here from a known provider, never
/// taken from user input, so the client can safely put it in an iframe.
pub fn parse_video(raw: &str) -> Res<(&'static str, Option<String>)> {
    let u = https_url(raw)?;
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").trim_start_matches("m.").to_lowercase();
    let segs: Vec<&str> = u.path_segments().map(|s| s.filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    match host.as_str() {
        "youtube.com" => {
            let id = match segs.first().copied() {
                Some("watch") => u.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned()),
                Some("embed") | Some("shorts") | Some("live") => segs.get(1).map(|s| s.to_string()),
                _ => None,
            };
            match id {
                Some(id) if valid_video_id(&id, Some(11)) => Ok(("youtube", Some(format!("https://www.youtube-nocookie.com/embed/{id}")))),
                _ => Ok(("link", None)),
            }
        }
        "youtu.be" => match segs.first() {
            Some(id) if valid_video_id(id, Some(11)) => Ok(("youtube", Some(format!("https://www.youtube-nocookie.com/embed/{id}")))),
            _ => Ok(("link", None)),
        },
        "vimeo.com" => match segs.first() {
            Some(id) if id.chars().all(|c| c.is_ascii_digit()) && valid_video_id(id, None) => {
                Ok(("vimeo", Some(format!("https://player.vimeo.com/video/{id}"))))
            }
            _ => Ok(("link", None)),
        },
        _ => {
            let path = u.path().to_lowercase();
            if [".mp4", ".webm", ".ogg"].iter().any(|e| path.ends_with(e)) {
                Ok(("file", Some(u.to_string())))
            } else {
                Ok(("link", None))
            }
        }
    }
}

/// An approved teaching assignment on an active subject **of an active institution** is required to create/publish
/// content (hiding an institution blocks publishing into it, just like hiding the subject).
pub(crate) fn can_publish(conn: &Connection, teacher_id: &str, subject_id: &str) -> Res<bool> {
    conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM teacher_subjects ts JOIN subjects s ON s.id = ts.subject_id
             WHERE ts.teacher_id = ?1 AND ts.subject_id = ?2 AND ts.status = 'approved' AND {SUBJECT_ACTIVE} AND {INSTITUTION_ACTIVE})"
        ),
        params![teacher_id, subject_id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

/// The teacher holds an *approved* assignment for the subject - and nothing more (no check that the subject or its
/// institution is active, which [`can_publish`] adds). For reading things that belong to the class as it is today.
pub(crate) fn is_approved_for(conn: &Connection, teacher_id: &str, subject_id: &str) -> Res<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2 AND status = 'approved')",
        params![teacher_id, subject_id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

/// 403 `not_assigned` unless [`can_publish`].
pub(crate) fn require_assignment(conn: &Connection, teacher_id: &str, subject_id: &str) -> Res<()> {
    if can_publish(conn, teacher_id, subject_id)? {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "not_assigned"))
    }
}

// ───────── why is something hidden from students? (one table, two readers) ─────────

/// Why a teacher's published item is not shown to students. The wire values are the snake_case names.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HiddenReason {
    /// The teacher's account is not an active teacher account (pending, rejected, suspended, role changed).
    AccountInactive,
    /// The subject's institution is switched off.
    InstitutionInactive,
    /// The subject itself is switched off.
    SubjectInactive,
    /// The teacher has no *approved* assignment for the subject (never asked, pending, rejected, withdrawn).
    NoAssignment,
}

impl HiddenReason {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "account_inactive" => Self::AccountInactive,
            "institution_inactive" => Self::InstitutionInactive,
            "subject_inactive" => Self::SubjectInactive,
            "no_assignment" => Self::NoAssignment,
            _ => return None,
        })
    }
}

/// A standing condition: the name [`HiddenReason`] reports when it fails and the SQL that holds when it is met.
pub(crate) type Standing = (&'static str, String);

/// The conditions, besides its own publication state, under which a teacher's content `a` is shown to students.
/// Expects aliases `u` (the owner, a LEFT JOIN for exams), `s` (subjects) and `a` = `a`. **This list is the only
/// place the rule lives:** [`visible`] and [`exam_standing`] AND it, [`hidden_reason_sql`] and [`exam_reason_sql`]
/// report the first failing entry, so they cannot drift. The order is the order of precedence of the reported
/// reason: the root cause comes first (a hidden institution before a hidden subject before a missing assignment),
/// because that is what the teacher cannot fix by asking again.
///
/// `ownerless_ok`: a row with no `teacher_id` is an **admin exam**; it has no teacher whose account or assignment
/// could fail, so those two conditions pass for it (the subject and the institution still must be active).
fn standing(a: &str, ownerless_ok: bool) -> [Standing; 4] {
    let owned = |cond: String| if ownerless_ok { format!("{a}.teacher_id IS NULL OR ({cond})") } else { cond };
    [
        ("account_inactive", owned("u.status = 'active' AND u.role = 'teacher'".to_string())),
        ("institution_inactive", INSTITUTION_ACTIVE.to_string()),
        ("subject_inactive", SUBJECT_ACTIVE.to_string()),
        (
            "no_assignment",
            owned(format!("EXISTS(SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = {a}.teacher_id AND ts.subject_id = {a}.subject_id AND ts.status = 'approved')")),
        ),
    ]
}

/// `(c1) AND (c2) AND …` — every standing condition holds.
pub(crate) fn all_hold(parts: &[Standing]) -> String {
    parts.iter().map(|(_, c)| format!("({c})")).collect::<Vec<_>>().join(" AND ")
}

/// `CASE WHEN NOT (c1) THEN 'reason1' WHEN NOT (c2) THEN 'reason2' … END`: NULL when every condition holds.
pub(crate) fn reason_case(parts: &[Standing]) -> String {
    let arms: Vec<String> = parts.iter().map(|(reason, c)| format!("WHEN NOT ({c}) THEN '{reason}'")).collect();
    format!("CASE {} END", arms.join(" "))
}

/// SQL condition: content `a` is published/scheduled AND its teacher is active and approved for the subject AND the
/// subject and its institution are active. Expects aliases `u` (users) and `s` (subjects) in the query.
pub fn visible(a: &str, live_status: &str) -> String {
    format!("{a}.status = '{live_status}' AND {}", all_hold(&standing(a, false)))
}

/// SQL expression (a column): why content `a` is hidden from students, or NULL when nothing stands in the way. It
/// ignores the item's own publication state (a draft has no reason). Same aliases as [`visible`].
pub fn hidden_reason_sql(a: &str) -> String {
    reason_case(&standing(a, false))
}

/// The same standing for an exam `a` (alias of `assessments`; `u` = LEFT JOIN of its teacher, `s` = its subject): an
/// admin exam needs no teacher, a teacher's exam needs the teacher in good standing, and in both cases the subject and
/// its institution must be active. Its own publication state (`published`/`closed`) is the caller's business.
pub(crate) fn exam_standing(a: &str) -> String {
    all_hold(&standing(a, true))
}

/// SQL column: why exam `a` is hidden from students (NULL = nothing stands in the way); see [`exam_standing`].
pub(crate) fn exam_reason_sql(a: &str) -> String {
    reason_case(&standing(a, true))
}

/// What the owner sees next to an item: shown to students right now, and if it is published but not shown, why.
/// A draft / cancelled / ended / archived item is simply not shown (`visible = false`, no reason): it is not hidden,
/// it is not published.
pub(crate) fn presence(public_state: bool, reason: Option<String>) -> (bool, Option<HiddenReason>) {
    if !public_state {
        return (false, None);
    }
    let reason = reason.as_deref().and_then(HiddenReason::parse);
    (reason.is_none(), reason)
}

#[derive(Clone, Copy)]
enum Scope<'a> {
    Subject(&'a str),
    Teacher(&'a str),
    Mine(&'a str),
    Feed(&'a str),
}

fn scope_clause(scope: Scope, a: &str, status: &str) -> String {
    match scope {
        Scope::Subject(_) => format!("{} AND {a}.subject_id = ?1", visible(a, status)),
        Scope::Teacher(_) => format!("{} AND {a}.teacher_id = ?1", visible(a, status)),
        Scope::Mine(_) => format!("{a}.teacher_id = ?1"),
        Scope::Feed(_) => format!(
            "{} AND ({a}.subject_id IN (SELECT subject_id FROM subject_enrollments WHERE student_id = ?1)
                  OR {a}.teacher_id IN (SELECT teacher_id FROM teacher_follows WHERE student_id = ?1))",
            visible(a, status)
        ),
    }
}

fn scope_id<'a>(scope: Scope<'a>) -> &'a str {
    match scope {
        Scope::Subject(i) | Scope::Teacher(i) | Scope::Mine(i) | Scope::Feed(i) => i,
    }
}

/// Claims the ONE announcement of a post or course: `published_at` is set with a conditional update and never
/// overwritten, so unpublishing and publishing again (or any later save) does not tell every follower a second time -
/// the same rule as for exams (`platform_exams::announce`). `table` is always a literal of this module.
fn first_publication(conn: &Connection, table: &str, id: &str, now: i64) -> bool {
    conn.execute(&format!("UPDATE {table} SET published_at = ?2 WHERE id = ?1 AND published_at IS NULL"), params![id, now]).map_or(false, |n| n == 1)
}

/// Tells followers/enrolled students about new content (callers announce only on the first publication).
fn announce(conn: &Connection, teacher: &User, subject_id: &str, kind: &str, title: &str, link: &str) {
    crate::platform_engage::notify_audience(
        conn,
        &teacher.id,
        subject_id,
        kind,
        serde_json::json!({ "title": title, "teacher": teacher.full_name }),
        link,
    );
}

/// The owner of a piece of content as the `teacher` of [`announce`], for when somebody else (an admin) is the one acting.
fn owner_user(id: &str, full_name: &str) -> User {
    User { id: id.to_string(), email: String::new(), full_name: full_name.to_string(), role: "teacher".into(), institution_type: None, status: "active".into(), status_reason: None }
}

/// Moderation by someone other than the owner: tell the owner.
fn tell_owner_moderated(conn: &Connection, owner: &str, title: &str, link: &str) {
    crate::platform_engage::notify(conn, owner, "content_unpublished", serde_json::json!({ "title": title }), link);
}

fn can_manage(conn: &Connection, table: &str, id: &str, user: &User) -> Res<String> {
    // `table` is always a literal from this module.
    let owner: String = conn
        .query_row(&format!("SELECT teacher_id FROM {table} WHERE id = ?1"), params![id], |r| r.get(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if owner == user.id || user.role == "admin" {
        Ok(owner)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

/// Non-owner admins may only unpublish/cancel (moderation), never edit.
fn admin_only_status(user: &User, owner: &str, patch_has_other_fields: bool) -> Res<()> {
    if user.id != owner && patch_has_other_fields {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(())
}

// ───────── posts ─────────

#[derive(Serialize, Debug)]
pub struct FileInfo {
    id: String,
    name: String,
    size: i64,
}

#[derive(Serialize, Debug)]
pub struct Post {
    id: String,
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    kind: String,
    title: String,
    excerpt: String,
    body: Option<String>,
    status: String,
    file: Option<FileInfo>,
    created_at: i64,
    updated_at: i64,
}

const POST_COLS: &str = "p.id, p.teacher_id, u.full_name, p.subject_id, s.name_ar, p.kind, p.title, p.body, p.status,
        p.file_id, f.original_name, f.size, p.created_at, p.updated_at";
const POST_FROM: &str = "FROM posts p JOIN users u ON u.id = p.teacher_id JOIN subjects s ON s.id = p.subject_id
     LEFT JOIN files f ON f.id = p.file_id";

/// The post columns (`map_post` reads them), then `extra` (e.g. `, <hidden-reason expression>`), then the joins.
fn post_select(extra: &str) -> String {
    format!("SELECT {POST_COLS}{extra} {POST_FROM}")
}

fn map_post(with_body: bool) -> impl Fn(&rusqlite::Row) -> rusqlite::Result<Post> {
    move |r| {
        let body: String = r.get(7)?;
        let file = match (r.get::<_, Option<String>>(9)?, r.get::<_, Option<String>>(10)?, r.get::<_, Option<i64>>(11)?) {
            (Some(id), Some(name), Some(size)) => Some(FileInfo { id, name, size }),
            _ => None,
        };
        Ok(Post {
            id: r.get(0)?,
            teacher_id: r.get(1)?,
            teacher_name: r.get(2)?,
            subject_id: r.get(3)?,
            subject_name: r.get(4)?,
            kind: r.get(5)?,
            title: r.get(6)?,
            excerpt: body.chars().take(200).collect(),
            body: with_body.then_some(body),
            status: r.get(8)?,
            file,
            created_at: r.get(12)?,
            updated_at: r.get(13)?,
        })
    }
}

fn list_posts(conn: &Connection, scope: Scope) -> Res<Vec<Post>> {
    let sql = format!("{} WHERE {} ORDER BY p.updated_at DESC LIMIT 100", post_select(""), scope_clause(scope, "p", "published"));
    conn.prepare(&sql)
        .map_err(db_err)?
        .query_map(params![scope_id(scope)], map_post(false))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

fn get_post(conn: &Connection, id: &str) -> Res<Post> {
    conn.query_row(&format!("{} WHERE p.id = ?1", post_select("")), params![id], map_post(true))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

pub(crate) fn visible_to_public(conn: &Connection, table: &str, a: &str, id: &str, status: &str) -> Res<bool> {
    conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM {table} {a} JOIN users u ON u.id = {a}.teacher_id JOIN subjects s ON s.id = {a}.subject_id
             WHERE {a}.id = ?1 AND {})",
            visible(a, status)
        ),
        params![id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

#[derive(Deserialize)]
pub struct PostReq {
    subject_id: Option<String>,
    kind: Option<String>,
    title: Option<String>,
    body: Option<String>,
    status: Option<String>,
}

fn create_post(conn: &Connection, user: &User, r: &PostReq) -> Res<Post> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    require_assignment(conn, &user.id, subject_id)?;
    let kind = one_of(r.kind.as_deref().unwrap_or("article"), &["article", "summary"], "invalid_kind")?;
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let body = text(r.body.as_deref().unwrap_or(""), MAX_BODY_CHARS, "invalid_body")?;
    let status = one_of(r.status.as_deref().unwrap_or("draft"), &["draft", "published"], "invalid_status")?;
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO posts(id, teacher_id, subject_id, kind, title, body, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id, user.id, subject_id, kind, title, body, status, now],
    )
    .map_err(db_err)?;
    if status == "published" && first_publication(conn, "posts", &id, now) {
        announce(conn, user, subject_id, "new_post", &title, &format!("/platform/posts/{id}"));
    }
    get_post(conn, &id)
}

fn update_post(conn: &Connection, user: &User, id: &str, r: &PostReq) -> Res<Post> {
    let owner = can_manage(conn, "posts", id, user)?;
    let other = r.kind.is_some() || r.title.is_some() || r.body.is_some() || r.subject_id.is_some();
    admin_only_status(user, &owner, other)?;
    let cur = get_post(conn, id)?;
    let kind = one_of(r.kind.as_deref().unwrap_or(&cur.kind), &["article", "summary"], "invalid_kind")?.to_string();
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title.clone(),
    };
    let body = match &r.body {
        Some(b) => text(b, MAX_BODY_CHARS, "invalid_body")?,
        None => cur.body.clone().unwrap_or_default(),
    };
    let status = one_of(r.status.as_deref().unwrap_or(&cur.status), &["draft", "published"], "invalid_status")?.to_string();
    // Whoever turns it on (the owner on any save that leaves it published, an admin on a transition) needs the
    // owner to hold a live assignment: nothing is published into a subject it cannot be seen in.
    if status == "published" && (user.id == owner || cur.status != "published") {
        require_assignment(conn, &owner, &cur.subject_id)?;
    }
    conn.execute(
        "UPDATE posts SET kind=?1, title=?2, body=?3, status=?4, updated_at=?5 WHERE id=?6",
        params![kind, title, body, status, now_ms(), id],
    )
    .map_err(db_err)?;
    if user.id != owner {
        audit(conn, &user.id, id, "post_moderated", &status);
        if status == "draft" && cur.status == "published" {
            tell_owner_moderated(conn, &owner, &title, &format!("/platform/posts/{id}"));
        }
    }
    // The first publication is announced once, whoever makes it (the owner, or an admin publishing the owner's draft):
    // every path that publishes claims `published_at`, so the announcement can neither be repeated nor skipped.
    if status == "published" && cur.status != "published" && first_publication(conn, "posts", id, now_ms()) {
        announce(conn, &owner_user(&owner, &cur.teacher_name), &cur.subject_id, "new_post", &title, &format!("/platform/posts/{id}"));
    }
    get_post(conn, id)
}

fn remove_file_row(state: &AppState, conn: &Connection, file_id: &str) {
    let _ = conn.execute("DELETE FROM files WHERE id = ?1", params![file_id]);
    let _ = std::fs::remove_file(state.platform.files_dir.join(file_id));
}

pub async fn create_post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(r): Json<PostReq>,
) -> Res<(StatusCode, Json<Post>)> {
    let user = require_role(&state, &headers, "teacher")?;
    create_post(&*lock(&state)?, &user, &r).map(|p| (StatusCode::CREATED, Json(p)))
}

pub async fn get_post_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<Post>> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    let post = get_post(&conn, &id)?;
    if post.teacher_id != user.id && user.role != "admin" && !visible_to_public(&conn, "posts", "p", &id, "published")? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    Ok(Json(post))
}

pub async fn update_post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<PostReq>,
) -> Res<Json<Post>> {
    let user = require_active(&state, &headers)?;
    update_post(&*lock(&state)?, &user, &id, &r).map(Json)
}

/// Deletes the post row and everything that points at it (reports); returns the attachment to remove from disk.
/// All or nothing.
fn delete_post(conn: &Connection, user: &User, id: &str) -> Res<Option<String>> {
    can_manage(conn, "posts", id, user)?;
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let file: Option<String> = tx.query_row("SELECT file_id FROM posts WHERE id = ?1", params![id], |r| r.get(0)).map_err(db_err)?;
    tx.execute("DELETE FROM posts WHERE id = ?1", params![id]).map_err(db_err)?;
    crate::platform_ops::purge_target_references(&tx, "post", id)?;
    audit(&tx, &user.id, id, "post_deleted", "");
    tx.commit().map_err(db_err)?;
    Ok(file)
}

pub async fn delete_post_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    if let Some(f) = delete_post(&conn, &user, &id)? {
        remove_file_row(&state, &conn, &f);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ───────── duplicating (phase 3-2) ─────────

/// Optional body of the duplicate routes: the new item's title (blank = "<title> (نسخة)").
#[derive(Deserialize, Default)]
pub struct DuplicateReq {
    title: Option<String>,
}

const COPY_SUFFIX: &str = " (نسخة)";

/// The title of a copy: the explicit one when given, otherwise the original's with a suffix, cut to the 200
/// characters a title may have.
fn copy_title(requested: &Option<String>, original: &str) -> Res<String> {
    if let Some(t) = opt_text(requested, 200, "invalid_title")? {
        return Ok(t);
    }
    let room = 200 - COPY_SUFFIX.chars().count();
    Ok(format!("{}{COPY_SUFFIX}", original.chars().take(room).collect::<String>()))
}

/// Only the **owner** duplicates (an admin may moderate but never author in someone's name), and the owner must still
/// hold a live assignment for the subject, like for creating anything.
fn own_for_copy(conn: &Connection, table: &str, id: &str, user: &User) -> Res<()> {
    if can_manage(conn, table, id, user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(())
}

/// A new **draft** post with the same kind, body and subject. The attachment is not copied (a file belongs to one post).
fn duplicate_post(conn: &Connection, user: &User, id: &str, req: &DuplicateReq) -> Res<Post> {
    own_for_copy(conn, "posts", id, user)?;
    let cur = get_post(conn, id)?;
    require_assignment(conn, &user.id, &cur.subject_id)?;
    let title = copy_title(&req.title, &cur.title)?;
    let new_id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO posts(id, teacher_id, subject_id, kind, title, body, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,'draft',?7,?7)",
        params![new_id, user.id, cur.subject_id, cur.kind, title, cur.body.unwrap_or_default(), now],
    )
    .map_err(db_err)?;
    get_post(conn, &new_id)
}

pub async fn duplicate_post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<DuplicateReq>>,
) -> Res<(StatusCode, Json<Post>)> {
    let user = require_role(&state, &headers, "teacher")?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    duplicate_post(&*lock(&state)?, &user, &id, &req).map(|p| (StatusCode::CREATED, Json(p)))
}

// ───────── attachments ─────────

/// (mime, magic-bytes check) for the extensions we accept. HTML/SVG/scripts are deliberately absent.
fn file_kind(ext: &str) -> Option<(&'static str, fn(&[u8]) -> bool)> {
    fn pdf(b: &[u8]) -> bool { b.starts_with(b"%PDF") }
    fn zip(b: &[u8]) -> bool { b.starts_with(b"PK\x03\x04") }
    fn png(b: &[u8]) -> bool { b.starts_with(b"\x89PNG\r\n\x1a\n") }
    fn jpg(b: &[u8]) -> bool { b.starts_with(&[0xFF, 0xD8, 0xFF]) }
    fn txt(b: &[u8]) -> bool { !b.contains(&0) }
    Some(match ext {
        "pdf" => ("application/pdf", pdf),
        "docx" => ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", zip),
        "pptx" => ("application/vnd.openxmlformats-officedocument.presentationml.presentation", zip),
        "xlsx" => ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", zip),
        "png" => ("image/png", png),
        "jpg" | "jpeg" => ("image/jpeg", jpg),
        "txt" => ("text/plain; charset=utf-8", txt),
        _ => return None,
    })
}

fn validate_upload(original: &str, bytes: &[u8]) -> Res<(&'static str, String)> {
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err(bad("invalid_file_size"));
    }
    let base: String = original
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let (mime, magic) = file_kind(&ext).ok_or_else(|| bad("file_type_not_allowed"))?;
    if !magic(bytes) {
        return Err(bad("file_content_mismatch"));
    }
    Ok((mime, base))
}

pub async fn upload_file_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(post_id): Path<String>,
    mut mp: Multipart,
) -> Res<Json<FileInfo>> {
    let user = require_active(&state, &headers)?;
    {
        let conn = lock(&state)?;
        if can_manage(&conn, "posts", &post_id, &user)? != user.id {
            return Err(err(StatusCode::FORBIDDEN, "forbidden"));
        }
    }
    let mut upload: Option<(String, Vec<u8>)> = None;
    while let Some(field) = mp.next_field().await.map_err(|_| bad("invalid_upload"))? {
        if field.name() == Some("file") {
            let name = field.file_name().unwrap_or("").to_string();
            let bytes = field.bytes().await.map_err(|_| bad("invalid_file_size"))?;
            upload = Some((name, bytes.to_vec()));
            break;
        }
    }
    let (name, bytes) = upload.ok_or_else(|| bad("invalid_upload"))?;
    let (mime, safe_name) = validate_upload(&name, &bytes)?;
    let file_id = new_id();
    std::fs::write(state.platform.files_dir.join(&file_id), &bytes).map_err(db_err)?;
    let conn = lock(&state)?;
    let old: Option<String> = conn.query_row("SELECT file_id FROM posts WHERE id = ?1", params![post_id], |r| r.get(0)).map_err(db_err)?;
    conn.execute(
        "INSERT INTO files(id, owner_id, original_name, mime, size, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![file_id, user.id, safe_name, mime, bytes.len() as i64, now_ms()],
    )
    .map_err(db_err)?;
    conn.execute("UPDATE posts SET file_id = ?1, updated_at = ?2 WHERE id = ?3", params![file_id, now_ms(), post_id]).map_err(db_err)?;
    if let Some(o) = old {
        remove_file_row(&state, &conn, &o);
    }
    Ok(Json(FileInfo { id: file_id, name: safe_name, size: bytes.len() as i64 }))
}

pub async fn remove_file_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(post_id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    if can_manage(&conn, "posts", &post_id, &user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let old: Option<String> = conn.query_row("SELECT file_id FROM posts WHERE id = ?1", params![post_id], |r| r.get(0)).map_err(db_err)?;
    conn.execute("UPDATE posts SET file_id = NULL WHERE id = ?1", params![post_id]).map_err(db_err)?;
    if let Some(o) = old {
        remove_file_row(&state, &conn, &o);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Download is allowed for the owner, admins, and anyone who can see the post.
pub async fn download_file_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(file_id): Path<String>) -> Res<Response> {
    let user = require_active(&state, &headers)?;
    let (name, mime): (String, String) = {
        let conn = lock(&state)?;
        let row = conn
            .query_row::<(String, String, String, String), _, _>(
                "SELECT f.original_name, f.mime, p.id, p.teacher_id FROM files f JOIN posts p ON p.file_id = f.id WHERE f.id = ?1",
                params![file_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
        if row.3 != user.id && user.role != "admin" && !visible_to_public(&conn, "posts", "p", &row.2, "published")? {
            return Err(err(StatusCode::NOT_FOUND, "not_found"));
        }
        (row.0, row.1)
    };
    let bytes = std::fs::read(state.platform.files_dir.join(&file_id)).map_err(|_| err(StatusCode::NOT_FOUND, "not_found"))?;
    let encoded: String = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>().replace('+', "%20");
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_DISPOSITION, format!("attachment; filename*=UTF-8''{encoded}")),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CACHE_CONTROL, "private, max-age=0".to_string()),
        ],
        bytes,
    )
        .into_response())
}

/// Deletes file rows (and disk files) that no post references any more.
pub fn purge_orphan_files(state: &crate::platform::PlatformState) {
    let Ok(conn) = state.conn.lock() else { return };
    let cutoff = now_ms() - 3_600_000;
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM files WHERE created_at < ?1 AND id NOT IN (SELECT file_id FROM posts WHERE file_id IS NOT NULL)")
        .and_then(|mut s| s.query_map(params![cutoff], |r| r.get(0))?.collect())
        .unwrap_or_default();
    for id in ids {
        let _ = conn.execute("DELETE FROM files WHERE id = ?1", params![id]);
        let _ = std::fs::remove_file(state.files_dir.join(&id));
    }
}

// ───────── courses & lessons ─────────

#[derive(Serialize, Debug)]
pub struct Course {
    id: String,
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    title: String,
    description: Option<String>,
    status: String,
    lesson_count: i64,
    updated_at: i64,
}

#[derive(Serialize, Debug)]
pub struct Lesson {
    id: String,
    course_id: String,
    position: i64,
    section: Option<String>,
    title: String,
    description: Option<String>,
    video_url: String,
    embed_url: Option<String>,
    video_kind: String,
}

#[derive(Serialize, Debug)]
pub struct CourseDetail {
    #[serde(flatten)]
    course: Course,
    lessons: Vec<Lesson>,
}

const COURSE_COLS: &str = "c.id, c.teacher_id, u.full_name, c.subject_id, s.name_ar, c.title, c.description, c.status,
        (SELECT count(*) FROM lessons l WHERE l.course_id = c.id), c.updated_at";
const COURSE_FROM: &str = "FROM courses c JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id";

/// See [`post_select`].
fn course_select(extra: &str) -> String {
    format!("SELECT {COURSE_COLS}{extra} {COURSE_FROM}")
}

fn map_course(r: &rusqlite::Row) -> rusqlite::Result<Course> {
    Ok(Course {
        id: r.get(0)?,
        teacher_id: r.get(1)?,
        teacher_name: r.get(2)?,
        subject_id: r.get(3)?,
        subject_name: r.get(4)?,
        title: r.get(5)?,
        description: r.get(6)?,
        status: r.get(7)?,
        lesson_count: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

fn list_courses(conn: &Connection, scope: Scope) -> Res<Vec<Course>> {
    let sql = format!("{} WHERE {} ORDER BY c.updated_at DESC LIMIT 100", course_select(""), scope_clause(scope, "c", "published"));
    conn.prepare(&sql)
        .map_err(db_err)?
        .query_map(params![scope_id(scope)], map_course)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

fn get_course(conn: &Connection, id: &str) -> Res<CourseDetail> {
    let course = conn
        .query_row(&format!("{} WHERE c.id = ?1", course_select("")), params![id], map_course)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let lessons = conn
        .prepare(
            "SELECT id, course_id, position, section, title, description, video_url, embed_url, video_kind
             FROM lessons WHERE course_id = ?1 ORDER BY position, created_at",
        )
        .map_err(db_err)?
        .query_map(params![id], |r| {
            Ok(Lesson {
                id: r.get(0)?,
                course_id: r.get(1)?,
                position: r.get(2)?,
                section: r.get(3)?,
                title: r.get(4)?,
                description: r.get(5)?,
                video_url: r.get(6)?,
                embed_url: r.get(7)?,
                video_kind: r.get(8)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(CourseDetail { course, lessons })
}

#[derive(Deserialize)]
pub struct CourseReq {
    subject_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    status: Option<String>,
}

fn create_course(conn: &Connection, user: &User, r: &CourseReq) -> Res<CourseDetail> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    require_assignment(conn, &user.id, subject_id)?;
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let status = one_of(r.status.as_deref().unwrap_or("draft"), &["draft", "published"], "invalid_status")?;
    if status == "published" {
        return Err(bad("course_needs_lessons")); // a course that has just been created has no lessons yet
    }
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO courses(id, teacher_id, subject_id, title, description, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
        params![id, user.id, subject_id, title, description, status, now],
    )
    .map_err(db_err)?;
    get_course(conn, &id)
}

fn update_course(conn: &Connection, user: &User, id: &str, r: &CourseReq) -> Res<CourseDetail> {
    let owner = can_manage(conn, "courses", id, user)?;
    admin_only_status(user, &owner, r.title.is_some() || r.description.is_some())?;
    let cur = get_course(conn, id)?.course;
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title,
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { cur.description };
    let status = one_of(r.status.as_deref().unwrap_or(&cur.status), &["draft", "published"], "invalid_status")?.to_string();
    if status == "published" && (user.id == owner || cur.status != "published") {
        require_assignment(conn, &owner, &cur.subject_id)?;
    }
    // Publishing is the draft -> published step; a course with nothing to watch is not published (and not announced).
    if status == "published" && cur.status != "published" && cur.lesson_count == 0 {
        return Err(bad("course_needs_lessons"));
    }
    conn.execute(
        "UPDATE courses SET title=?1, description=?2, status=?3, updated_at=?4 WHERE id=?5",
        params![title, description, status, now_ms(), id],
    )
    .map_err(db_err)?;
    if user.id != owner {
        audit(conn, &user.id, id, "course_moderated", &status);
        if status == "draft" && cur.status == "published" {
            tell_owner_moderated(conn, &owner, &title, &format!("/platform/courses/{id}"));
        }
    }
    // announced once on the first publication, by whoever makes it (see `update_post`)
    if status == "published" && cur.status != "published" && first_publication(conn, "courses", id, now_ms()) {
        announce(conn, &owner_user(&owner, &cur.teacher_name), &cur.subject_id, "new_course", &title, &format!("/platform/courses/{id}"));
    }
    get_course(conn, id)
}

#[derive(Deserialize)]
pub struct LessonReq {
    title: Option<String>,
    description: Option<String>,
    section: Option<String>,
    video_url: Option<String>,
}

fn add_lesson(conn: &Connection, user: &User, course_id: &str, r: &LessonReq) -> Res<Lesson> {
    if can_manage(conn, "courses", course_id, user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let section = opt_text(&r.section, 120, "section_too_long")?;
    let (kind, embed) = parse_video(r.video_url.as_deref().unwrap_or(""))?;
    let url = r.video_url.as_deref().unwrap_or("").trim().to_string();
    let pos: i64 = conn
        .query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM lessons WHERE course_id = ?1", params![course_id], |r| r.get(0))
        .map_err(db_err)?;
    let id = new_id();
    conn.execute(
        "INSERT INTO lessons(id, course_id, position, section, title, description, video_url, embed_url, video_kind, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![id, course_id, pos, section, title, description, url, embed, kind, now_ms()],
    )
    .map_err(db_err)?;
    conn.execute("UPDATE courses SET updated_at = ?1 WHERE id = ?2", params![now_ms(), course_id]).map_err(db_err)?;
    Ok(Lesson { id, course_id: course_id.into(), position: pos, section, title, description, video_url: url, embed_url: embed, video_kind: kind.into() })
}

fn lesson_course(conn: &Connection, lesson_id: &str) -> Res<String> {
    conn.query_row("SELECT course_id FROM lessons WHERE id = ?1", params![lesson_id], |r| r.get(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn update_lesson(conn: &Connection, user: &User, id: &str, r: &LessonReq) -> Res<()> {
    let course = lesson_course(conn, id)?;
    if can_manage(conn, "courses", &course, user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let (title, description, section, url, embed, kind): (String, Option<String>, Option<String>, String, Option<String>, String) = conn
        .query_row(
            "SELECT title, description, section, video_url, embed_url, video_kind FROM lessons WHERE id = ?1",
            params![id],
            |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?, x.get(5)?)),
        )
        .map_err(db_err)?;
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => title,
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { description };
    let section = if r.section.is_some() { opt_text(&r.section, 120, "section_too_long")? } else { section };
    let (url, embed, kind) = match &r.video_url {
        Some(u) => {
            let (k, e) = parse_video(u)?;
            (u.trim().to_string(), e, k.to_string())
        }
        None => (url, embed, kind),
    };
    conn.execute(
        "UPDATE lessons SET title=?1, description=?2, section=?3, video_url=?4, embed_url=?5, video_kind=?6 WHERE id=?7",
        params![title, description, section, url, embed, kind, id],
    )
    .map_err(db_err)?;
    Ok(())
}

fn delete_lesson(conn: &Connection, user: &User, id: &str) -> Res<()> {
    let course = lesson_course(conn, id)?;
    can_manage(conn, "courses", &course, user)?;
    conn.execute("DELETE FROM lessons WHERE id = ?1", params![id]).map_err(db_err)?;
    Ok(())
}

/// Swaps a lesson with its neighbour; `up` moves it earlier.
fn move_lesson(conn: &Connection, user: &User, id: &str, up: bool) -> Res<()> {
    let course = lesson_course(conn, id)?;
    if can_manage(conn, "courses", &course, user)? != user.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM lessons WHERE course_id = ?1 ORDER BY position, created_at")
        .map_err(db_err)?
        .query_map(params![course], |r| r.get(0))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let i = ids.iter().position(|x| x == id).ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let j = if up { i.checked_sub(1) } else { (i + 1 < ids.len()).then_some(i + 1) };
    let mut order = ids;
    if let Some(j) = j {
        order.swap(i, j);
    }
    // Renumber densely so positions stay unique and stable.
    for (n, lid) in order.iter().enumerate() {
        conn.execute("UPDATE lessons SET position = ?1 WHERE id = ?2", params![n as i64 + 1, lid]).map_err(db_err)?;
    }
    Ok(())
}

pub async fn create_course_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<CourseReq>) -> Res<(StatusCode, Json<CourseDetail>)> {
    let user = require_role(&state, &headers, "teacher")?;
    create_course(&*lock(&state)?, &user, &r).map(|c| (StatusCode::CREATED, Json(c)))
}

pub async fn get_course_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<CourseDetail>> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    let c = get_course(&conn, &id)?;
    if c.course.teacher_id != user.id && user.role != "admin" && !visible_to_public(&conn, "courses", "c", &id, "published")? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    Ok(Json(c))
}

pub async fn update_course_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<CourseReq>,
) -> Res<Json<CourseDetail>> {
    let user = require_active(&state, &headers)?;
    update_course(&*lock(&state)?, &user, &id, &r).map(Json)
}

/// Deletes the course (its lessons and their progress cascade) together with the reviews of the course and the
/// reports about it or about those reviews, none of which has a foreign key to it. All or nothing.
fn delete_course(conn: &Connection, user: &User, id: &str) -> Res<()> {
    can_manage(conn, "courses", id, user)?;
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    crate::platform_ops::purge_target_references(&tx, "course", id)?;
    tx.execute("DELETE FROM courses WHERE id = ?1", params![id]).map_err(db_err)?;
    audit(&tx, &user.id, id, "course_deleted", "");
    tx.commit().map_err(db_err)?;
    Ok(())
}

pub async fn delete_course_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    delete_course(&*lock(&state)?, &user, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// A new **draft** course with copies of all the lessons (same order, sections, descriptions and links; new ids, so
/// nobody's progress is carried over). All or nothing.
fn duplicate_course(conn: &Connection, user: &User, id: &str, req: &DuplicateReq) -> Res<CourseDetail> {
    own_for_copy(conn, "courses", id, user)?;
    let cur = get_course(conn, id)?;
    require_assignment(conn, &user.id, &cur.course.subject_id)?;
    let title = copy_title(&req.title, &cur.course.title)?;
    let new_course = new_id();
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO courses(id, teacher_id, subject_id, title, description, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,'draft',?6,?6)",
        params![new_course, user.id, cur.course.subject_id, title, cur.course.description, now],
    )
    .map_err(db_err)?;
    for l in &cur.lessons {
        tx.execute(
            "INSERT INTO lessons(id, course_id, position, section, title, description, video_url, embed_url, video_kind, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![new_id(), new_course, l.position, l.section, l.title, l.description, l.video_url, l.embed_url, l.video_kind, now],
        )
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    get_course(conn, &new_course)
}

pub async fn duplicate_course_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<DuplicateReq>>,
) -> Res<(StatusCode, Json<CourseDetail>)> {
    let user = require_role(&state, &headers, "teacher")?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    duplicate_course(&*lock(&state)?, &user, &id, &req).map(|c| (StatusCode::CREATED, Json(c)))
}

pub async fn add_lesson_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(course_id): Path<String>,
    Json(r): Json<LessonReq>,
) -> Res<(StatusCode, Json<Lesson>)> {
    let user = require_role(&state, &headers, "teacher")?;
    add_lesson(&*lock(&state)?, &user, &course_id, &r).map(|l| (StatusCode::CREATED, Json(l)))
}

pub async fn update_lesson_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<LessonReq>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "teacher")?;
    update_lesson(&*lock(&state)?, &user, &id, &r)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_lesson_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    delete_lesson(&*lock(&state)?, &user, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct MoveReq {
    direction: String,
}

pub async fn move_lesson_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<MoveReq>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "teacher")?;
    let up = one_of(&r.direction, &["up", "down"], "invalid_direction")? == "up";
    move_lesson(&*lock(&state)?, &user, &id, up)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── live sessions ─────────

#[derive(Serialize, Debug)]
pub struct Live {
    id: String,
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    title: String,
    description: Option<String>,
    starts_at: i64,
    duration_min: i64,
    join_url: String,
    status: String,
}

const LIVE_COLS: &str = "l.id, l.teacher_id, u.full_name, l.subject_id, s.name_ar, l.title, l.description,
        l.starts_at, l.duration_min, l.join_url, l.status";
const LIVE_FROM: &str = "FROM live_sessions l JOIN users u ON u.id = l.teacher_id JOIN subjects s ON s.id = l.subject_id";

/// See [`post_select`].
fn live_select(extra: &str) -> String {
    format!("SELECT {LIVE_COLS}{extra} {LIVE_FROM}")
}

fn map_live(r: &rusqlite::Row) -> rusqlite::Result<Live> {
    Ok(Live {
        id: r.get(0)?,
        teacher_id: r.get(1)?,
        teacher_name: r.get(2)?,
        subject_id: r.get(3)?,
        subject_name: r.get(4)?,
        title: r.get(5)?,
        description: r.get(6)?,
        starts_at: r.get(7)?,
        duration_min: r.get(8)?,
        join_url: r.get(9)?,
        status: r.get(10)?,
    })
}

/// Public scopes only list sessions that have not ended yet; `Mine` lists everything.
fn list_live(conn: &Connection, scope: Scope, now: i64) -> Res<Vec<Live>> {
    let not_ended = if matches!(scope, Scope::Mine(_)) { "" } else { " AND l.starts_at + l.duration_min * 60000 > ?2" };
    let sql = format!(
        "{} WHERE {}{not_ended} ORDER BY l.starts_at {} LIMIT 100",
        live_select(""),
        scope_clause(scope, "l", "scheduled"),
        if matches!(scope, Scope::Mine(_)) { "DESC" } else { "ASC" }
    );
    let mut stmt = conn.prepare(&sql).map_err(db_err)?;
    let rows = if matches!(scope, Scope::Mine(_)) {
        stmt.query_map(params![scope_id(scope)], map_live)
    } else {
        stmt.query_map(params![scope_id(scope), now], map_live)
    }
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)?;
    Ok(rows)
}

#[derive(Deserialize)]
pub struct LiveReq {
    subject_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    starts_at: Option<i64>,
    duration_min: Option<i64>,
    join_url: Option<String>,
    status: Option<String>,
}

fn check_live_time(starts_at: i64, duration: i64) -> Res<()> {
    let now = now_ms();
    if !(5..=480).contains(&duration) || starts_at < now - 3_600_000 || starts_at > now + 366 * 86_400_000 {
        return Err(bad("invalid_time"));
    }
    Ok(())
}

fn get_live(conn: &Connection, id: &str) -> Res<Live> {
    conn.query_row(&format!("{} WHERE l.id = ?1", live_select("")), params![id], map_live)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn create_live(conn: &Connection, user: &User, r: &LiveReq) -> Res<Live> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    require_assignment(conn, &user.id, subject_id)?;
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let starts_at = r.starts_at.ok_or_else(|| bad("invalid_time"))?;
    let duration = r.duration_min.unwrap_or(60);
    check_live_time(starts_at, duration)?;
    let url = https_url(r.join_url.as_deref().unwrap_or(""))?.to_string();
    let id = new_id();
    conn.execute(
        "INSERT INTO live_sessions(id, teacher_id, subject_id, title, description, starts_at, duration_min, join_url, status, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'scheduled',?9)",
        params![id, user.id, subject_id, title, description, starts_at, duration, url, now_ms()],
    )
    .map_err(db_err)?;
    announce(conn, user, subject_id, "live_scheduled", &title, &format!("/platform/subjects/{subject_id}"));
    get_live(conn, &id)
}

fn update_live(conn: &Connection, user: &User, id: &str, r: &LiveReq) -> Res<Live> {
    let owner = can_manage(conn, "live_sessions", id, user)?;
    admin_only_status(user, &owner, r.title.is_some() || r.description.is_some() || r.starts_at.is_some() || r.duration_min.is_some() || r.join_url.is_some())?;
    let cur = get_live(conn, id)?;
    let (cur_status, cur_subject, cur_teacher, cur_starts_at, cur_url) = (cur.status.clone(), cur.subject_id.clone(), cur.teacher_name.clone(), cur.starts_at, cur.join_url.clone());
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title,
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { cur.description };
    let starts_at = r.starts_at.unwrap_or(cur.starts_at);
    let duration = r.duration_min.unwrap_or(cur.duration_min);
    // The editor sends the time back on every save: only a time that really moves is checked, so the title of a
    // session that is already over can still be corrected.
    if starts_at != cur.starts_at || duration != cur.duration_min {
        check_live_time(starts_at, duration)?;
    }
    let url = match &r.join_url {
        Some(u) => https_url(u)?.to_string(),
        None => cur.join_url,
    };
    let status = one_of(r.status.as_deref().unwrap_or(&cur.status), &["scheduled", "cancelled"], "invalid_status")?.to_string();
    // Only a TRANSITION to scheduled (re-activating a cancelled session) needs a live assignment: an item that is
    // already scheduled is not being published by this save, so its owner can still fix the title or the link of a
    // session that is over (or hidden) after the approval went away.
    if status == "scheduled" && cur_status != "scheduled" {
        require_assignment(conn, &owner, &cur_subject)?;
    }
    conn.execute(
        "UPDATE live_sessions SET title=?1, description=?2, starts_at=?3, duration_min=?4, join_url=?5, status=?6 WHERE id=?7",
        params![title, description, starts_at, duration, url, status, id],
    )
    .map_err(db_err)?;
    if user.id != owner {
        audit(conn, &user.id, id, "live_moderated", &status);
        if status == "cancelled" && cur_status == "scheduled" {
            tell_owner_moderated(conn, &owner, &title, &format!("/platform/subjects/{}", cur_subject));
        }
    }
    let teacher = owner_user(&owner, &cur_teacher);
    let link = format!("/platform/subjects/{cur_subject}");
    let over = starts_at + duration * 60_000 <= now_ms(); // nothing to tell about a session that is already over
    // The audience is told only about a session they can see (published by a teacher in good standing, subject and
    // institution active): the owner may still edit a hidden session after the approval went away, and that must not
    // become a way to broadcast to its students and followers. Checked after the update, against the new state.
    let shown = visible_to_public(conn, "live_sessions", "l", id, &status)?;
    if !shown {
        // nothing to announce
    } else if status == "cancelled" && cur_status == "scheduled" {
        announce(conn, &teacher, &cur_subject, "live_cancelled", &title, &link);
    } else if status == "scheduled" && cur_status == "cancelled" && !over {
        // back on: told like a new session (it is one again)
        announce(conn, &teacher, &cur_subject, "live_scheduled", &title, &link);
    } else if status == "scheduled" && cur_status == "scheduled" && !over && (starts_at != cur_starts_at || url != cur_url) {
        // The time or the link of a session students may be planning to join moved: once per save, to the same audience.
        // A new title or description alone is not worth a notice.
        crate::platform_engage::notify_audience(
            conn,
            &owner,
            &cur_subject,
            "live_updated",
            serde_json::json!({ "title": title, "teacher": teacher.full_name, "starts_at": starts_at }),
            &link,
        );
    }
    get_live(conn, id)
}

pub async fn create_live_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<LiveReq>) -> Res<(StatusCode, Json<Live>)> {
    let user = require_role(&state, &headers, "teacher")?;
    create_live(&*lock(&state)?, &user, &r).map(|l| (StatusCode::CREATED, Json(l)))
}

pub async fn update_live_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<LiveReq>,
) -> Res<Json<MyLive>> {
    let user = require_active(&state, &headers)?;
    update_my_live(&*lock(&state)?, &user, &id, &r, now_ms()).map(Json)
}

/// [`update_live`], answered in the owner's shape (`ended`, `visible`, `hidden_reason`) - the same as `GET /live/{id}` -
/// so an editor that stores the reply keeps showing whether the session is over or hidden after every save.
fn update_my_live(conn: &Connection, user: &User, id: &str, r: &LiveReq, now: i64) -> Res<MyLive> {
    update_live(conn, user, id, r)?;
    get_my_live(conn, user, id, now)
}

/// Deletes the session and the reports about it. All or nothing.
fn delete_live(conn: &Connection, user: &User, id: &str) -> Res<()> {
    can_manage(conn, "live_sessions", id, user)?;
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute("DELETE FROM live_sessions WHERE id = ?1", params![id]).map_err(db_err)?;
    crate::platform_ops::purge_target_references(&tx, "live", id)?;
    audit(&tx, &user.id, id, "live_deleted", "");
    tx.commit().map_err(db_err)?;
    Ok(())
}

pub async fn delete_live_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    delete_live(&*lock(&state)?, &user, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── bundles ─────────

#[derive(Serialize)]
pub struct Bundle {
    posts: Vec<Post>,
    courses: Vec<Course>,
    live: Vec<Live>,
}

fn bundle(conn: &Connection, scope: Scope) -> Res<Bundle> {
    Ok(Bundle { posts: list_posts(conn, scope)?, courses: list_courses(conn, scope)?, live: list_live(conn, scope, now_ms())? })
}

pub async fn subject_content_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<Bundle>> {
    require_active(&state, &headers)?;
    bundle(&*lock(&state)?, Scope::Subject(&id)).map(Json)
}

pub async fn teacher_content_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<Bundle>> {
    require_active(&state, &headers)?;
    bundle(&*lock(&state)?, Scope::Teacher(&id)).map(Json)
}

// ───────── the teacher's own content (the "My content" hub, phase 3-2) ─────────

const MY_DEFAULT_LIMIT: i64 = 50;
const MY_MAX_LIMIT: i64 = 100;
const MY_MAX_SEARCH_CHARS: usize = 100;

/// `GET /my/content` query. Every field is optional; blank values count as absent.
#[derive(Deserialize, Default, Debug)]
pub struct MyQuery {
    /// Title substring (case-insensitive; `%`, `_` and `\` are literal).
    q: Option<String>,
    /// `draft` | `published` | `hidden` (published/scheduled but not shown to students).
    status: Option<String>,
    subject_id: Option<String>,
    /// `post` | `course` | `live`: only that list is filled.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// `updated` (default: latest change first; live sessions by start) | `title` (A-Z). Applied BEFORE paging, so every page
    /// continues the same order instead of reordering only the rows that happen to be on it.
    sort: Option<String>,
    /// Per type, default 50, at most 100.
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MySort {
    Updated,
    Title,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MyStatus {
    Draft,
    Published,
    Hidden,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MyKind {
    Post,
    Course,
    Live,
}

#[derive(Debug)]
struct MyFilter {
    like: Option<String>,
    status: Option<MyStatus>,
    subject_id: Option<String>,
    kind: Option<MyKind>,
    sort: MySort,
    limit: i64,
    offset: i64,
}

impl MyFilter {
    fn parse(q: &MyQuery) -> Res<Self> {
        let blank = |o: &Option<String>| o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let search = blank(&q.q);
        if search.as_deref().map_or(false, |s| s.chars().count() > MY_MAX_SEARCH_CHARS) {
            return Err(bad("invalid_query"));
        }
        let status = match blank(&q.status).as_deref() {
            None => None,
            Some("draft") => Some(MyStatus::Draft),
            Some("published") => Some(MyStatus::Published),
            Some("hidden") => Some(MyStatus::Hidden),
            Some(_) => return Err(bad("invalid_status")),
        };
        let kind = match blank(&q.kind).as_deref() {
            None => None,
            Some("post") => Some(MyKind::Post),
            Some("course") => Some(MyKind::Course),
            Some("live") => Some(MyKind::Live),
            Some(_) => return Err(bad("invalid_type")),
        };
        let sort = match blank(&q.sort).as_deref() {
            None | Some("updated") => MySort::Updated,
            Some("title") => MySort::Title,
            Some(_) => return Err(bad("invalid_sort")),
        };
        Ok(MyFilter {
            like: search.as_deref().map(like_pattern),
            status,
            subject_id: blank(&q.subject_id),
            kind,
            sort,
            limit: q.limit.unwrap_or(MY_DEFAULT_LIMIT).clamp(1, MY_MAX_LIMIT),
            offset: q.offset.unwrap_or(0).max(0),
        })
    }

    fn wants(&self, kind: MyKind) -> bool {
        self.kind.map_or(true, |k| k == kind)
    }
}

/// A post or course as its owner sees it: the usual fields plus whether students see it right now.
#[derive(Serialize, Debug)]
pub struct MyItem<T> {
    #[serde(flatten)]
    item: T,
    visible: bool,
    hidden_reason: Option<HiddenReason>,
}

/// A live session as its owner sees it; `ended` = its time is over (it is then no longer listed to students).
#[derive(Serialize, Debug)]
pub struct MyLive {
    #[serde(flatten)]
    live: Live,
    ended: bool,
    visible: bool,
    hidden_reason: Option<HiddenReason>,
}

/// How many items match the filters (before `limit`/`offset`); a type excluded by `type` counts 0.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct MyTotals {
    posts: i64,
    courses: i64,
    live: i64,
}

#[derive(Serialize, Debug)]
pub struct MyContent {
    posts: Vec<MyItem<Post>>,
    courses: Vec<MyItem<Course>>,
    live: Vec<MyLive>,
    totals: MyTotals,
}

fn live_ended(l: &Live, now: i64) -> bool {
    l.starts_at + l.duration_min * 60_000 <= now
}

fn my_live(live: Live, reason: Option<String>, now: i64) -> MyLive {
    let ended = live_ended(&live, now);
    let (visible, hidden_reason) = presence(live.status == "scheduled" && !ended, reason);
    MyLive { live, ended, visible, hidden_reason }
}

/// The status filter as SQL for the alias `a` of `kind`. Live sessions have no draft: a cancelled session is the
/// unpublished state, a scheduled one the published state. `hidden` = in the published state (and, for a session,
/// not over) but not shown to students.
fn my_status_sql(kind: MyKind, a: &str, status: MyStatus, now_param: usize) -> String {
    let (draft, published) = if kind == MyKind::Live { ("cancelled", "scheduled") } else { ("draft", "published") };
    match status {
        MyStatus::Draft => format!("{a}.status = '{draft}'"),
        MyStatus::Published => format!("{a}.status = '{published}'"),
        MyStatus::Hidden => {
            let running = if kind == MyKind::Live { format!(" AND {a}.starts_at + {a}.duration_min * 60000 > ?{now_param}") } else { String::new() };
            format!("{a}.status = '{published}'{running} AND ({}) IS NOT NULL", hidden_reason_sql(a))
        }
    }
}

/// One page of the caller's items of one kind, each with the SQL hidden-reason column, plus the total count.
/// Ownership is always `a.teacher_id = ?1` with the id of the signed-in teacher (`Scope::Mine`).
fn my_page<T>(
    conn: &Connection,
    kind: MyKind,
    teacher_id: &str,
    f: &MyFilter,
    now: i64,
    map: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Res<(Vec<(T, Option<String>)>, i64)> {
    use rusqlite::types::Value;
    // alias, column list, joins, ordering, and the index of the reason column (= number of listed columns)
    let (a, cols, from, order, reason_col) = match kind {
        MyKind::Post => ("p", POST_COLS, POST_FROM, "p.updated_at DESC, p.id", 14),
        MyKind::Course => ("c", COURSE_COLS, COURSE_FROM, "c.updated_at DESC, c.id", 10),
        MyKind::Live => ("l", LIVE_COLS, LIVE_FROM, "l.starts_at DESC, l.id", 11),
    };
    let mut args: Vec<Value> = vec![teacher_id.to_string().into()];
    let mut clauses = vec![scope_clause(Scope::Mine(teacher_id), a, "published")]; // `a.teacher_id = ?1`
    if let Some(like) = &f.like {
        args.push(like.clone().into());
        clauses.push(format!("lower({a}.title) LIKE ?{} ESCAPE '\\'", args.len()));
    }
    if let Some(subject) = &f.subject_id {
        args.push(subject.clone().into());
        clauses.push(format!("{a}.subject_id = ?{}", args.len()));
    }
    if let Some(status) = f.status {
        // a live session's "hidden" needs the clock; the other kinds never mention the parameter
        let now_param = if kind == MyKind::Live && status == MyStatus::Hidden {
            args.push(now.into());
            args.len()
        } else {
            0
        };
        clauses.push(my_status_sql(kind, a, status, now_param));
    }
    let where_sql = clauses.into_iter().map(|c| format!("({c})")).collect::<Vec<_>>().join(" AND ");
    let total: i64 = conn
        .query_row(&format!("SELECT count(*) {from} WHERE {where_sql}"), rusqlite::params_from_iter(args.iter()), |r| r.get(0))
        .map_err(db_err)?;
    let (limit_at, offset_at) = (args.len() + 1, args.len() + 2);
    args.push(f.limit.into());
    args.push(f.offset.into());
    // A-Z by title: `lower` is ASCII-only in SQLite, which is all a case fold needs (Arabic has no case); the id breaks ties
    let order = if f.sort == MySort::Title { format!("lower({a}.title), {a}.id") } else { order.to_string() };
    let sql = format!("SELECT {cols}, ({}) {from} WHERE {where_sql} ORDER BY {order} LIMIT ?{limit_at} OFFSET ?{offset_at}", hidden_reason_sql(a));
    let rows = conn
        .prepare(&sql)
        .map_err(db_err)?
        .query_map(rusqlite::params_from_iter(args.iter()), |r| Ok((map(r)?, r.get::<_, Option<String>>(reason_col)?)))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok((rows, total))
}

/// The caller's own posts, courses and live sessions, filtered and paged per type. `teacher_id` comes from the
/// session; nothing in the filter can widen the scope to somebody else's rows.
fn my_content(conn: &Connection, teacher_id: &str, f: &MyFilter, now: i64) -> Res<MyContent> {
    let mut out = MyContent { posts: vec![], courses: vec![], live: vec![], totals: MyTotals::default() };
    if f.wants(MyKind::Post) {
        let (rows, total) = my_page(conn, MyKind::Post, teacher_id, f, now, map_post(false))?;
        out.posts = rows
            .into_iter()
            .map(|(item, reason)| {
                let (visible, hidden_reason) = presence(item.status == "published", reason);
                MyItem { item, visible, hidden_reason }
            })
            .collect();
        out.totals.posts = total;
    }
    if f.wants(MyKind::Course) {
        let (rows, total) = my_page(conn, MyKind::Course, teacher_id, f, now, map_course)?;
        out.courses = rows
            .into_iter()
            .map(|(item, reason)| {
                let (visible, hidden_reason) = presence(item.status == "published", reason);
                MyItem { item, visible, hidden_reason }
            })
            .collect();
        out.totals.courses = total;
    }
    if f.wants(MyKind::Live) {
        let (rows, total) = my_page(conn, MyKind::Live, teacher_id, f, now, map_live)?;
        out.live = rows.into_iter().map(|(live, reason)| my_live(live, reason, now)).collect();
        out.totals.live = total;
    }
    Ok(out)
}

pub async fn my_content_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<MyQuery>) -> Res<Json<MyContent>> {
    let user = require_role(&state, &headers, "teacher")?;
    let filter = MyFilter::parse(&q)?;
    my_content(&*lock(&state)?, &user.id, &filter, now_ms()).map(Json)
}

/// One live session for its owner (or an admin), in the same shape as an item of `GET /my/content`.
fn get_my_live(conn: &Connection, user: &User, id: &str, now: i64) -> Res<MyLive> {
    can_manage(conn, "live_sessions", id, user)?;
    let (live, reason) = conn
        .query_row(&format!("{} WHERE l.id = ?1", live_select(&format!(", ({})", hidden_reason_sql("l")))), params![id], |r| {
            Ok((map_live(r)?, r.get::<_, Option<String>>(11)?))
        })
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    Ok(my_live(live, reason, now))
}

pub async fn get_live_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<Json<MyLive>> {
    let user = require_active(&state, &headers)?;
    get_my_live(&*lock(&state)?, &user, &id, now_ms()).map(Json)
}

/// Student home feed: content/live from enrolled subjects and followed teachers.
pub async fn feed_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Bundle>> {
    let user = require_role(&state, &headers, "student")?;
    let mut b = bundle(&*lock(&state)?, Scope::Feed(&user.id))?;
    b.posts.truncate(10);
    b.courses.truncate(10);
    Ok(Json(b))
}

/// Router layer for the upload route (default body limit is 2 MB).
pub fn upload_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MAX_FILE_BYTES + 512 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    struct World {
        conn: Connection,
        teacher: User,
        student: User,
        admin: User,
        subject: String,
    }

    fn world() -> World {
        let conn = create_test_db();
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','ج',1,0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0)", []).unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO subject_enrollments VALUES (?1,'s1',0)", params![student.id]).unwrap();
        World { conn, teacher, student, admin, subject: "s1".into() }
    }

    fn post_req(w: &World, status: &str) -> PostReq {
        PostReq { subject_id: Some(w.subject.clone()), kind: Some("summary".into()), title: Some("ملخص".into()), body: Some("نص".into()), status: Some(status.into()) }
    }

    #[test]
    fn video_urls_are_classified_and_sanitized() {
        let y = |u: &str| parse_video(u).unwrap();
        assert_eq!(y("https://www.youtube.com/watch?v=dQw4w9WgXcQ").1.as_deref(), Some("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"));
        assert_eq!(y("https://youtu.be/dQw4w9WgXcQ?t=5").0, "youtube");
        assert_eq!(y("https://youtube.com/embed/dQw4w9WgXcQ").0, "youtube");
        assert_eq!(y("https://vimeo.com/123456").1.as_deref(), Some("https://player.vimeo.com/video/123456"));
        assert_eq!(y("https://cdn.example.com/v/lesson.MP4").0, "file");
        assert_eq!(y("https://example.com/page"), ("link", None));
        assert_eq!(y("https://www.youtube.com/watch?v=bad\"onload"), ("link", None)); // bad id never embedded
        for bad_url in ["http://youtu.be/dQw4w9WgXcQ", "javascript:alert(1)", "https://u:p@example.com/", "https://exa mple.com", "", "ftp://x.com/a.mp4"] {
            assert!(parse_video(bad_url).is_err(), "{bad_url}");
        }
    }

    #[test]
    fn creating_content_requires_approved_assignment() {
        let w = world();
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        assert_eq!(create_post(&w.conn, &other, &post_req(&w, "draft")).unwrap_err().0, StatusCode::FORBIDDEN);
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','pending',0,NULL)", params![other.id]).unwrap();
        assert_eq!(create_post(&w.conn, &other, &post_req(&w, "draft")).unwrap_err().0, StatusCode::FORBIDDEN, "pending is not enough");
        assert!(create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).is_ok());
    }

    #[test]
    fn visibility_follows_status_and_assignment() {
        let w = world();
        let draft = create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap();
        let live_post = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        let public = || list_posts(&w.conn, Scope::Subject("s1")).unwrap().len();
        assert_eq!(public(), 1, "drafts hidden");
        assert!(!visible_to_public(&w.conn, "posts", "p", &draft.id, "published").unwrap());
        assert_eq!(list_posts(&w.conn, Scope::Mine(&w.teacher.id)).unwrap().len(), 2);
        assert_eq!(list_posts(&w.conn, Scope::Feed(&w.student.id)).unwrap().len(), 1, "enrolled student sees it in feed");
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected'", []).unwrap();
        assert_eq!(public(), 0, "revoking the assignment hides content");
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved'", []).unwrap();
        w.conn.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![w.teacher.id]).unwrap();
        assert_eq!(public(), 0, "suspended teacher's content hidden");
        assert!(!visible_to_public(&w.conn, "posts", "p", &live_post.id, "published").unwrap());
    }

    #[test]
    fn only_owner_edits_and_admin_only_moderates_status() {
        let w = world();
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        let edit = |title: &str| PostReq { subject_id: None, kind: None, title: Some(title.into()), body: None, status: None };
        assert_eq!(update_post(&w.conn, &w.student, &p.id, &edit("x")).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(update_post(&w.conn, &w.admin, &p.id, &edit("x")).unwrap_err().0, StatusCode::FORBIDDEN, "admin cannot rewrite");
        let unpublish = PostReq { subject_id: None, kind: None, title: None, body: None, status: Some("draft".into()) };
        assert_eq!(update_post(&w.conn, &w.admin, &p.id, &unpublish).unwrap().status, "draft");
        assert_eq!(list_posts(&w.conn, Scope::Subject("s1")).unwrap().len(), 0);
        assert_eq!(update_post(&w.conn, &w.teacher, &p.id, &edit("جديد")).unwrap().title, "جديد");
        let audit_n: i64 = w.conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'post_moderated'", [], |r| r.get(0)).unwrap();
        assert_eq!(audit_n, 1);
    }

    #[test]
    fn upload_validation() {
        assert!(validate_upload("a.pdf", b"%PDF-1.4 x").is_ok());
        assert_eq!(validate_upload("a.pdf", b"<html>").unwrap_err().0, StatusCode::BAD_REQUEST, "magic mismatch");
        assert_eq!(validate_upload("evil.html", b"<html>").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("evil.svg", b"<svg>").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("a.png", b"").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("a.pdf", &vec![b'%'; MAX_FILE_BYTES + 1]).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(validate_upload("../../etc/pass wd.docx", b"PK\x03\x04..").unwrap().1, "pass wd.docx", "path stripped");
        assert!(validate_upload("noext", b"x").is_err());
    }

    #[test]
    fn course_lessons_order_and_move() {
        let w = world();
        // a course starts as a draft: publishing one without lessons is refused (`course_needs_lessons`, phase 3-2)
        let c = create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("دورة".into()), description: None, status: Some("draft".into()) }).unwrap();
        let add = |t: &str| add_lesson(&w.conn, &w.teacher, &c.course.id, &LessonReq { title: Some(t.into()), description: None, section: None, video_url: Some("https://youtu.be/dQw4w9WgXcQ".into()) }).unwrap();
        let (a, b, cc) = (add("أ"), add("ب"), add("ج"));
        let titles = || get_course(&w.conn, &c.course.id).unwrap().lessons.iter().map(|l| l.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(), ["أ", "ب", "ج"]);
        move_lesson(&w.conn, &w.teacher, &cc.id, true).unwrap();
        assert_eq!(titles(), ["أ", "ج", "ب"]);
        move_lesson(&w.conn, &w.teacher, &a.id, true).unwrap(); // already first: no-op
        move_lesson(&w.conn, &w.teacher, &b.id, false).unwrap(); // already last: no-op
        assert_eq!(titles(), ["أ", "ج", "ب"]);
        assert_eq!(add_lesson(&w.conn, &w.teacher, &c.course.id, &LessonReq { title: Some("x".into()), description: None, section: None, video_url: Some("http://x.com/a.mp4".into()) }).unwrap_err().0, StatusCode::BAD_REQUEST);
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        assert_eq!(move_lesson(&w.conn, &other, &a.id, false).unwrap_err().0, StatusCode::FORBIDDEN);
        // now that it has lessons it can be published, and then it is listed with all three
        let publish = CourseReq { subject_id: None, title: None, description: None, status: Some("published".into()) };
        update_course(&w.conn, &w.teacher, &c.course.id, &publish).unwrap();
        assert_eq!(list_courses(&w.conn, Scope::Subject("s1")).unwrap()[0].lesson_count, 3);
        delete_lesson(&w.conn, &w.teacher, &b.id).unwrap();
        assert_eq!(titles(), ["أ", "ج"]);
    }

    #[test]
    fn live_sessions_validate_and_expire_from_listing() {
        let w = world();
        let now = now_ms();
        let req = |starts: i64, dur: i64, url: &str| LiveReq { subject_id: Some("s1".into()), title: Some("درس".into()), description: None, starts_at: Some(starts), duration_min: Some(dur), join_url: Some(url.into()), status: None };
        assert_eq!(create_live(&w.conn, &w.teacher, &req(now + 1000, 60, "http://meet.example.com/x")).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create_live(&w.conn, &w.teacher, &req(now + 1000, 2, "https://meet.example.com/x")).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create_live(&w.conn, &w.teacher, &req(now - 10 * 86_400_000, 60, "https://meet.example.com/x")).unwrap_err().0, StatusCode::BAD_REQUEST);
        let l = create_live(&w.conn, &w.teacher, &req(now + 3_600_000, 60, "https://meet.example.com/x")).unwrap();
        assert_eq!(list_live(&w.conn, Scope::Subject("s1"), now).unwrap().len(), 1);
        assert_eq!(list_live(&w.conn, Scope::Subject("s1"), now + 3 * 3_600_000).unwrap().len(), 0, "ended sessions drop off");
        let cancel = LiveReq { subject_id: None, title: None, description: None, starts_at: None, duration_min: None, join_url: None, status: Some("cancelled".into()) };
        update_live(&w.conn, &w.admin, &l.id, &cancel).unwrap();
        assert_eq!(list_live(&w.conn, Scope::Subject("s1"), now).unwrap().len(), 0);
        assert_eq!(list_live(&w.conn, Scope::Mine(&w.teacher.id), now).unwrap().len(), 1);
    }

    #[test]
    fn deleting_post_cascades_and_orphan_files_purged() {
        let w = world();
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        w.conn.execute("INSERT INTO files VALUES ('f1', ?1, 'a.pdf', 'application/pdf', 4, 0)", params![w.teacher.id]).unwrap();
        w.conn.execute("UPDATE posts SET file_id = 'f1' WHERE id = ?1", params![p.id]).unwrap();
        assert_eq!(get_post(&w.conn, &p.id).unwrap().file.unwrap().name, "a.pdf");
        w.conn.execute("DELETE FROM subjects WHERE id = 's1'", []).unwrap();
        let n: i64 = w.conn.query_row("SELECT count(*) FROM posts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "posts cascade with the subject");
        let mut state = crate::platform::PlatformState::for_tests(w.conn);
        state.files_dir = std::env::temp_dir();
        purge_orphan_files(&state);
        let files: i64 = state.conn.lock().unwrap().query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap();
        assert_eq!(files, 0);
    }

    // ───── phase 3-2: hidden institutions, the owner's view, the editors' rules ─────

    fn raw_post(conn: &Connection, id: &str, teacher: &str, subject: &str, status: &str, title: &str, at: i64) {
        conn.execute(
            "INSERT INTO posts(id, teacher_id, subject_id, kind, title, body, status, created_at, updated_at) VALUES (?1,?2,?3,'article',?4,'نص',?5,?6,?6)",
            params![id, teacher, subject, title, status, at],
        )
        .unwrap();
    }

    fn raw_course(conn: &Connection, id: &str, teacher: &str, subject: &str, status: &str, title: &str, at: i64) {
        conn.execute(
            "INSERT INTO courses(id, teacher_id, subject_id, title, description, status, created_at, updated_at) VALUES (?1,?2,?3,?4,NULL,?5,?6,?6)",
            params![id, teacher, subject, title, status, at],
        )
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn raw_live(conn: &Connection, id: &str, teacher: &str, subject: &str, status: &str, title: &str, starts_at: i64, duration: i64) {
        conn.execute(
            "INSERT INTO live_sessions(id, teacher_id, subject_id, title, description, starts_at, duration_min, join_url, status, created_at)
             VALUES (?1,?2,?3,?4,NULL,?5,?6,'https://meet.example.com/x',?7,?5)",
            params![id, teacher, subject, title, starts_at, duration, status],
        )
        .unwrap();
    }

    fn add_subject(conn: &Connection, id: &str, institution: &str) {
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES (?1,?2,?3,1,0)", params![id, institution, format!("مادة {id}")]).unwrap();
    }

    fn approve(conn: &Connection, teacher: &User, subject: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,?2,'approved',0,0)",
            params![teacher.id, subject],
        )
        .unwrap();
    }

    fn mine(w: &World, f: impl FnOnce(&mut MyQuery)) -> MyContent {
        let mut q = MyQuery::default();
        f(&mut q);
        my_content(&w.conn, &w.teacher.id, &MyFilter::parse(&q).unwrap(), now_ms()).unwrap()
    }

    fn titles<T>(items: &[MyItem<T>], title: impl Fn(&T) -> &str) -> Vec<String> {
        items.iter().map(|i| title(&i.item).to_string()).collect()
    }

    fn code_of(e: &crate::relay::Err) -> String {
        e.1.clone()
    }

    #[test]
    fn a_hidden_institution_hides_everything_under_it_and_blocks_creating_and_publishing() {
        let w = world();
        let post = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        let draft_post = create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap();
        let course = create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("دورة".into()), description: None, status: None }).unwrap();
        add_lesson(&w.conn, &w.teacher, &course.course.id, &LessonReq { title: Some("د".into()), description: None, section: None, video_url: Some("https://youtu.be/dQw4w9WgXcQ".into()) }).unwrap();
        update_course(&w.conn, &w.teacher, &course.course.id, &CourseReq { subject_id: None, title: None, description: None, status: Some("published".into()) }).unwrap();
        let now = now_ms();
        let live = create_live(
            &w.conn,
            &w.teacher,
            &LiveReq { subject_id: Some("s1".into()), title: Some("بث".into()), description: None, starts_at: Some(now + 3_600_000), duration_min: Some(60), join_url: Some("https://meet.example.com/x".into()), status: None },
        )
        .unwrap();
        let served = |scope: Scope| (list_posts(&w.conn, scope).unwrap().len(), list_courses(&w.conn, scope).unwrap().len(), list_live(&w.conn, scope, now).unwrap().len());
        for scope in [Scope::Subject("s1"), Scope::Teacher(&w.teacher.id), Scope::Feed(&w.student.id)] {
            assert_eq!(served(scope), (1, 1, 1), "visible while the institution is on");
        }

        w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        for scope in [Scope::Subject("s1"), Scope::Teacher(&w.teacher.id), Scope::Feed(&w.student.id)] {
            assert_eq!(served(scope), (0, 0, 0), "the subject is on, the institution is off: nothing is served");
        }
        assert!(!visible_to_public(&w.conn, "posts", "p", &post.id, "published").unwrap());
        assert!(!visible_to_public(&w.conn, "courses", "c", &course.course.id, "published").unwrap());
        assert!(!visible_to_public(&w.conn, "live_sessions", "l", &live.id, "scheduled").unwrap());
        assert_eq!(served(Scope::Mine(&w.teacher.id)), (2, 1, 1), "the owner still has all of it");

        // nothing can be created or switched on in it
        let forbidden = |e: crate::relay::Err| (e.0, code_of(&e).contains("not_assigned"));
        assert_eq!(forbidden(create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap_err()), (StatusCode::FORBIDDEN, true));
        assert_eq!(
            forbidden(create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("x".into()), description: None, status: None }).unwrap_err()),
            (StatusCode::FORBIDDEN, true)
        );
        let req = LiveReq { subject_id: Some("s1".into()), title: Some("x".into()), description: None, starts_at: Some(now + 7_200_000), duration_min: Some(30), join_url: Some("https://m.example.com/y".into()), status: None };
        assert_eq!(forbidden(create_live(&w.conn, &w.teacher, &req).unwrap_err()), (StatusCode::FORBIDDEN, true));
        let publish = PostReq { subject_id: None, kind: None, title: None, body: None, status: Some("published".into()) };
        assert_eq!(forbidden(update_post(&w.conn, &w.teacher, &draft_post.id, &publish).unwrap_err()), (StatusCode::FORBIDDEN, true));
        assert_eq!(forbidden(duplicate_post(&w.conn, &w.teacher, &post.id, &DuplicateReq::default()).unwrap_err()), (StatusCode::FORBIDDEN, true));
        assert!(!can_publish(&w.conn, &w.teacher.id, "s1").unwrap());

        // switching it back on restores everything, and publishing works again
        w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        assert_eq!(served(Scope::Subject("s1")), (1, 1, 1));
        assert!(update_post(&w.conn, &w.teacher, &draft_post.id, &publish).is_ok());
    }

    #[test]
    fn hidden_reason_and_visible_agree_over_the_whole_standing_matrix() {
        let w = world();
        let now = now_ms();
        raw_post(&w.conn, "p1", &w.teacher.id, "s1", "published", "منشور", now);
        raw_course(&w.conn, "c1", &w.teacher.id, "s1", "published", "دورة", now);
        raw_live(&w.conn, "l1", &w.teacher.id, "s1", "scheduled", "بث", now + 3_600_000, 60);
        let accounts = [("active", "teacher"), ("pending", "teacher"), ("suspended", "teacher"), ("rejected", "teacher"), ("active", "student"), ("active", "admin")];
        let assignments = [None, Some("pending"), Some("rejected"), Some("approved")];
        let mut cases = 0;
        for (status, role) in accounts {
            for assignment in assignments {
                for subject_on in [true, false] {
                    for institution_on in [true, false] {
                        w.conn.execute("UPDATE users SET status = ?1, role = ?2 WHERE id = ?3", params![status, role, w.teacher.id]).unwrap();
                        w.conn.execute("DELETE FROM teacher_subjects WHERE teacher_id = ?1", params![w.teacher.id]).unwrap();
                        if let Some(a) = assignment {
                            w.conn
                                .execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s1',?2,0)", params![w.teacher.id, a])
                                .unwrap();
                        }
                        w.conn.execute("UPDATE subjects SET is_active = ?1 WHERE id = 's1'", params![subject_on]).unwrap();
                        w.conn.execute("UPDATE institutions SET is_active = ?1 WHERE id = 'i1'", params![institution_on]).unwrap();

                        // the root cause wins: account, then institution, then subject, then assignment
                        let expected = if !(status == "active" && role == "teacher") {
                            Some(HiddenReason::AccountInactive)
                        } else if !institution_on {
                            Some(HiddenReason::InstitutionInactive)
                        } else if !subject_on {
                            Some(HiddenReason::SubjectInactive)
                        } else if assignment != Some("approved") {
                            Some(HiddenReason::NoAssignment)
                        } else {
                            None
                        };
                        let case = format!("{status}/{role} assignment={assignment:?} subject_on={subject_on} institution_on={institution_on}");

                        // what students are actually served (the visible() predicate) ...
                        let served = (
                            list_posts(&w.conn, Scope::Subject("s1")).unwrap().len(),
                            list_courses(&w.conn, Scope::Subject("s1")).unwrap().len(),
                            list_live(&w.conn, Scope::Subject("s1"), now).unwrap().len(),
                        );
                        assert_eq!(served, if expected.is_none() { (1, 1, 1) } else { (0, 0, 0) }, "{case}");

                        // ... and what the owner is told about the very same rows
                        let c = my_content(&w.conn, &w.teacher.id, &MyFilter::parse(&MyQuery::default()).unwrap(), now).unwrap();
                        let seen = [
                            (c.posts[0].visible, c.posts[0].hidden_reason),
                            (c.courses[0].visible, c.courses[0].hidden_reason),
                            (c.live[0].visible, c.live[0].hidden_reason),
                        ];
                        for (visible, reason) in seen {
                            assert_eq!((visible, reason), (expected.is_none(), expected), "{case}");
                        }
                        // the `hidden` tab is exactly the published rows that have a reason
                        let hidden = mine(&w, |q| q.status = Some("hidden".into()));
                        assert_eq!((hidden.posts.len(), hidden.courses.len(), hidden.live.len()), if expected.is_some() { (1, 1, 1) } else { (0, 0, 0) }, "{case}");
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 6 * 4 * 2 * 2);
        // wire values
        w.conn.execute("UPDATE users SET status = 'active', role = 'teacher' WHERE id = ?1", params![w.teacher.id]).unwrap();
        w.conn.execute("UPDATE subjects SET is_active = 1", []).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        let json = serde_json::to_value(mine(&w, |_| {}).posts).unwrap();
        assert_eq!(json[0]["hidden_reason"], "institution_inactive");
        assert_eq!(json[0]["visible"], false);
    }

    #[test]
    fn a_draft_or_finished_item_is_not_hidden_it_is_just_not_published() {
        let w = world();
        let now = now_ms();
        raw_post(&w.conn, "p1", &w.teacher.id, "s1", "draft", "مسودة", now);
        raw_live(&w.conn, "l1", &w.teacher.id, "s1", "scheduled", "منتهي", now - 5 * 3_600_000, 60);
        raw_live(&w.conn, "l2", &w.teacher.id, "s1", "cancelled", "ملغى", now + 3_600_000, 60);
        // even when the subject is switched off there is nothing to report for them
        w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
        let c = mine(&w, |_| {});
        assert_eq!((c.posts[0].visible, c.posts[0].hidden_reason), (false, None));
        let ended = c.live.iter().find(|l| l.live.id == "l1").unwrap();
        assert_eq!((ended.ended, ended.visible, ended.hidden_reason), (true, false, None), "over is not hidden");
        let cancelled = c.live.iter().find(|l| l.live.id == "l2").unwrap();
        assert_eq!((cancelled.ended, cancelled.visible, cancelled.hidden_reason), (false, false, None));
        let hidden = mine(&w, |q| q.status = Some("hidden".into()));
        assert_eq!((hidden.posts.len(), hidden.live.len(), hidden.totals.posts, hidden.totals.live), (0, 0, 0, 0), "none of them is in the hidden tab");
    }

    #[test]
    fn my_content_filters_pages_counts_and_only_ever_shows_the_callers_own_rows() {
        let w = world();
        let now = now_ms();
        add_subject(&w.conn, "s2", "i1");
        approve(&w.conn, &w.teacher, "s2");
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        approve(&w.conn, &other, "s1");
        approve(&w.conn, &other, "s2");
        let t = &w.teacher.id;
        raw_post(&w.conn, "p1", t, "s1", "published", "Alpha one", now - 1000);
        raw_post(&w.conn, "p2", t, "s1", "draft", "alpha two", now - 2000);
        raw_post(&w.conn, "p3", t, "s2", "published", "Beta", now - 3000);
        raw_post(&w.conn, "p4", t, "s1", "draft", "100% done", now - 4000);
        raw_post(&w.conn, "p5", t, "s1", "draft", "a_b", now - 5000);
        raw_course(&w.conn, "c1", t, "s1", "published", "Alpha course", now - 1000);
        raw_course(&w.conn, "c2", t, "s2", "draft", "Gamma", now - 2000);
        raw_live(&w.conn, "l1", t, "s1", "scheduled", "Alpha live", now + 3_600_000, 60);
        raw_live(&w.conn, "l2", t, "s1", "scheduled", "Delta", now - 5 * 3_600_000, 60);
        raw_live(&w.conn, "l3", t, "s2", "cancelled", "Eps", now + 7_200_000, 60);
        // somebody else's rows that every filter below would match if the scope ever leaked
        raw_post(&w.conn, "o1", &other.id, "s1", "published", "Alpha other", now);
        raw_post(&w.conn, "o2", &other.id, "s2", "draft", "100% other", now);
        raw_course(&w.conn, "o3", &other.id, "s1", "published", "Alpha other course", now);
        raw_live(&w.conn, "o4", &other.id, "s1", "scheduled", "Alpha other live", now + 3_600_000, 60);

        let all = mine(&w, |_| {});
        assert_eq!((all.posts.len(), all.courses.len(), all.live.len()), (5, 2, 3));
        assert_eq!((all.totals.posts, all.totals.courses, all.totals.live), (5, 2, 3));
        let owners: Vec<&str> = all.posts.iter().map(|p| p.item.teacher_id.as_str()).chain(all.courses.iter().map(|c| c.item.teacher_id.as_str())).chain(all.live.iter().map(|l| l.live.teacher_id.as_str())).collect();
        assert!(owners.iter().all(|o| *o == t.as_str()), "another teacher's row never appears: {owners:?}");
        assert_eq!(titles(&all.posts, |p| &p.title), ["Alpha one", "alpha two", "Beta", "100% done", "a_b"], "newest update first");

        // title search: substring, any case; % _ and \ are literal
        let alpha = mine(&w, |q| q.q = Some("ALPHA".into()));
        assert_eq!(titles(&alpha.posts, |p| &p.title), ["Alpha one", "alpha two"]);
        assert_eq!((alpha.totals.posts, alpha.totals.courses, alpha.totals.live), (2, 1, 1));
        assert_eq!(mine(&w, |q| q.q = Some("100%".into())).posts.len(), 1);
        for (needle, expect) in [("%", "100% done"), ("_", "a_b")] {
            let r = mine(&w, |q| q.q = Some(needle.into()));
            assert_eq!(titles(&r.posts, |p| &p.title), [expect], "{needle:?} is a literal");
        }
        assert_eq!(mine(&w, |q| q.q = Some("\\".into())).totals.posts, 0);
        assert_eq!(mine(&w, |q| q.q = Some("  ".into())).totals.posts, 5, "blank search = no search");

        // status
        let draft = mine(&w, |q| q.status = Some("draft".into()));
        assert_eq!((draft.posts.len(), draft.courses.len(), draft.live.len()), (3, 1, 1), "a cancelled live session is the unpublished state");
        let published = mine(&w, |q| q.status = Some("published".into()));
        assert_eq!((published.posts.len(), published.courses.len(), published.live.len()), (2, 1, 2));
        assert_eq!(mine(&w, |q| q.status = Some("hidden".into())).totals, MyTotals::default(), "nothing hidden while everything stands");
        w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's2'", []).unwrap();
        let hidden = mine(&w, |q| q.status = Some("hidden".into()));
        assert_eq!(titles(&hidden.posts, |p| &p.title), ["Beta"]);
        assert_eq!((hidden.posts[0].visible, hidden.posts[0].hidden_reason), (false, Some(HiddenReason::SubjectInactive)));
        assert_eq!((hidden.courses.len(), hidden.live.len()), (0, 0), "the draft course and the cancelled session are not 'hidden'");

        // subject and type
        let s2 = mine(&w, |q| q.subject_id = Some("s2".into()));
        assert_eq!((s2.posts.len(), s2.courses.len(), s2.live.len()), (1, 1, 1));
        assert_eq!(mine(&w, |q| q.subject_id = Some("s1' OR '1'='1".into())).totals, MyTotals::default(), "bound, never spliced");
        let courses_only = mine(&w, |q| q.kind = Some("course".into()));
        assert_eq!((courses_only.posts.len(), courses_only.courses.len(), courses_only.live.len()), (0, 2, 0));
        assert_eq!((courses_only.totals.posts, courses_only.totals.courses, courses_only.totals.live), (0, 2, 0), "an excluded type counts 0");
        assert_eq!(mine(&w, |q| q.kind = Some("live".into())).live.len(), 3);

        // paging: the total ignores limit/offset
        let page = mine(&w, |q| {
            q.limit = Some(2);
            q.offset = Some(0);
        });
        assert_eq!((page.posts.len(), page.totals.posts), (2, 5));
        let page2 = mine(&w, |q| {
            q.limit = Some(2);
            q.offset = Some(4);
        });
        assert_eq!(titles(&page2.posts, |p| &p.title), ["a_b"]);
        assert_eq!(mine(&w, |q| q.offset = Some(99)).posts.len(), 0);
        assert_eq!(MyFilter::parse(&MyQuery::default()).unwrap().limit, 50, "default");
        assert_eq!(MyFilter::parse(&MyQuery { limit: Some(1000), ..Default::default() }).unwrap().limit, 100, "max");
        assert_eq!(MyFilter::parse(&MyQuery { limit: Some(0), ..Default::default() }).unwrap().limit, 1);
        assert_eq!(MyFilter::parse(&MyQuery { offset: Some(-5), ..Default::default() }).unwrap().offset, 0);

        // sorting by title orders the WHOLE set before it is paged: page two continues where page one stopped
        let by_title = |off: i64| mine(&w, |q| {
            q.sort = Some("title".into());
            q.limit = Some(2);
            q.offset = Some(off);
        });
        assert_eq!(titles(&by_title(0).posts, |p| &p.title), ["100% done", "a_b"]);
        assert_eq!(titles(&by_title(2).posts, |p| &p.title), ["Alpha one", "alpha two"], "case-insensitive A-Z, page two");
        assert_eq!(titles(&by_title(4).posts, |p| &p.title), ["Beta"]);
        let default_order = mine(&w, |q| q.sort = Some("updated".into()));
        assert_eq!(titles(&default_order.posts, |p| &p.title), titles(&all.posts, |p| &p.title), "`updated` is the default order");
        assert_eq!(titles(&mine(&w, |q| q.sort = Some("title".into())).courses, |c| &c.title), ["Alpha course", "Gamma"]);
        assert_eq!(mine(&w, |q| q.sort = Some("  ".into())).posts.len(), 5, "blank sort = default");

        // bad input is refused, not guessed at
        for (q, code) in [
            (MyQuery { sort: Some("random".into()), ..Default::default() }, "invalid_sort"),
            (MyQuery { status: Some("archived".into()), ..Default::default() }, "invalid_status"),
            (MyQuery { kind: Some("exam".into()), ..Default::default() }, "invalid_type"),
            (MyQuery { q: Some("x".repeat(101)), ..Default::default() }, "invalid_query"),
        ] {
            let e = MyFilter::parse(&q).unwrap_err();
            assert_eq!((e.0, code_of(&e).contains(code)), (StatusCode::BAD_REQUEST, true), "{code}");
        }

        // the live flags
        let live = mine(&w, |q| q.kind = Some("live".into())).live;
        let by = |id: &str| live.iter().find(|l| l.live.id == id).unwrap();
        assert_eq!((by("l1").ended, by("l1").visible), (false, true));
        assert_eq!((by("l2").ended, by("l2").visible, by("l2").hidden_reason), (true, false, None));

        // the other teacher sees only their own, with the very same filters
        let theirs = my_content(&w.conn, &other.id, &MyFilter::parse(&MyQuery { q: Some("alpha".into()), ..Default::default() }).unwrap(), now).unwrap();
        assert_eq!((theirs.totals.posts, theirs.totals.courses, theirs.totals.live), (1, 1, 1));
        assert_eq!(titles(&theirs.posts, |p| &p.title), ["Alpha other"]);
        let json = serde_json::to_value(&all).unwrap();
        for key in ["posts", "courses", "live", "totals"] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(json["live"][0].get("ended").is_some() && json["courses"][0].get("hidden_reason").is_some());
    }

    #[test]
    fn a_finished_live_session_can_be_renamed_but_moving_its_time_is_still_checked() {
        let w = world();
        let now = now_ms();
        raw_live(&w.conn, "l1", &w.teacher.id, "s1", "scheduled", "قديم", now - 5 * 3_600_000, 60);
        let cur = get_live(&w.conn, "l1").unwrap();
        // the editor sends the time back with every save
        let echo = |title: &str| LiveReq {
            subject_id: None, title: Some(title.into()), description: None, starts_at: Some(cur.starts_at), duration_min: Some(cur.duration_min), join_url: Some(cur.join_url.clone()), status: None,
        };
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &echo("اسم جديد")).unwrap().title, "اسم جديد");
        // really moving the time (or the length) is checked as before
        let moved = |starts: Option<i64>, dur: Option<i64>| LiveReq { subject_id: None, title: None, description: None, starts_at: starts, duration_min: dur, join_url: None, status: None };
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &moved(Some(now - 10 * 86_400_000), None)).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &moved(None, Some(2))).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert!(update_live(&w.conn, &w.teacher, "l1", &moved(Some(now + 3_600_000), Some(45))).is_ok());
        assert_eq!(get_live(&w.conn, "l1").unwrap().duration_min, 45);
    }

    #[test]
    fn a_live_join_link_must_stay_https_on_update_and_scheduling_needs_a_live_assignment() {
        let w = world();
        let now = now_ms();
        raw_live(&w.conn, "l1", &w.teacher.id, "s1", "scheduled", "بث", now + 3_600_000, 60);
        let set_url = |u: &str| LiveReq { subject_id: None, title: None, description: None, starts_at: None, duration_min: None, join_url: Some(u.into()), status: None };
        for bad_url in ["http://meet.example.com/x", "javascript:alert(1)", "ftp://x.com/a", "https://u:p@example.com/", "", "https://exa mple.com"] {
            let e = update_live(&w.conn, &w.teacher, "l1", &set_url(bad_url)).unwrap_err();
            assert_eq!((e.0, code_of(&e).contains("invalid_url")), (StatusCode::BAD_REQUEST, true), "{bad_url:?}");
        }
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &set_url("https://zoom.example.com/j/1")).unwrap().join_url, "https://zoom.example.com/j/1");
        assert_eq!(get_live(&w.conn, "l1").unwrap().join_url, "https://zoom.example.com/j/1", "a refused link changed nothing before it");

        let cancel = LiveReq { subject_id: None, title: None, description: None, starts_at: None, duration_min: None, join_url: None, status: Some("cancelled".into()) };
        let schedule = LiveReq { status: Some("scheduled".into()), ..LiveReq { subject_id: None, title: None, description: None, starts_at: None, duration_min: None, join_url: None, status: None } };
        update_live(&w.conn, &w.admin, "l1", &cancel).unwrap(); // moderation needs no assignment
        w.conn.execute("DELETE FROM teacher_subjects", []).unwrap();
        // the owner cannot switch it back on, nor can an admin do it for them
        for who in [&w.teacher, &w.admin] {
            let e = update_live(&w.conn, who, "l1", &schedule).unwrap_err();
            assert_eq!((e.0, code_of(&e).contains("not_assigned")), (StatusCode::FORBIDDEN, true), "{}", who.role);
        }
        assert_eq!(get_live(&w.conn, "l1").unwrap().status, "cancelled");
        approve(&w.conn, &w.teacher, "s1");
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &schedule).unwrap().status, "scheduled");
    }

    #[test]
    fn a_live_update_answers_in_the_owner_shape_so_the_editor_keeps_the_ended_and_hidden_state() {
        let w = world();
        let now = now_ms();
        raw_live(&w.conn, "l1", &w.teacher.id, "s1", "scheduled", "قديم", now - 5 * 3_600_000, 60);
        raw_live(&w.conn, "l2", &w.teacher.id, "s1", "scheduled", "قادم", now + 3_600_000, 60);
        let rename = |title: &str, status: Option<&str>| LiveReq { subject_id: None, title: Some(title.into()), description: None, starts_at: None, duration_min: None, join_url: None, status: status.map(String::from) };
        let done = update_my_live(&w.conn, &w.teacher, "l1", &rename("اسم جديد", None), now).unwrap();
        assert_eq!((done.live.title.as_str(), done.ended, done.visible, done.hidden_reason), ("اسم جديد", true, false, None));
        let up = update_my_live(&w.conn, &w.teacher, "l2", &rename("قادم 2", Some("cancelled")), now).unwrap();
        assert_eq!((up.live.status.as_str(), up.ended, up.visible), ("cancelled", false, false));
        let back = update_my_live(&w.conn, &w.teacher, "l2", &rename("قادم 2", Some("scheduled")), now).unwrap();
        assert_eq!((back.ended, back.visible, back.hidden_reason), (false, true, None));
        // hidden by the subject: the reply says why, so the editor's explanation does not disappear after a save
        w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
        let hidden = update_my_live(&w.conn, &w.teacher, "l2", &rename("قادم 3", None), now).unwrap();
        assert_eq!((hidden.visible, hidden.hidden_reason), (false, Some(HiddenReason::SubjectInactive)));
        let json = serde_json::to_value(&hidden).unwrap();
        assert!(json.get("ended").is_some() && json.get("hidden_reason").is_some() && json.get("join_url").is_some());
        assert_eq!(update_my_live(&w.conn, &w.student, "l2", &rename("x", None), now).unwrap_err().0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn saving_an_already_scheduled_session_needs_no_live_assignment_only_the_transition_does() {
        let w = world();
        let now = now_ms();
        raw_live(&w.conn, "l1", &w.teacher.id, "s1", "scheduled", "بث", now - 5 * 3_600_000, 60); // already over
        w.conn.execute("DELETE FROM teacher_subjects", []).unwrap();
        let req = |title: &str, status: Option<&str>| LiveReq {
            subject_id: None, title: Some(title.into()), description: None, starts_at: None, duration_min: None, join_url: None, status: status.map(String::from),
        };
        // the editor echoes status 'scheduled' with every save: still not a transition
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &req("عنوان مصحَّح", Some("scheduled"))).unwrap().title, "عنوان مصحَّح");
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &req("آخر", None)).unwrap().title, "آخر");
        // cancelling is always allowed, but coming back from it is the transition that needs the approval
        assert_eq!(update_live(&w.conn, &w.teacher, "l1", &req("آخر", Some("cancelled"))).unwrap().status, "cancelled");
        let e = update_live(&w.conn, &w.teacher, "l1", &req("آخر", Some("scheduled"))).unwrap_err();
        assert_eq!((e.0, code_of(&e).contains("not_assigned")), (StatusCode::FORBIDDEN, true));
    }

    #[test]
    fn publishing_needs_a_live_assignment_for_posts_and_courses_too_whoever_does_it() {
        let w = world();
        let draft = create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap();
        let course = create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("د".into()), description: None, status: None }).unwrap();
        add_lesson(&w.conn, &w.teacher, &course.course.id, &LessonReq { title: Some("د".into()), description: None, section: None, video_url: Some("https://youtu.be/dQw4w9WgXcQ".into()) }).unwrap();
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected'", []).unwrap();
        let publish_post = PostReq { subject_id: None, kind: None, title: None, body: None, status: Some("published".into()) };
        let publish_course = CourseReq { subject_id: None, title: None, description: None, status: Some("published".into()) };
        for who in [&w.teacher, &w.admin] {
            assert_eq!(update_post(&w.conn, who, &draft.id, &publish_post).unwrap_err().0, StatusCode::FORBIDDEN, "post by {}", who.role);
            assert_eq!(update_course(&w.conn, who, &course.course.id, &publish_course).unwrap_err().0, StatusCode::FORBIDDEN, "course by {}", who.role);
        }
        assert_eq!(get_post(&w.conn, &draft.id).unwrap().status, "draft");
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved'", []).unwrap();
        assert_eq!(update_post(&w.conn, &w.teacher, &draft.id, &publish_post).unwrap().status, "published");
        assert_eq!(update_course(&w.conn, &w.teacher, &course.course.id, &publish_course).unwrap().course.status, "published");
    }

    #[test]
    fn a_course_without_lessons_cannot_be_published_on_create_or_update() {
        let w = world();
        let new_course = |status: Option<&str>| CourseReq { subject_id: Some("s1".into()), title: Some("دورة".into()), description: None, status: status.map(Into::into) };
        let e = create_course(&w.conn, &w.teacher, &new_course(Some("published"))).unwrap_err();
        assert_eq!((e.0, code_of(&e).contains("course_needs_lessons")), (StatusCode::BAD_REQUEST, true));
        let n: i64 = w.conn.query_row("SELECT count(*) FROM courses", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "nothing was created");

        let c = create_course(&w.conn, &w.teacher, &new_course(None)).unwrap();
        assert_eq!(c.course.status, "draft");
        let publish = CourseReq { subject_id: None, title: None, description: None, status: Some("published".into()) };
        let e = update_course(&w.conn, &w.teacher, &c.course.id, &publish).unwrap_err();
        assert_eq!((e.0, code_of(&e).contains("course_needs_lessons")), (StatusCode::BAD_REQUEST, true));
        let notified = || -> i64 { w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'new_course'", [], |r| r.get(0)).unwrap() };
        assert_eq!((get_course(&w.conn, &c.course.id).unwrap().course.status.as_str(), notified()), ("draft", 0), "a refused publication is not announced either");

        add_lesson(&w.conn, &w.teacher, &c.course.id, &LessonReq { title: Some("درس".into()), description: None, section: None, video_url: Some("https://youtu.be/dQw4w9WgXcQ".into()) }).unwrap();
        assert_eq!(update_course(&w.conn, &w.teacher, &c.course.id, &publish).unwrap().course.status, "published");
        assert_eq!(notified(), 1, "announced once, now that it has something to watch");
        // renaming it later (still published) is not a publication
        let rename = CourseReq { subject_id: None, title: Some("اسم".into()), description: None, status: None };
        assert_eq!(update_course(&w.conn, &w.teacher, &c.course.id, &rename).unwrap().course.title, "اسم");
        assert_eq!(notified(), 1);
    }

    #[test]
    fn duplicating_a_post_makes_an_owned_draft_without_the_attachment() {
        let w = world();
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        approve(&w.conn, &other, "s1");
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        w.conn.execute("INSERT INTO files VALUES ('f1', ?1, 'a.pdf', 'application/pdf', 4, 0)", params![w.teacher.id]).unwrap();
        w.conn.execute("UPDATE posts SET file_id = 'f1' WHERE id = ?1", params![p.id]).unwrap();

        let copy = duplicate_post(&w.conn, &w.teacher, &p.id, &DuplicateReq::default()).unwrap();
        assert_ne!(copy.id, p.id);
        assert_eq!((copy.status.as_str(), copy.title.as_str(), copy.kind.as_str(), copy.body.as_deref(), copy.teacher_id.as_str()), ("draft", "ملخص (نسخة)", "summary", Some("نص"), w.teacher.id.as_str()));
        assert!(copy.file.is_none(), "the attachment belongs to the original only");
        let original = get_post(&w.conn, &p.id).unwrap();
        assert_eq!((original.status.as_str(), original.file.as_ref().map(|f| f.name.as_str())), ("published", Some("a.pdf")), "the original is untouched");
        assert_eq!(list_posts(&w.conn, Scope::Subject("s1")).unwrap().len(), 1, "the copy is a draft: students do not see it");
        let notified: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'new_post'", [], |r| r.get(0)).unwrap();
        assert_eq!(notified, 1, "only the original's publication was announced");

        // titles: explicit wins, blank falls back, the limit is the title limit
        let named = duplicate_post(&w.conn, &w.teacher, &p.id, &DuplicateReq { title: Some("  عنوان خاص  ".into()) }).unwrap();
        assert_eq!(named.title, "عنوان خاص");
        assert_eq!(duplicate_post(&w.conn, &w.teacher, &p.id, &DuplicateReq { title: Some("   ".into()) }).unwrap().title, "ملخص (نسخة)");
        assert_eq!(duplicate_post(&w.conn, &w.teacher, &p.id, &DuplicateReq { title: Some("x".repeat(201)) }).unwrap_err().0, StatusCode::BAD_REQUEST);
        w.conn.execute("UPDATE posts SET title = ?1 WHERE id = ?2", params!["ع".repeat(200), p.id]).unwrap();
        let long = duplicate_post(&w.conn, &w.teacher, &p.id, &DuplicateReq::default()).unwrap();
        assert_eq!(long.title.chars().count(), 200);
        assert!(long.title.ends_with("(نسخة)"));

        // only the owner: a colleague, an admin and a stranger's id all create nothing
        let count = || -> i64 { w.conn.query_row("SELECT count(*) FROM posts", [], |r| r.get(0)).unwrap() };
        let before = count();
        // even an admin who somehow holds an assignment for the subject cannot copy it: only the owner can
        approve(&w.conn, &w.admin, "s1");
        for who in [&other, &w.admin, &w.student] {
            let e = duplicate_post(&w.conn, who, &p.id, &DuplicateReq::default()).unwrap_err();
            assert_eq!((e.0, code_of(&e).contains("forbidden")), (StatusCode::FORBIDDEN, true), "{}", who.role);
        }
        assert_eq!(duplicate_post(&w.conn, &w.teacher, "nope", &DuplicateReq::default()).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(count(), before);
        // and the owner needs the assignment, like for creating anything
        w.conn.execute("UPDATE teacher_subjects SET status = 'pending' WHERE teacher_id = ?1", params![w.teacher.id]).unwrap();
        assert_eq!(duplicate_post(&w.conn, &w.teacher, &p.id, &DuplicateReq::default()).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(count(), before);
    }

    #[test]
    fn duplicating_a_course_copies_its_lessons_in_order_as_a_draft_with_no_progress() {
        let w = world();
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        approve(&w.conn, &other, "s1");
        let c = create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("دورة".into()), description: Some("وصف".into()), status: None }).unwrap();
        let lesson = |title: &str, section: Option<&str>, url: &str| LessonReq { title: Some(title.into()), description: Some(format!("وصف {title}")), section: section.map(Into::into), video_url: Some(url.into()) };
        let l1 = add_lesson(&w.conn, &w.teacher, &c.course.id, &lesson("أ", Some("القسم 1"), "https://youtu.be/dQw4w9WgXcQ")).unwrap();
        add_lesson(&w.conn, &w.teacher, &c.course.id, &lesson("ب", Some("القسم 1"), "https://vimeo.com/123456")).unwrap();
        add_lesson(&w.conn, &w.teacher, &c.course.id, &lesson("ج", None, "https://example.com/page")).unwrap();
        update_course(&w.conn, &w.teacher, &c.course.id, &CourseReq { subject_id: None, title: None, description: None, status: Some("published".into()) }).unwrap();
        w.conn.execute("INSERT INTO lesson_progress VALUES (?1, ?2, 1)", params![w.student.id, l1.id]).unwrap();

        let copy = duplicate_course(&w.conn, &w.teacher, &c.course.id, &DuplicateReq::default()).unwrap();
        assert_ne!(copy.course.id, c.course.id);
        assert_eq!((copy.course.status.as_str(), copy.course.title.as_str(), copy.course.description.as_deref(), copy.course.lesson_count), ("draft", "دورة (نسخة)", Some("وصف"), 3));
        let shape = |d: &CourseDetail| d.lessons.iter().map(|l| (l.position, l.section.clone(), l.title.clone(), l.description.clone(), l.video_url.clone(), l.embed_url.clone(), l.video_kind.clone())).collect::<Vec<_>>();
        assert_eq!(shape(&copy), shape(&get_course(&w.conn, &c.course.id).unwrap()), "same order, sections, links and embeds");
        let ids: std::collections::HashSet<_> = copy.lessons.iter().map(|l| l.id.clone()).chain(get_course(&w.conn, &c.course.id).unwrap().lessons.into_iter().map(|l| l.id)).collect();
        assert_eq!(ids.len(), 6, "new ids: the two courses share no lesson");
        let progress: i64 = w.conn.query_row("SELECT count(*) FROM lesson_progress", [], |r| r.get(0)).unwrap();
        assert_eq!(progress, 1, "nobody's progress is carried over");
        assert_eq!(list_courses(&w.conn, Scope::Subject("s1")).unwrap().len(), 1, "the copy is a draft: not listed to students");
        assert_eq!(get_course(&w.conn, &c.course.id).unwrap().course.status, "published");

        let count = || -> i64 { w.conn.query_row("SELECT (SELECT count(*) FROM courses) * 100 + (SELECT count(*) FROM lessons)", [], |r| r.get(0)).unwrap() };
        let before = count();
        // even an admin who somehow holds an assignment for the subject cannot copy it: only the owner can
        approve(&w.conn, &w.admin, "s1");
        for who in [&other, &w.admin] {
            let e = duplicate_course(&w.conn, who, &c.course.id, &DuplicateReq::default()).unwrap_err();
            assert_eq!((e.0, code_of(&e).contains("forbidden")), (StatusCode::FORBIDDEN, true), "{}", who.role);
        }
        assert_eq!(duplicate_course(&w.conn, &w.teacher, "nope", &DuplicateReq::default()).unwrap_err().0, StatusCode::NOT_FOUND);
        w.conn.execute("DELETE FROM teacher_subjects WHERE teacher_id = ?1", params![w.teacher.id]).unwrap();
        assert_eq!(duplicate_course(&w.conn, &w.teacher, &c.course.id, &DuplicateReq::default()).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(count(), before, "a refused copy leaves no half-made course behind");
    }

    #[test]
    fn deleting_content_deletes_the_reviews_and_reports_that_point_at_it_and_nothing_else() {
        let w = world();
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        let now = now_ms();
        let t = &w.teacher.id;
        raw_post(&w.conn, "p1", t, "s1", "published", "منشور", now);
        raw_post(&w.conn, "p2", t, "s1", "published", "آخر", now);
        raw_course(&w.conn, "c1", t, "s1", "published", "دورة", now);
        raw_course(&w.conn, "c2", t, "s1", "published", "أخرى", now);
        raw_live(&w.conn, "l1", t, "s1", "scheduled", "بث", now + 3_600_000, 60);
        raw_live(&w.conn, "l2", t, "s1", "scheduled", "بث 2", now + 3_600_000, 60);
        let report = |id: &str, ttype: &str, tid: &str| {
            w.conn
                .execute("INSERT INTO reports(id,reporter_id,target_type,target_id,reason,status,created_at) VALUES (?1,?2,?3,?4,'سبب','open',0)", params![id, w.student.id, ttype, tid])
                .unwrap()
        };
        let review = |id: &str, ttype: &str, tid: &str| {
            w.conn
                .execute("INSERT INTO reviews(id,student_id,target_type,target_id,rating,created_at,updated_at) VALUES (?1,?2,?3,?4,5,0,0)", params![id, w.student.id, ttype, tid])
                .unwrap()
        };
        report("r-p1", "post", "p1");
        report("r-p2", "post", "p2");
        report("r-c1", "course", "c1");
        report("r-c2", "course", "c2");
        report("r-l1", "live", "l1");
        report("r-l2", "live", "l2");
        report("r-teacher", "teacher", t);
        review("v-c1", "course", "c1");
        review("v-c2", "course", "c2");
        review("v-teacher", "teacher", t);
        report("r-v-c1", "review", "v-c1");
        report("r-v-c2", "review", "v-c2");
        report("r-v-teacher", "review", "v-teacher");
        let left = || -> Vec<String> {
            let mut ids: Vec<String> = w.conn.prepare("SELECT id FROM reports UNION ALL SELECT id FROM reviews").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
            ids.sort();
            ids
        };
        assert_eq!(left().len(), 13);

        // a stranger cannot delete (and nothing is removed along the way)
        assert_eq!(delete_post(&w.conn, &other, "p1").unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(delete_course(&w.conn, &other, "c1").unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(delete_live(&w.conn, &other, "l1").unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(left().len(), 13);

        delete_post(&w.conn, &w.teacher, "p1").unwrap();
        assert!(!left().contains(&"r-p1".to_string()) && left().contains(&"r-p2".to_string()));
        delete_live(&w.conn, &w.teacher, "l1").unwrap();
        assert!(!left().contains(&"r-l1".to_string()) && left().contains(&"r-l2".to_string()));
        delete_course(&w.conn, &w.teacher, "c1").unwrap();
        let after = left();
        for gone in ["r-c1", "v-c1", "r-v-c1"] {
            assert!(!after.contains(&gone.to_string()), "{gone} is removed with the course");
        }
        for kept in ["r-p2", "r-l2", "r-c2", "v-c2", "r-v-c2", "r-teacher", "v-teacher", "r-v-teacher"] {
            assert!(after.contains(&kept.to_string()), "{kept} belongs to something that still exists");
        }
        assert_eq!(after.len(), 13 - 5);
        assert_eq!(delete_course(&w.conn, &w.teacher, "c1").unwrap_err().0, StatusCode::NOT_FOUND);
        // admins may still moderate by deleting; the cleanup is the same
        delete_post(&w.conn, &w.admin, "p2").unwrap();
        assert!(!left().contains(&"r-p2".to_string()));
        let audits: i64 = w.conn.query_row("SELECT count(*) FROM audit_log WHERE action IN ('post_deleted','course_deleted','live_deleted')", [], |r| r.get(0)).unwrap();
        assert_eq!(audits, 4);
    }

    #[test]
    fn get_live_gives_the_owner_or_an_admin_the_owners_view() {
        let w = world();
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        let now = now_ms();
        raw_live(&w.conn, "l1", &w.teacher.id, "s1", "scheduled", "بث", now + 3_600_000, 60);
        raw_live(&w.conn, "l2", &w.teacher.id, "s1", "scheduled", "منتهي", now - 5 * 3_600_000, 60);
        let l = get_my_live(&w.conn, &w.teacher, "l1", now).unwrap();
        assert_eq!((l.live.title.as_str(), l.ended, l.visible, l.hidden_reason), ("بث", false, true, None));
        let json = serde_json::to_value(&l).unwrap();
        for key in ["id", "teacher_id", "subject_id", "subject_name", "title", "description", "starts_at", "duration_min", "join_url", "status", "ended", "visible", "hidden_reason"] {
            assert!(json.get(key).is_some(), "{key} in {json}");
        }
        let list_item = serde_json::to_value(&mine(&w, |q| q.kind = Some("live".into())).live[0]).unwrap();
        let keys = |v: &serde_json::Value| -> std::collections::BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
        assert_eq!(keys(&json), keys(&list_item), "same shape as an item of /my/content");
        let done = get_my_live(&w.conn, &w.teacher, "l2", now).unwrap();
        assert_eq!((done.ended, done.visible, done.hidden_reason), (true, false, None));
        w.conn.execute("UPDATE subjects SET is_active = 0", []).unwrap();
        let hidden = get_my_live(&w.conn, &w.admin, "l1", now).unwrap();
        assert_eq!((hidden.visible, hidden.hidden_reason), (false, Some(HiddenReason::SubjectInactive)), "an admin gets the same view");
        for who in [&other, &w.student] {
            assert_eq!(get_my_live(&w.conn, who, "l1", now).unwrap_err().0, StatusCode::FORBIDDEN, "{}", who.role);
        }
        assert_eq!(get_my_live(&w.conn, &w.teacher, "nope", now).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    // ───── phase 3-4: announce once, live changes, back-fill ─────

    fn count_kind(conn: &Connection, user: &str, kind: &str) -> i64 {
        conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = ?2", params![user, kind], |r| r.get(0)).unwrap()
    }

    fn stamp(conn: &Connection, table: &str, id: &str) -> Option<i64> {
        conn.query_row(&format!("SELECT published_at FROM {table} WHERE id = ?1"), params![id], |r| r.get(0)).unwrap()
    }

    fn status_req(status: &str) -> PostReq {
        PostReq { subject_id: None, kind: None, title: None, body: None, status: Some(status.into()) }
    }

    #[test]
    fn a_post_is_announced_once_however_often_it_goes_back_to_a_draft_and_out_again() {
        let w = world();
        let told = || count_kind(&w.conn, &w.student.id, "new_post");
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap();
        assert_eq!((told(), stamp(&w.conn, "posts", &p.id)), (0, None), "a draft is not announced and has no publication time");
        update_post(&w.conn, &w.teacher, &p.id, &status_req("published")).unwrap();
        let first = stamp(&w.conn, "posts", &p.id).expect("claimed by the first publication");
        assert_eq!(told(), 1);
        for round in 0..3 {
            update_post(&w.conn, &w.teacher, &p.id, &status_req("draft")).unwrap();
            assert_eq!(stamp(&w.conn, "posts", &p.id), Some(first), "round {round}: unpublishing keeps the time");
            update_post(&w.conn, &w.teacher, &p.id, &status_req("published")).unwrap();
            assert_eq!((told(), stamp(&w.conn, "posts", &p.id)), (1, Some(first)), "round {round}: publishing again announces nothing and keeps the first time");
        }
        // a save that leaves it published is not a publication either
        update_post(&w.conn, &w.teacher, &p.id, &PostReq { subject_id: None, kind: None, title: Some("عنوان جديد".into()), body: None, status: None }).unwrap();
        assert_eq!(told(), 1);
        // created as published: announced at creation, once
        let direct = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        assert_eq!((told(), stamp(&w.conn, "posts", &direct.id).is_some()), (2, true));
        // a copy is a new draft with no history: its own first publication announces
        let copy = duplicate_post(&w.conn, &w.teacher, &p.id, &DuplicateReq::default()).unwrap();
        assert_eq!((stamp(&w.conn, "posts", &copy.id), told()), (None, 2), "duplicates start without a publication time");
        update_post(&w.conn, &w.teacher, &copy.id, &status_req("published")).unwrap();
        assert_eq!((told(), stamp(&w.conn, "posts", &copy.id).is_some()), (3, true));
    }

    #[test]
    fn a_course_is_announced_once_too_and_a_copy_starts_over() {
        let w = world();
        let told = || count_kind(&w.conn, &w.student.id, "new_course");
        let c = create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("دورة".into()), description: None, status: Some("draft".into()) }).unwrap();
        let id = c.course.id.clone();
        add_lesson(&w.conn, &w.teacher, &id, &LessonReq { title: Some("درس".into()), description: None, section: None, video_url: Some("https://youtu.be/dQw4w9WgXcQ".into()) }).unwrap();
        let set = |status: &str| update_course(&w.conn, &w.teacher, &id, &CourseReq { subject_id: None, title: None, description: None, status: Some(status.into()) }).unwrap();
        assert_eq!((told(), stamp(&w.conn, "courses", &id)), (0, None));
        set("published");
        let first = stamp(&w.conn, "courses", &id).expect("claimed");
        assert_eq!(told(), 1);
        for _ in 0..3 {
            set("draft");
            set("published");
        }
        assert_eq!((told(), stamp(&w.conn, "courses", &id)), (1, Some(first)));
        let copy = duplicate_course(&w.conn, &w.teacher, &id, &DuplicateReq::default()).unwrap();
        assert_eq!(stamp(&w.conn, "courses", &copy.course.id), None, "a copy starts without a publication time");
    }

    #[test]
    fn the_migration_back_fills_published_rows_so_old_content_is_never_announced_again() {
        // a database written before `published_at` existed (the real v1 dump), upgraded the way the server does it
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        c.execute_batch(include_str!("../tests/fixtures/platform_v1.sql")).unwrap();
        for t in ["posts", "courses"] {
            assert!(!crate::platform::column_exists(&c, t, "published_at").unwrap(), "{t}: the fixture predates the column");
        }
        crate::platform::apply_schema(&c).unwrap();
        let get = |sql: &str| -> (i64, Option<i64>) { c.query_row(sql, [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() };
        let (created, published) = get("SELECT created_at, published_at FROM posts WHERE status = 'published' LIMIT 1");
        assert_eq!(published, Some(created), "an already published post counts as announced when it was created");
        let (created, published) = get("SELECT created_at, published_at FROM courses WHERE status = 'published' LIMIT 1");
        assert_eq!(published, Some(created));
        // a draft has not been announced; running the migration again changes nothing (idempotent)
        c.execute("UPDATE posts SET status = 'draft', published_at = NULL", []).unwrap();
        let before: Vec<Option<i64>> = c.prepare("SELECT published_at FROM courses ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        crate::platform::apply_schema(&c).unwrap();
        crate::platform::apply_schema(&c).unwrap();
        let after: Vec<Option<i64>> = c.prepare("SELECT published_at FROM courses ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(before, after);
        assert_eq!(get("SELECT count(*), max(published_at) FROM posts WHERE status = 'draft'").1, None, "a draft is not back-filled");

        // and the point of it: a legacy published post that is unpublished and published again is NOT announced again
        let w = world();
        raw_post(&w.conn, "legacy", &w.teacher.id, "s1", "published", "قديم", 1_000);
        assert_eq!(stamp(&w.conn, "posts", "legacy"), None, "the legacy state: published, never stamped");
        migrate(&w.conn).unwrap();
        assert_eq!(stamp(&w.conn, "posts", "legacy"), Some(1_000));
        update_post(&w.conn, &w.teacher, "legacy", &status_req("draft")).unwrap();
        update_post(&w.conn, &w.teacher, "legacy", &status_req("published")).unwrap();
        assert_eq!(count_kind(&w.conn, &w.student.id, "new_post"), 0, "nobody is told about old content again");
    }

    fn live_req(f: impl FnOnce(&mut LiveReq)) -> LiveReq {
        let mut r = LiveReq { subject_id: None, title: None, description: None, starts_at: None, duration_min: None, join_url: None, status: None };
        f(&mut r);
        r
    }

    #[test]
    fn live_updated_goes_out_only_when_the_time_or_the_link_of_a_scheduled_session_moves() {
        let w = world();
        let follower = insert_test_user(&w.conn, "f@x.com", "student", "active");
        w.conn.execute("INSERT INTO teacher_follows VALUES (?1, ?2, 0)", params![follower.id, w.teacher.id]).unwrap();
        let now = now_ms();
        let l = create_live(
            &w.conn,
            &w.teacher,
            &LiveReq { subject_id: Some("s1".into()), title: Some("درس".into()), description: None, starts_at: Some(now + 7_200_000), duration_min: Some(60), join_url: Some("https://meet.example.com/a".into()), status: None },
        )
        .unwrap();
        let updated = |u: &User| count_kind(&w.conn, &u.id, "live_updated");
        let all = || (updated(&w.student), updated(&follower), updated(&w.teacher));
        assert_eq!(count_kind(&w.conn, &w.student.id, "live_scheduled"), 1);

        // title, description, duration and an unchanged time/link are not worth a notice
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.title = Some("عنوان آخر".into()))).unwrap();
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.description = Some("وصف".into()))).unwrap();
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.duration_min = Some(90))).unwrap();
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| { r.starts_at = Some(now + 7_200_000); r.join_url = Some("https://meet.example.com/a".into()); })).unwrap();
        assert_eq!(all(), (0, 0, 0));

        // a new time: once per save, to everyone who would be told about a new session - never the teacher
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.starts_at = Some(now + 10_800_000))).unwrap();
        assert_eq!(all(), (1, 1, 0));
        let (data, link): (String, String) = w.conn.query_row("SELECT data, link FROM notifications WHERE user_id = ?1 AND kind = 'live_updated'", params![w.student.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let data: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!((data["title"].as_str(), data["teacher"].as_str(), data["starts_at"].as_i64()), (Some("عنوان آخر"), Some("T"), Some(now + 10_800_000)));
        assert_eq!(link, "/platform/subjects/s1");
        // a new link; and both at once is still ONE notice for that save
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.join_url = Some("https://meet.example.com/b".into()))).unwrap();
        assert_eq!(all(), (2, 2, 0));
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| { r.starts_at = Some(now + 14_400_000); r.join_url = Some("https://meet.example.com/c".into()); })).unwrap();
        assert_eq!(all(), (3, 3, 0), "one notice per save, not one per field");
        assert_eq!(count_kind(&w.conn, &w.student.id, "live_scheduled"), 1, "and none of it is announced as a new session");
    }

    #[test]
    fn a_cancelled_session_that_comes_back_is_announced_as_scheduled_and_a_finished_one_is_never_re_announced() {
        let w = world();
        let now = now_ms();
        let mk = |start: i64, dur: i64| create_live(&w.conn, &w.teacher, &LiveReq { subject_id: Some("s1".into()), title: Some("درس".into()), description: None, starts_at: Some(start), duration_min: Some(dur), join_url: Some("https://meet.example.com/a".into()), status: None }).unwrap();
        let l = mk(now + 7_200_000, 60);
        let n = |kind: &str| count_kind(&w.conn, &w.student.id, kind);
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.status = Some("cancelled".into()))).unwrap();
        assert_eq!((n("live_cancelled"), n("live_scheduled")), (1, 1));
        // a link change while it is cancelled concerns nobody
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.join_url = Some("https://meet.example.com/z".into()))).unwrap();
        assert_eq!(n("live_updated"), 0);
        // bringing it back is a (re)scheduling, not an update
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.status = Some("scheduled".into()))).unwrap();
        assert_eq!((n("live_scheduled"), n("live_updated")), (2, 0));
        // a session that is over: nothing to tell, whatever changes
        raw_live(&w.conn, "over", &w.teacher.id, "s1", "scheduled", "انتهى", now - 3 * 3_600_000, 60);
        update_live(&w.conn, &w.teacher, "over", &live_req(|r| r.join_url = Some("https://meet.example.com/q".into()))).unwrap();
        assert_eq!(n("live_updated"), 0, "a finished session is not announced");
        // an admin's hand-made cancellation is still `content_unpublished` for the owner (and `live_cancelled` for the audience)
        update_live(&w.conn, &w.admin, &l.id, &live_req(|r| r.status = Some("cancelled".into()))).unwrap();
        assert_eq!((count_kind(&w.conn, &w.teacher.id, "content_unpublished"), count_kind(&w.conn, &w.teacher.id, "content_auto_hidden")), (1, 0));
        assert_eq!(n("live_cancelled"), 2);
    }

    #[test]
    fn a_session_students_cannot_see_is_not_announced_whatever_its_owner_changes() {
        let w = world();
        let follower = insert_test_user(&w.conn, "f@x.com", "student", "active");
        w.conn.execute("INSERT INTO teacher_follows VALUES (?1, ?2, 0)", params![follower.id, w.teacher.id]).unwrap();
        let now = now_ms();
        let l = create_live(
            &w.conn,
            &w.teacher,
            &LiveReq { subject_id: Some("s1".into()), title: Some("درس".into()), description: None, starts_at: Some(now + 7_200_000), duration_min: Some(60), join_url: Some("https://meet.example.com/a".into()), status: None },
        )
        .unwrap();
        let told = || {
            ["live_updated", "live_cancelled", "live_scheduled"].map(|k| (count_kind(&w.conn, &w.student.id, k), count_kind(&w.conn, &follower.id, k)))
        };
        assert_eq!(told(), [(0, 0), (0, 0), (1, 1)]);
        let mut hours = 2;
        let mut move_it = |w: &World| {
            hours += 1;
            update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| { r.title = Some(format!("{}", "ع".repeat(200))); r.starts_at = Some(now + hours * 3_600_000); })).unwrap();
        };
        // each way the session can stop being shown to students: the owner can still save it, nobody is told
        let hide: [(&str, &str, &str); 4] = [
            ("the assignment was revoked", "UPDATE teacher_subjects SET status = 'rejected'", "UPDATE teacher_subjects SET status = 'approved'"),
            ("the subject is switched off", "UPDATE subjects SET is_active = 0", "UPDATE subjects SET is_active = 1"),
            ("the institution is switched off", "UPDATE institutions SET is_active = 0", "UPDATE institutions SET is_active = 1"),
            ("the owner's account is suspended", "UPDATE users SET status = 'suspended' WHERE role = 'teacher'", "UPDATE users SET status = 'active' WHERE role = 'teacher'"),
        ];
        for (label, off, on) in hide {
            w.conn.execute(off, []).unwrap();
            move_it(&w);
            assert_eq!(told(), [(0, 0), (0, 0), (1, 1)], "{label}: no 'live_updated' for a session nobody can see");
            w.conn.execute(on, []).unwrap();
        }
        // back in good standing the same kind of save is announced again
        move_it(&w);
        assert_eq!(told()[0], (1, 1));
        // cancelling a hidden session tells nobody either; cancelling a visible one does
        w.conn.execute("UPDATE subjects SET is_active = 0", []).unwrap();
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.status = Some("cancelled".into()))).unwrap();
        assert_eq!(told()[1], (0, 0), "cancelled while hidden");
        w.conn.execute("UPDATE subjects SET is_active = 1", []).unwrap();
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.status = Some("scheduled".into()))).unwrap();
        assert_eq!(told()[2], (2, 2), "back on in good standing: announced like a new session");
        update_live(&w.conn, &w.teacher, &l.id, &live_req(|r| r.status = Some("cancelled".into()))).unwrap();
        assert_eq!(told()[1], (1, 1));
        // an admin bringing a suspended teacher's cancelled session back (the assignment is all that is asked of the owner)
        // does not announce it either: the owner is not in good standing
        w.conn.execute("UPDATE users SET status = 'suspended' WHERE role = 'teacher'", []).unwrap();
        update_live(&w.conn, &w.admin, &l.id, &live_req(|r| r.status = Some("scheduled".into()))).unwrap();
        assert_eq!(told()[2], (2, 2));
    }

    #[test]
    fn an_admin_publishing_a_draft_claims_the_announcement_so_it_goes_out_once_and_never_again() {
        let w = world();
        // a post
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap();
        assert_eq!((stamp(&w.conn, "posts", &p.id), count_kind(&w.conn, &w.student.id, "new_post")), (None, 0));
        update_post(&w.conn, &w.admin, &p.id, &status_req("published")).unwrap();
        let first = stamp(&w.conn, "posts", &p.id).expect("the admin's publication claimed it - the invariant holds on this path too");
        assert_eq!(count_kind(&w.conn, &w.student.id, "new_post"), 1, "announced, as the owner's post");
        let data: String = w.conn.query_row("SELECT data FROM notifications WHERE kind = 'new_post'", [], |r| r.get(0)).unwrap();
        assert!(data.contains("\"teacher\":\"T\""), "in the owner's name, not the admin's: {data}");
        // unpublished by the admin, republished by the owner: not announced a second time
        update_post(&w.conn, &w.admin, &p.id, &status_req("draft")).unwrap();
        update_post(&w.conn, &w.teacher, &p.id, &status_req("published")).unwrap();
        assert_eq!((stamp(&w.conn, "posts", &p.id), count_kind(&w.conn, &w.student.id, "new_post")), (Some(first), 1));
        // a post the owner published first is not announced again by the admin's later republication
        let q = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        assert_eq!(count_kind(&w.conn, &w.student.id, "new_post"), 2);
        update_post(&w.conn, &w.admin, &q.id, &status_req("draft")).unwrap();
        update_post(&w.conn, &w.admin, &q.id, &status_req("published")).unwrap();
        assert_eq!(count_kind(&w.conn, &w.student.id, "new_post"), 2);
        // a course
        let c = create_course(&w.conn, &w.teacher, &CourseReq { subject_id: Some("s1".into()), title: Some("دورة".into()), description: None, status: Some("draft".into()) }).unwrap();
        add_lesson(&w.conn, &w.teacher, &c.course.id, &LessonReq { title: Some("درس".into()), description: None, section: None, video_url: Some("https://youtu.be/dQw4w9WgXcQ".into()) }).unwrap();
        let set = |who: &User, status: &str| update_course(&w.conn, who, &c.course.id, &CourseReq { subject_id: None, title: None, description: None, status: Some(status.into()) }).unwrap();
        set(&w.admin, "published");
        assert_eq!((stamp(&w.conn, "courses", &c.course.id).is_some(), count_kind(&w.conn, &w.student.id, "new_course")), (true, 1));
        set(&w.admin, "draft");
        set(&w.teacher, "published");
        assert_eq!(count_kind(&w.conn, &w.student.id, "new_course"), 1);
        // an admin cannot publish into a subject the owner is no longer approved for (unchanged), and nothing is claimed then
        let r = create_post(&w.conn, &w.teacher, &post_req(&w, "draft")).unwrap();
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected'", []).unwrap();
        assert_eq!(update_post(&w.conn, &w.admin, &r.id, &status_req("published")).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(stamp(&w.conn, "posts", &r.id), None);
    }

    #[test]
    fn unpublishing_by_hand_keeps_saying_content_unpublished() {
        let w = world();
        let p = create_post(&w.conn, &w.teacher, &post_req(&w, "published")).unwrap();
        update_post(&w.conn, &w.admin, &p.id, &status_req("draft")).unwrap();
        assert_eq!((count_kind(&w.conn, &w.teacher.id, "content_unpublished"), count_kind(&w.conn, &w.teacher.id, "content_auto_hidden")), (1, 0));
    }
}
