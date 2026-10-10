//! Student engagement: lesson progress, ratings & reviews, in-app notifications.
//! Other platform modules call `notify` / `notify_audience` when something worth telling happens.

use crate::platform::{bad, db_err, lock, new_id, opt_text, require_active, require_role, Res, User, INSTITUTION_ACTIVE, SUBJECT_ACTIVE};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS lesson_progress (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  lesson_id TEXT NOT NULL REFERENCES lessons(id) ON DELETE CASCADE,
  completed_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, lesson_id)
);
CREATE TABLE IF NOT EXISTS reviews (
  id TEXT PRIMARY KEY,
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_type TEXT NOT NULL CHECK (target_type IN ('course','teacher')),
  target_id TEXT NOT NULL,
  rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
  comment TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (student_id, target_type, target_id)
);
CREATE INDEX IF NOT EXISTS idx_reviews_target ON reviews(target_type, target_id);
CREATE TABLE IF NOT EXISTS notifications (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  data TEXT NOT NULL,
  link TEXT,
  read_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notif_user ON notifications(user_id, created_at);
";

const NOTIFICATION_TTL_MS: i64 = 90 * 86_400_000;

// ───────── notifications ─────────

/// Stores a notification. `data` is rendered by the client from `kind` (no server-side language).
pub fn notify(conn: &Connection, user_id: &str, kind: &str, data: Value, link: &str) {
    let _ = conn.execute(
        "INSERT INTO notifications(id, user_id, kind, data, link, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![new_id(), user_id, kind, data.to_string(), link, now_ms()],
    );
}

/// Notifies every active student who follows the teacher or is enrolled in the subject (never the teacher).
pub fn notify_audience(conn: &Connection, teacher_id: &str, subject_id: &str, kind: &str, data: Value, link: &str) {
    let _ = conn.execute(
        "INSERT INTO notifications(id, user_id, kind, data, link, created_at)
         SELECT lower(hex(randomblob(16))), a.uid, ?3, ?4, ?5, ?6 FROM (
           SELECT student_id AS uid FROM teacher_follows WHERE teacher_id = ?1
           UNION SELECT student_id FROM subject_enrollments WHERE subject_id = ?2
         ) a JOIN users u ON u.id = a.uid AND u.status = 'active' AND u.id <> ?1",
        params![teacher_id, subject_id, kind, data.to_string(), link, now_ms()],
    );
}

pub fn purge_old_notifications(state: &crate::platform::PlatformState) {
    if let Ok(conn) = state.conn.lock() {
        let _ = conn.execute("DELETE FROM notifications WHERE created_at < ?1", params![now_ms() - NOTIFICATION_TTL_MS]);
    }
}

// ───────── coalesced notices (phase 3-4) ─────────

/// What [`upsert_unread`] does once the caller has seen the current unread notice (if there is one).
pub(crate) enum Fold {
    /// Store this data: the unread notice is updated in place (and moved to the top, `created_at` = now), or inserted
    /// when there is none.
    Write(Value),
    /// Take the unread notice away (a no-op when there is none).
    Remove,
    /// Leave everything as it is.
    Skip,
}

/// **The one helper for notices that must not pile up** (new enrolments, new followers, "answers wait for you", score
/// changes, new feedback): finds the newest UNREAD notice of `user_id` with this `kind` and `link`, hands its data
/// (`None` = there is none) to `fold`, and does what `fold` answers. A notice that was already read is never touched, so
/// the next event starts a new one. `now` is passed in (never read from the clock here) so the callers' rules around it
/// can be tested at their exact boundaries. Errors are returned: a caller inside a transaction must be able to roll back.
pub(crate) fn upsert_unread(conn: &Connection, user_id: &str, kind: &str, link: &str, now: i64, fold: impl FnOnce(Option<Value>) -> Res<Fold>) -> Res<()> {
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT id, data FROM notifications WHERE user_id = ?1 AND kind = ?2 AND link = ?3 AND read_at IS NULL ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![user_id, kind, link],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;
    let current = existing.as_ref().map(|(_, data)| serde_json::from_str::<Value>(data).unwrap_or(Value::Null));
    match (fold(current)?, existing) {
        (Fold::Skip, _) | (Fold::Remove, None) => {}
        (Fold::Remove, Some((id, _))) => {
            conn.execute("DELETE FROM notifications WHERE id = ?1", params![id]).map_err(db_err)?;
        }
        (Fold::Write(data), Some((id, _))) => {
            conn.execute("UPDATE notifications SET data = ?2, created_at = ?3 WHERE id = ?1", params![id, data.to_string(), now]).map_err(db_err)?;
        }
        (Fold::Write(data), None) => {
            conn.execute(
                "INSERT INTO notifications(id, user_id, kind, data, link, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![new_id(), user_id, kind, data.to_string(), link, now],
            )
            .map_err(db_err)?;
        }
    }
    Ok(())
}

/// `count` of a coalesced notice's data (1 when it has none), plus one.
fn next_count(current: &Option<Value>) -> i64 {
    current.as_ref().and_then(|d| d["count"].as_i64()).unwrap_or(0).max(0) + 1
}

/// Teachers who get "something happened in your subject": active teachers with an APPROVED assignment for it.
fn approved_teachers(conn: &Connection, subject_id: &str) -> Res<Vec<String>> {
    conn.prepare(
        "SELECT ts.teacher_id FROM teacher_subjects ts JOIN users u ON u.id = ts.teacher_id
         WHERE ts.subject_id = ?1 AND ts.status = 'approved' AND u.role = 'teacher' AND u.status = 'active' ORDER BY ts.teacher_id",
    )
    .map_err(db_err)?
    .query_map(params![subject_id], |r| r.get(0))
    .map_err(db_err)?
    .collect::<Result<Vec<String>, _>>()
    .map_err(db_err)
}

/// The text of a coalesced "people joined" notice (new enrolments, new followers) after one more person joined.
///
/// The count is **derived from state, not from events**: the notice remembers `since` (when its first person joined) and
/// `count_since(since)` says how many of those people are *still* there, so one student who leaves and joins again ten
/// times is one student. When nothing changed (the same person came back) the notice is left alone - not even moved to
/// the top. A notice without `since` (written before it existed) keeps counting events and starts remembering from now.
fn fold_joined(cur: &Option<Value>, now: i64, mut data: serde_json::Map<String, Value>, count_since: impl FnOnce(i64) -> Res<i64>) -> Res<Fold> {
    let since = cur.as_ref().and_then(|d| d["since"].as_i64());
    let (count, since) = match (cur, since) {
        (Some(_), Some(since)) => (count_since(since)?.max(1), since),
        (Some(_), None) => (next_count(cur), now),
        (None, _) => (1, now),
    };
    if cur.as_ref().map_or(false, |d| d["count"].as_i64() == Some(count) && d["since"].as_i64() == Some(since)) {
        return Ok(Fold::Skip);
    }
    data.insert("count".into(), json!(count));
    data.insert("since".into(), json!(since));
    Ok(Fold::Write(Value::Object(data)))
}

/// A student has just enrolled in `subject_id`: every active teacher approved for it gets ONE unread notice per subject
/// (`new_enrollment {subject, count, since}`), the count growing while it stays unread (see [`fold_joined`]: students
/// who joined since the notice began and are still enrolled). The student is never named.
pub(crate) fn tell_new_enrollment(conn: &Connection, subject_id: &str, now: i64) -> Res<()> {
    let name: String = conn.query_row("SELECT name_ar FROM subjects WHERE id = ?1", params![subject_id], |r| r.get(0)).optional().map_err(db_err)?.unwrap_or_default();
    let link = format!("/platform/subjects/{subject_id}");
    for teacher in approved_teachers(conn, subject_id)? {
        upsert_unread(conn, &teacher, "new_enrollment", &link, now, |cur| {
            let mut data = serde_json::Map::new();
            data.insert("subject".into(), json!(name));
            fold_joined(&cur, now, data, |since| {
                conn.query_row(
                    "SELECT count(*) FROM subject_enrollments e JOIN users u ON u.id = e.student_id AND u.status = 'active'
                     WHERE e.subject_id = ?1 AND e.created_at >= ?2",
                    params![subject_id, since],
                    |r| r.get(0),
                )
                .map_err(db_err)
            })
        })?;
    }
    Ok(())
}

/// A student has just followed `teacher_id` (a NEW follow): `new_follower {count, since}`, coalesced like enrolments
/// (people who followed since the notice began and still do). No name.
pub(crate) fn tell_new_follower(conn: &Connection, teacher_id: &str, now: i64) -> Res<()> {
    let link = format!("/platform/teachers/{teacher_id}");
    upsert_unread(conn, teacher_id, "new_follower", &link, now, |cur| {
        fold_joined(&cur, now, serde_json::Map::new(), |since| {
            conn.query_row(
                "SELECT count(*) FROM teacher_follows f JOIN users u ON u.id = f.student_id AND u.status = 'active'
                 WHERE f.teacher_id = ?1 AND f.created_at >= ?2",
                params![teacher_id, since],
                |r| r.get(0),
            )
            .map_err(db_err)
        })
    })
}

/// The least time between two "answers wait for you" notices of one exam (when the first was read in between).
pub(crate) const SUBMISSION_NOTICE_GAP_MS: i64 = 3_600_000;

/// A student has submitted an attempt of `exam_id` that holds written answers nobody has graded. The exam's OWNER (an
/// active teacher; an admin's exam has none - the admin has the sidebar badge) gets `submission_pending {title, count}`
/// linking to the grading sheet. **Throttle: at most one notice per exam per hour.** While an unread one exists it is
/// updated in place (`count` of folded submissions, moved to the top) and no second one is ever added; once it has been
/// read, the next submission starts a new notice only when the newest one is at least an hour old, otherwise nothing is
/// sent.
pub(crate) fn tell_submission_pending(conn: &Connection, exam_id: &str, now: i64) -> Res<()> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT a.teacher_id, a.title FROM assessments a JOIN users u ON u.id = a.teacher_id AND u.role = 'teacher' AND u.status = 'active' WHERE a.id = ?1",
            params![exam_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;
    let Some((owner, title)) = row else { return Ok(()) };
    let link = format!("/platform/grading/{exam_id}");
    upsert_unread(conn, &owner, "submission_pending", &link, now, |cur| {
        if cur.is_some() {
            return Ok(Fold::Write(json!({ "title": title, "count": next_count(&cur) })));
        }
        let newest: Option<i64> = conn
            .query_row("SELECT max(created_at) FROM notifications WHERE user_id = ?1 AND kind = 'submission_pending' AND link = ?2", params![owner, link], |r| r.get(0))
            .map_err(db_err)?;
        Ok(if newest.map_or(false, |t| now - t < SUBMISSION_NOTICE_GAP_MS) { Fold::Skip } else { Fold::Write(json!({ "title": title, "count": 1 })) })
    })
}

/// A grader wrote (new or changed) feedback on an answer of this attempt: the student gets ONE unread
/// `feedback_added {title}` per attempt, however many answers were commented - a later comment only moves it up.
pub(crate) fn tell_feedback_added(conn: &Connection, student_id: &str, attempt_id: &str, title: &str, now: i64) -> Res<()> {
    upsert_unread(conn, student_id, "feedback_added", &format!("/platform/attempts/{attempt_id}"), now, |_| Ok(Fold::Write(json!({ "title": title }))))
}

// ───────── listing ─────────

#[derive(Serialize, Debug)]
pub struct Notification {
    id: String,
    kind: String,
    data: Value,
    link: Option<String>,
    read: bool,
    created_at: i64,
}

#[derive(Serialize, Debug)]
pub struct NotificationList {
    /// Every unread notice of the user, whatever the filters of this request.
    pub(crate) unread: i64,
    pub(crate) items: Vec<Notification>,
    /// Pass back as `cursor` for the next page; `None` on the last page.
    pub(crate) next: Option<String>,
}

/// `GET /notifications` query. All optional: without any of it the first page of 20 comes back.
#[derive(Deserialize, Default, Debug, Clone)]
pub struct NotifQuery {
    limit: Option<i64>,
    /// Opaque `"<created_at>:<rowid>"` of the last row of the previous page.
    cursor: Option<String>,
    /// `exams` | `content` | `people` | `account`.
    group: Option<String>,
    /// `1` / `true`: only unread notices.
    unread: Option<String>,
}

const NOTIF_DEFAULT_LIMIT: i64 = 20;
const NOTIF_MAX_LIMIT: i64 = 50;

/// The SQL condition for a notification group. The strings are literals of this file (never request text).
fn group_condition(group: &str) -> Res<&'static str> {
    Ok(match group {
        "exams" => "kind IN ('new_assessment','assessment_graded','exam_closing','exam_results','score_changed','feedback_added','submission_pending','exam_admin_closed','exam_admin_archived','exam_admin_released')",
        "content" => "kind IN ('new_post','new_course','live_scheduled','live_cancelled','live_updated','new_review','content_unpublished','content_auto_hidden')",
        "people" => "kind IN ('new_enrollment','new_follower')",
        "account" => "(kind LIKE 'account\\_%' ESCAPE '\\' OR kind LIKE 'teaching\\_%' ESCAPE '\\')",
        _ => return Err(bad("invalid_group")),
    })
}

/// Strict `"<created_at>:<rowid>"`: two canonical non-negative integers. Anything else is a 400, never a guess.
fn parse_cursor(raw: &str) -> Res<(i64, i64)> {
    let invalid = || bad("invalid_cursor");
    let (a, b) = raw.split_once(':').ok_or_else(invalid)?;
    let (created, rowid) = (a.parse::<i64>().map_err(|_| invalid())?, b.parse::<i64>().map_err(|_| invalid())?);
    if created < 0 || rowid < 0 || format!("{created}:{rowid}") != raw {
        return Err(invalid());
    }
    Ok((created, rowid))
}

pub(crate) fn list_notifications(conn: &Connection, user_id: &str, q: &NotifQuery) -> Res<NotificationList> {
    let limit = q.limit.unwrap_or(NOTIF_DEFAULT_LIMIT).clamp(1, NOTIF_MAX_LIMIT);
    let unread_total: i64 = conn
        .query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND read_at IS NULL", params![user_id], |r| r.get(0))
        .map_err(db_err)?;
    let mut conds = vec!["user_id = ?1".to_string()];
    let mut args: Vec<rusqlite::types::Value> = vec![user_id.to_string().into()];
    if let Some(g) = q.group.as_deref().filter(|g| !g.is_empty()) {
        conds.push(group_condition(g)?.to_string());
    }
    if matches!(q.unread.as_deref(), Some("1" | "true")) {
        conds.push("read_at IS NULL".into());
    }
    if let Some(c) = q.cursor.as_deref().filter(|c| !c.is_empty()) {
        let (created, rowid) = parse_cursor(c)?;
        args.push(created.into());
        args.push(rowid.into());
        conds.push(format!("(created_at < ?{0} OR (created_at = ?{0} AND rowid < ?{1}))", args.len() - 1, args.len()));
    }
    let sql = format!(
        "SELECT rowid, id, kind, data, link, read_at, created_at FROM notifications WHERE {} ORDER BY created_at DESC, rowid DESC LIMIT {}",
        conds.join(" AND "),
        limit + 1 // one more than asked: its presence says there is a next page
    );
    let mut rows = conn
        .prepare(&sql)
        .map_err(db_err)?
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            let data: String = r.get(3)?;
            let rowid: i64 = r.get(0)?;
            Ok((
                rowid,
                Notification {
                    id: r.get(1)?,
                    kind: r.get(2)?,
                    data: serde_json::from_str(&data).unwrap_or(Value::Null),
                    link: r.get(4)?,
                    read: r.get::<_, Option<i64>>(5)?.is_some(),
                    created_at: r.get(6)?,
                },
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next = if more { rows.last().map(|(rowid, n)| format!("{}:{}", n.created_at, rowid)) } else { None };
    Ok(NotificationList { unread: unread_total, items: rows.into_iter().map(|(_, n)| n).collect(), next })
}

pub async fn notifications_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<NotifQuery>) -> Res<Json<NotificationList>> {
    let user = require_active(&state, &headers)?;
    list_notifications(&*lock(&state)?, &user.id, &q).map(Json)
}

#[derive(Deserialize)]
pub struct ReadReq {
    ids: Option<Vec<String>>,
}

fn mark_read(conn: &Connection, user_id: &str, ids: &Option<Vec<String>>) -> Res<()> {
    let now = now_ms();
    match ids {
        Some(ids) if !ids.is_empty() => {
            for id in ids.iter().take(100) {
                conn.execute("UPDATE notifications SET read_at = ?1 WHERE id = ?2 AND user_id = ?3 AND read_at IS NULL", params![now, id, user_id])
                    .map_err(db_err)?;
            }
        }
        _ => {
            conn.execute("UPDATE notifications SET read_at = ?1 WHERE user_id = ?2 AND read_at IS NULL", params![now, user_id]).map_err(db_err)?;
        }
    }
    Ok(())
}

pub async fn mark_read_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<ReadReq>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    mark_read(&*lock(&state)?, &user.id, &r.ids)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── lesson progress ─────────

/// The lesson's course must be visible to the student (published, teacher active+approved) and the
/// student enrolled in the course's subject.
fn lesson_accessible(conn: &Connection, student_id: &str, lesson_id: &str) -> Res<bool> {
    // `visible` = published, teacher active and approved for the subject, subject AND its institution active
    conn.query_row(
        &format!(
            "SELECT EXISTS(
               SELECT 1 FROM lessons l JOIN courses c ON c.id = l.course_id
               JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id
               WHERE l.id = ?1 AND {}
                 AND EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?2 AND e.subject_id = c.subject_id))",
            crate::platform_content::visible("c", "published")
        ),
        params![lesson_id, student_id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

fn set_complete(conn: &Connection, student: &User, lesson_id: &str, done: bool) -> Res<()> {
    if !lesson_accessible(conn, &student.id, lesson_id)? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    if done {
        conn.execute("INSERT OR IGNORE INTO lesson_progress(student_id, lesson_id, completed_at) VALUES (?1,?2,?3)", params![student.id, lesson_id, now_ms()])
            .map_err(db_err)?;
    } else {
        conn.execute("DELETE FROM lesson_progress WHERE student_id = ?1 AND lesson_id = ?2", params![student.id, lesson_id]).map_err(db_err)?;
    }
    Ok(())
}

pub async fn complete_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    set_complete(&*lock(&state)?, &user, &id, true)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn uncomplete_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    set_complete(&*lock(&state)?, &user, &id, false)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct CourseProgress {
    completed: Vec<String>,
}

pub async fn course_progress_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(course_id): Path<String>) -> Res<Json<CourseProgress>> {
    let user = require_role(&state, &headers, "student")?;
    let conn = lock(&state)?;
    let completed = conn
        .prepare("SELECT p.lesson_id FROM lesson_progress p JOIN lessons l ON l.id = p.lesson_id WHERE p.student_id = ?1 AND l.course_id = ?2")
        .map_err(db_err)?
        .query_map(params![user.id, course_id], |r| r.get(0))
        .map_err(db_err)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(db_err)?;
    Ok(Json(CourseProgress { completed }))
}

#[derive(Serialize, Debug)]
pub struct ProgressItem {
    course_id: String,
    title: String,
    subject_name: String,
    total: i64,
    completed: i64,
}

fn my_progress(conn: &Connection, student_id: &str) -> Res<Vec<ProgressItem>> {
    // only courses the student can still open: published, teacher in good standing, subject and institution active
    conn.prepare(&format!(
        "SELECT c.id, c.title, s.name_ar,
                (SELECT count(*) FROM lessons l WHERE l.course_id = c.id),
                (SELECT count(*) FROM lesson_progress p JOIN lessons l ON l.id = p.lesson_id WHERE l.course_id = c.id AND p.student_id = ?1),
                (SELECT max(p.completed_at) FROM lesson_progress p JOIN lessons l ON l.id = p.lesson_id WHERE l.course_id = c.id AND p.student_id = ?1) AS last
         FROM courses c JOIN subjects s ON s.id = c.subject_id JOIN users u ON u.id = c.teacher_id
         WHERE {}
           AND EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?1 AND e.subject_id = c.subject_id)
           AND last IS NOT NULL
         ORDER BY last DESC LIMIT 20",
        crate::platform_content::visible("c", "published")
    ))
    .map_err(db_err)?
    .query_map(params![student_id], |r| {
        Ok(ProgressItem { course_id: r.get(0)?, title: r.get(1)?, subject_name: r.get(2)?, total: r.get(3)?, completed: r.get(4)? })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

pub async fn my_progress_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Vec<ProgressItem>>> {
    let user = require_role(&state, &headers, "student")?;
    my_progress(&*lock(&state)?, &user.id).map(Json)
}

// ───────── reviews ─────────

#[derive(Serialize, Debug, Clone)]
pub struct Review {
    id: String,
    rating: i64,
    comment: Option<String>,
    student_name: String,
    mine: bool,
    created_at: i64,
}

#[derive(Serialize, Debug)]
pub struct ReviewSummary {
    average: f64,
    count: i64,
    can_review: bool,
    mine: Option<Review>,
    items: Vec<Review>,
}

/// Privacy: students appear by first name only.
fn first_name(full: &str) -> String {
    full.split_whitespace().next().unwrap_or("").to_string()
}

/// Only students enrolled in the subject (course) or in a subject the teacher is approved for may review.
fn can_review(conn: &Connection, student_id: &str, target_type: &str, target_id: &str) -> Res<bool> {
    let sql = match target_type {
        // a course can be reviewed while students can see it (published, teacher in good standing, subject and
        // institution active) and only by someone enrolled in its subject
        "course" => format!(
            "SELECT EXISTS(SELECT 1 FROM courses c JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id
              JOIN subject_enrollments e ON e.subject_id = c.subject_id AND e.student_id = ?1
              WHERE c.id = ?2 AND {})",
            crate::platform_content::visible("c", "published")
        ),
        // a teacher can be reviewed through an enrolment the student can still see: the subject and its institution
        // must be active (a hidden institution is hidden everywhere, reviews included)
        "teacher" => format!(
            "SELECT EXISTS(SELECT 1 FROM teacher_subjects ts JOIN subjects s ON s.id = ts.subject_id
              JOIN subject_enrollments e ON e.subject_id = ts.subject_id AND e.student_id = ?1
              JOIN users u ON u.id = ts.teacher_id AND u.role = 'teacher' AND u.status = 'active'
              WHERE ts.teacher_id = ?2 AND ts.status = 'approved' AND {SUBJECT_ACTIVE} AND {INSTITUTION_ACTIVE})"
        ),
        _ => return Err(bad("invalid_target")),
    };
    conn.query_row(&sql, params![student_id, target_id], |r| r.get(0)).map_err(db_err)
}

/// Takes away the "new review" notices that quote reviews which are about to be deleted (the notice carries the first
/// 80 characters of the comment, which must not outlive the comment). `reviews_sql` selects those review ids and takes
/// one parameter; callers pass a literal of their own, never request text. Run it BEFORE the reviews are deleted.
pub(crate) fn purge_review_notices(conn: &Connection, reviews_sql: &str, param: &str) -> Res<()> {
    conn.execute(
        &format!("DELETE FROM notifications WHERE kind = 'new_review' AND json_extract(data, '$.review') IN ({reviews_sql})"),
        params![param],
    )
    .map_err(db_err)?;
    Ok(())
}

fn review_owner(conn: &Connection, target_type: &str, target_id: &str) -> Res<Option<(String, String)>> {
    // -> (teacher_id, link) for notifications; None when the target does not exist
    match target_type {
        "course" => conn
            .query_row("SELECT teacher_id FROM courses WHERE id = ?1", params![target_id], |r| r.get::<_, String>(0))
            .optional()
            .map_err(db_err)
            .map(|o| o.map(|t| (t, format!("/platform/courses/{target_id}")))),
        "teacher" => conn
            .query_row("SELECT id FROM users WHERE id = ?1 AND role = 'teacher'", params![target_id], |r| r.get::<_, String>(0))
            .optional()
            .map_err(db_err)
            .map(|o| o.map(|t| (t, format!("/platform/teachers/{target_id}")))),
        _ => Err(bad("invalid_target")),
    }
}

#[derive(Deserialize)]
pub struct ReviewReq {
    target_type: String,
    target_id: String,
    rating: i64,
    comment: Option<String>,
}

fn upsert_review(conn: &Connection, student: &User, r: &ReviewReq) -> Res<()> {
    if !(1..=5).contains(&r.rating) {
        return Err(bad("invalid_rating"));
    }
    let comment = opt_text(&r.comment, 1000, "comment_too_long")?;
    let (teacher_id, link) = review_owner(conn, &r.target_type, &r.target_id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if !can_review(conn, &student.id, &r.target_type, &r.target_id)? {
        return Err(err(StatusCode::FORBIDDEN, "not_enrolled"));
    }
    let now = now_ms();
    let existed: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM reviews WHERE student_id = ?1 AND target_type = ?2 AND target_id = ?3)",
            params![student.id, r.target_type, r.target_id],
            |x| x.get(0),
        )
        .map_err(db_err)?;
    conn.execute(
        "INSERT INTO reviews(id, student_id, target_type, target_id, rating, comment, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7)
         ON CONFLICT(student_id, target_type, target_id) DO UPDATE SET rating = excluded.rating, comment = excluded.comment, updated_at = excluded.updated_at",
        params![new_id(), student.id, r.target_type, r.target_id, r.rating, comment, now],
    )
    .map_err(db_err)?;
    if !existed {
        // what the teacher needs to see without opening anything: which course (empty for a review of the teacher) and
        // how it starts (plain text, 80 characters at most - rendered as text by the client, never as markup).
        // `review` ties the notice to its review, so that deleting the review takes the quoted comment away too.
        let review_id: String = conn
            .query_row(
                "SELECT id FROM reviews WHERE student_id = ?1 AND target_type = ?2 AND target_id = ?3",
                params![student.id, r.target_type, r.target_id],
                |x| x.get(0),
            )
            .map_err(db_err)?;
        let title: String = if r.target_type == "course" {
            conn.query_row("SELECT title FROM courses WHERE id = ?1", params![r.target_id], |x| x.get(0)).optional().map_err(db_err)?.unwrap_or_default()
        } else {
            String::new()
        };
        let comment: String = comment.as_deref().unwrap_or("").chars().take(80).collect();
        notify(conn, &teacher_id, "new_review", json!({ "rating": r.rating, "target": r.target_type, "title": title, "comment": comment, "review": review_id }), &link);
    }
    Ok(())
}

fn review_summary(conn: &Connection, viewer: &User, target_type: &str, target_id: &str) -> Res<ReviewSummary> {
    if target_type != "course" && target_type != "teacher" {
        return Err(bad("invalid_target"));
    }
    if target_type == "course" && viewer.role != "admin" {
        // the ratings, comments and reviewer names of a course are shown only while students can see the course
        // (published, teacher in good standing, subject and institution active) - or to its owner
        let owner: Option<String> =
            conn.query_row("SELECT teacher_id FROM courses WHERE id = ?1", params![target_id], |r| r.get(0)).optional().map_err(db_err)?;
        let shown = owner.as_deref() == Some(viewer.id.as_str()) || crate::platform_content::visible_to_public(conn, "courses", "c", target_id, "published")?;
        if !shown {
            return Err(err(StatusCode::NOT_FOUND, "not_found"));
        }
    }
    let (average, count): (Option<f64>, i64) = conn
        .query_row(
            "SELECT avg(rating), count(*) FROM reviews WHERE target_type = ?1 AND target_id = ?2",
            params![target_type, target_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(db_err)?;
    let items = conn
        .prepare(
            "SELECT r.id, r.rating, r.comment, u.full_name, r.student_id, r.created_at FROM reviews r JOIN users u ON u.id = r.student_id
             WHERE r.target_type = ?1 AND r.target_id = ?2 ORDER BY r.updated_at DESC LIMIT 30",
        )
        .map_err(db_err)?
        .query_map(params![target_type, target_id], |r| {
            Ok(Review {
                id: r.get(0)?,
                rating: r.get(1)?,
                comment: r.get(2)?,
                student_name: first_name(&r.get::<_, String>(3)?),
                mine: r.get::<_, String>(4)? == viewer.id,
                created_at: r.get(5)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let mine = items.iter().find(|r| r.mine).cloned();
    let eligible = viewer.role == "student" && can_review(conn, &viewer.id, target_type, target_id)?;
    Ok(ReviewSummary { average: (average.unwrap_or(0.0) * 10.0).round() / 10.0, count, can_review: eligible, mine, items })
}

fn delete_review(conn: &Connection, user: &User, id: &str) -> Res<()> {
    let owner: String = conn
        .query_row("SELECT student_id FROM reviews WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if owner != user.id && user.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    purge_review_notices(conn, "SELECT id FROM reviews WHERE id = ?1", id)?;
    conn.execute("DELETE FROM reviews WHERE id = ?1", params![id]).map_err(db_err)?;
    if owner != user.id {
        crate::platform::audit(conn, &user.id, id, "review_deleted", "");
    }
    Ok(())
}

pub async fn put_review_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<ReviewReq>) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    upsert_review(&*lock(&state)?, &user, &r)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reviews_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path((t, id)): Path<(String, String)>) -> Res<Json<ReviewSummary>> {
    let user = require_active(&state, &headers)?;
    review_summary(&*lock(&state)?, &user, &t, &id).map(Json)
}

pub async fn delete_review_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    delete_review(&*lock(&state)?, &user, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    struct W {
        conn: Connection,
        teacher: User,
        student: User,
        outsider: User,
        admin: User,
    }

    fn world() -> W {
        let conn = create_test_db();
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','ج',1,0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0)", []).unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        let outsider = insert_test_user(&conn, "o@x.com", "student", "active");
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO subject_enrollments VALUES (?1,'s1',0)", params![student.id]).unwrap();
        conn.execute("INSERT INTO courses VALUES ('c1', ?1, 's1', 'دورة', NULL, 'published', 0, 0, NULL)", params![teacher.id]).unwrap();
        for (i, n) in ["l1", "l2", "l3"].iter().enumerate() {
            conn.execute("INSERT INTO lessons VALUES (?1,'c1',?2,NULL,'د',NULL,'https://x.com/a.mp4','https://x.com/a.mp4','file',0)", params![n, i as i64 + 1]).unwrap();
        }
        W { conn, teacher, student, outsider, admin }
    }

    #[test]
    fn progress_requires_enrollment_and_tracks_percent() {
        let w = world();
        assert_eq!(set_complete(&w.conn, &w.outsider, "l1", true).unwrap_err().0, StatusCode::NOT_FOUND, "not enrolled");
        assert_eq!(set_complete(&w.conn, &w.student, "nope", true).unwrap_err().0, StatusCode::NOT_FOUND);
        set_complete(&w.conn, &w.student, "l1", true).unwrap();
        set_complete(&w.conn, &w.student, "l1", true).unwrap(); // idempotent
        set_complete(&w.conn, &w.student, "l2", true).unwrap();
        let p = &my_progress(&w.conn, &w.student.id).unwrap()[0];
        assert_eq!((p.completed, p.total), (2, 3));
        set_complete(&w.conn, &w.student, "l2", false).unwrap();
        assert_eq!(my_progress(&w.conn, &w.student.id).unwrap()[0].completed, 1);
        assert!(my_progress(&w.conn, &w.outsider.id).unwrap().is_empty());
        w.conn.execute("UPDATE courses SET status = 'draft'", []).unwrap();
        assert_eq!(set_complete(&w.conn, &w.student, "l3", true).unwrap_err().0, StatusCode::NOT_FOUND, "unpublished course");
    }

    #[test]
    fn reviews_are_limited_to_enrolled_students_and_upsert() {
        let w = world();
        let req = |t: &str, id: &str, rating: i64| ReviewReq { target_type: t.into(), target_id: id.into(), rating, comment: Some("ممتاز".into()) };
        assert_eq!(upsert_review(&w.conn, &w.outsider, &req("course", "c1", 5)).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(upsert_review(&w.conn, &w.student, &req("course", "c1", 6)).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(upsert_review(&w.conn, &w.student, &req("course", "nope", 5)).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(upsert_review(&w.conn, &w.student, &req("bogus", "c1", 5)).unwrap_err().0, StatusCode::BAD_REQUEST);
        upsert_review(&w.conn, &w.student, &req("course", "c1", 4)).unwrap();
        upsert_review(&w.conn, &w.student, &req("course", "c1", 2)).unwrap(); // update, not a second row
        upsert_review(&w.conn, &w.student, &req("teacher", &w.teacher.id, 5)).unwrap();
        let s = review_summary(&w.conn, &w.student, "course", "c1").unwrap();
        assert_eq!((s.count, s.average), (1, 2.0));
        assert!(s.mine.is_some() && s.can_review);
        let o = review_summary(&w.conn, &w.outsider, "course", "c1").unwrap();
        assert!(!o.can_review && o.mine.is_none());
        let n: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = 'new_review'", params![w.teacher.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 2, "one per new review (course + teacher), none for the update");
    }

    #[test]
    fn review_names_are_first_name_only_and_deletable_by_owner_or_admin() {
        let w = world();
        w.conn.execute("UPDATE users SET full_name = 'سارة أحمد العلي' WHERE id = ?1", params![w.student.id]).unwrap();
        upsert_review(&w.conn, &w.student, &ReviewReq { target_type: "teacher".into(), target_id: w.teacher.id.clone(), rating: 5, comment: None }).unwrap();
        let s = review_summary(&w.conn, &w.admin, "teacher", &w.teacher.id).unwrap();
        assert_eq!(s.items[0].student_name, "سارة");
        let id = s.items[0].id.clone();
        assert_eq!(delete_review(&w.conn, &w.outsider, &id).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(delete_review(&w.conn, &w.teacher, &id).unwrap_err().0, StatusCode::FORBIDDEN, "teachers cannot delete their reviews");
        delete_review(&w.conn, &w.admin, &id).unwrap();
        assert_eq!(review_summary(&w.conn, &w.admin, "teacher", &w.teacher.id).unwrap().count, 0);
    }

    #[test]
    fn notifications_fan_out_to_followers_and_enrolled_only() {
        let w = world();
        let follower = insert_test_user(&w.conn, "f@x.com", "student", "active");
        let suspended = insert_test_user(&w.conn, "z@x.com", "student", "suspended");
        w.conn.execute("INSERT INTO teacher_follows VALUES (?1, ?2, 0)", params![follower.id, w.teacher.id]).unwrap();
        w.conn.execute("INSERT INTO teacher_follows VALUES (?1, ?2, 0)", params![suspended.id, w.teacher.id]).unwrap();
        // student is enrolled AND (also) follows -> must still get exactly one
        w.conn.execute("INSERT INTO teacher_follows VALUES (?1, ?2, 0)", params![w.student.id, w.teacher.id]).unwrap();
        notify_audience(&w.conn, &w.teacher.id, "s1", "new_post", json!({"title": "t"}), "/platform/posts/x");
        let count = |u: &User| list_notifications(&w.conn, &u.id, &NotifQuery::default()).unwrap().items.len();
        assert_eq!((count(&w.student), count(&follower), count(&suspended), count(&w.outsider), count(&w.teacher)), (1, 1, 0, 0, 0));
        let list = list_notifications(&w.conn, &w.student.id, &NotifQuery::default()).unwrap();
        assert_eq!(list.unread, 1);
        mark_read(&w.conn, &w.outsider.id, &None).unwrap(); // cannot touch others'
        assert_eq!(list_notifications(&w.conn, &w.student.id, &NotifQuery::default()).unwrap().unread, 1);
        mark_read(&w.conn, &w.student.id, &Some(vec![list.items[0].id.clone()])).unwrap();
        assert_eq!(list_notifications(&w.conn, &w.student.id, &NotifQuery::default()).unwrap().unread, 0);
        assert_eq!(list_notifications(&w.conn, &w.student.id, &NotifQuery::default()).unwrap().items[0].data["title"], "t");
    }

    // ───── phase 3-2: a hidden institution takes the course out of reach ─────

    #[test]
    fn a_hidden_institution_closes_the_lessons_the_progress_list_and_course_reviews() {
        let w = world();
        let review = ReviewReq { target_type: "course".into(), target_id: "c1".into(), rating: 4, comment: None };
        set_complete(&w.conn, &w.student, "l1", true).unwrap();
        assert!(lesson_accessible(&w.conn, &w.student.id, "l2").unwrap());
        assert_eq!(my_progress(&w.conn, &w.student.id).unwrap().len(), 1);
        assert!(review_summary(&w.conn, &w.student, "course", "c1").unwrap().can_review);

        w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        assert!(!lesson_accessible(&w.conn, &w.student.id, "l2").unwrap());
        assert_eq!(set_complete(&w.conn, &w.student, "l2", true).unwrap_err().0, StatusCode::NOT_FOUND, "no new progress in a hidden institution");
        assert_eq!(set_complete(&w.conn, &w.student, "l1", false).unwrap_err().0, StatusCode::NOT_FOUND, "nor can it be un-ticked");
        assert!(my_progress(&w.conn, &w.student.id).unwrap().is_empty(), "the course leaves the student's progress list");
        assert_eq!(upsert_review(&w.conn, &w.student, &review).unwrap_err().0, StatusCode::FORBIDDEN);
        // the ratings, comments and reviewer names of the hidden course are no longer readable by a student either
        assert_eq!(review_summary(&w.conn, &w.student, "course", "c1").unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(review_summary(&w.conn, &w.outsider, "course", "c1").unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(review_summary(&w.conn, &w.teacher, "course", "c1").unwrap().count, 0, "the owner still sees their own course");
        assert_eq!(review_summary(&w.conn, &w.admin, "course", "c1").unwrap().count, 0, "and so does an admin (moderation)");
        let kept: i64 = w.conn.query_row("SELECT count(*) FROM lesson_progress", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, 1, "what the student already did is kept");

        w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        assert_eq!(my_progress(&w.conn, &w.student.id).unwrap()[0].completed, 1, "and is there again when the institution is");
        set_complete(&w.conn, &w.student, "l2", true).unwrap();
        // the same door closes for the other reasons `visible()` knows about
        w.conn.execute("UPDATE users SET role = 'student' WHERE id = ?1", params![w.teacher.id]).unwrap();
        assert!(!lesson_accessible(&w.conn, &w.student.id, "l3").unwrap(), "a teacher whose role changed no longer serves courses");
    }

    #[test]
    fn a_hidden_course_or_institution_closes_reviews_for_students() {
        let w = world();
        let rev = |t: &str, id: &str| ReviewReq { target_type: t.into(), target_id: id.into(), rating: 5, comment: Some("جيد".into()) };
        upsert_review(&w.conn, &w.student, &rev("course", "c1")).unwrap();
        assert_eq!(review_summary(&w.conn, &w.outsider, "course", "c1").unwrap().items.len(), 1, "a visible course shows its reviews");
        // a draft course: reviews are the owner's and the admin's business only
        w.conn.execute("UPDATE courses SET status = 'draft' WHERE id = 'c1'", []).unwrap();
        assert_eq!(review_summary(&w.conn, &w.student, "course", "c1").unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(review_summary(&w.conn, &w.teacher, "course", "c1").unwrap().items.len(), 1);
        w.conn.execute("UPDATE courses SET status = 'published' WHERE id = 'c1'", []).unwrap();
        // an id that does not exist is a 404, never an empty (information-free but different) summary
        assert_eq!(review_summary(&w.conn, &w.student, "course", "nope").unwrap_err().0, StatusCode::NOT_FOUND);

        // a teacher can be reviewed only through an enrolment in a subject whose institution (and the subject) is active
        assert!(can_review(&w.conn, &w.student.id, "teacher", &w.teacher.id).unwrap());
        w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        assert!(!can_review(&w.conn, &w.student.id, "teacher", &w.teacher.id).unwrap(), "hidden institution");
        assert_eq!(upsert_review(&w.conn, &w.student, &rev("teacher", &w.teacher.id)).unwrap_err().0, StatusCode::FORBIDDEN);
        let before: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'new_review'", [], |r| r.get(0)).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
        assert!(!can_review(&w.conn, &w.student.id, "teacher", &w.teacher.id).unwrap(), "hidden subject");
        // an enrolment in a second, still visible subject keeps the door open
        w.conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s2','i1','أخرى',1,0)", []).unwrap();
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s2','approved',0,0)", params![w.teacher.id]).unwrap();
        w.conn.execute("INSERT INTO subject_enrollments VALUES (?1,'s2',0)", params![w.student.id]).unwrap();
        assert!(can_review(&w.conn, &w.student.id, "teacher", &w.teacher.id).unwrap(), "an enrolment that is still visible keeps the door open");
        let after: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'new_review'", [], |r| r.get(0)).unwrap();
        assert_eq!(before, after);
    }

    // ───── phase 3-4: paging, groups, coalescing, the review preview ─────

    fn put(conn: &Connection, user: &User, kind: &str, created_at: i64, read: bool) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO notifications(id, user_id, kind, data, link, read_at, created_at) VALUES (?1,?2,?3,'{}',NULL,?4,?5)",
            params![id, user.id, kind, read.then_some(1_i64), created_at],
        )
        .unwrap();
        id
    }

    fn page(w: &W, user: &User, f: impl FnOnce(&mut NotifQuery)) -> NotificationList {
        let mut q = NotifQuery::default();
        f(&mut q);
        list_notifications(&w.conn, &user.id, &q).unwrap()
    }

    fn ids(l: &NotificationList) -> Vec<String> {
        l.items.iter().map(|n| n.id.clone()).collect()
    }

    #[test]
    fn pages_follow_the_cursor_in_a_stable_order_even_when_every_timestamp_is_equal() {
        let w = world();
        // 45 notices in the same millisecond: only the row number can order them
        let all: Vec<String> = (0..45).map(|_| put(&w.conn, &w.student, "new_post", 1_000, false)).collect();
        let newest_first: Vec<String> = all.iter().rev().cloned().collect();
        let mut seen: Vec<String> = vec![];
        let mut cursor: Option<String> = None;
        let mut sizes = vec![];
        loop {
            let l = page(&w, &w.student, |q| q.cursor = cursor.clone());
            sizes.push(l.items.len());
            seen.extend(ids(&l));
            assert_eq!(l.unread, 45, "the unread total does not depend on the page");
            match l.next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(sizes, [20, 20, 5], "pages of 20 by default");
        assert_eq!(seen, newest_first, "every notice exactly once, newest first, ties by insertion order");
        // an exact multiple has no phantom last page: `next` is null on the last full page
        let w2 = world();
        for _ in 0..40 {
            put(&w2.conn, &w2.student, "new_post", 1_000, false);
        }
        let first = page(&w2, &w2.student, |_| {});
        let second = page(&w2, &w2.student, |q| q.cursor = first.next.clone());
        assert_eq!((first.items.len(), second.items.len(), first.next.is_some(), second.next), (20, 20, true, None));
        // a different created_at orders before the row number; a newer one comes first
        let newer = put(&w.conn, &w.student, "new_course", 2_000, false);
        let top = page(&w, &w.student, |_| {});
        assert_eq!(top.items[0].id, newer);
        // limits: default 20, at most 50, at least 1
        assert_eq!(page(&w, &w.student, |q| q.limit = Some(500)).items.len(), 46.min(50));
        assert_eq!(page(&w, &w.student, |q| q.limit = Some(0)).items.len(), 1);
        assert_eq!(page(&w, &w.student, |q| q.limit = Some(-7)).items.len(), 1);
        let l = page(&w, &w.student, |q| q.limit = Some(7));
        assert_eq!((l.items.len(), l.next.is_some()), (7, true));
        // the cursor encodes where the page ended: "<created_at>:<rowid>"
        let (c, r) = l.next.as_deref().unwrap().split_once(':').unwrap();
        assert_eq!(c.parse::<i64>().unwrap(), 1_000);
        assert!(r.parse::<i64>().unwrap() > 0);
    }

    #[test]
    fn a_garbage_cursor_is_a_400_never_a_guess() {
        let w = world();
        put(&w.conn, &w.student, "new_post", 1_000, false);
        for bad_cursor in ["x", "1", "1:", ":1", "a:b", "1:2:3", "-1:5", "5:-1", "01:2", "+1:2", " 1:2", "1: 2", "1:2 ", "1.5:2", "0x10:2", "99999999999999999999:1", "1;DROP TABLE users:1"] {
            let e = list_notifications(&w.conn, &w.student.id, &NotifQuery { cursor: Some(bad_cursor.into()), ..Default::default() }).unwrap_err();
            assert_eq!((e.0, e.1.contains("invalid_cursor")), (StatusCode::BAD_REQUEST, true), "{bad_cursor:?}");
        }
        // an absent or empty cursor is simply the first page; a well-formed one that points past everything is an empty page
        assert_eq!(page(&w, &w.student, |q| q.cursor = Some(String::new())).items.len(), 1);
        let end = page(&w, &w.student, |q| q.cursor = Some("0:0".into()));
        assert_eq!((end.items.len(), end.next), (0, None));
    }

    #[test]
    fn groups_map_kinds_on_the_server_and_an_unknown_group_is_a_400() {
        let w = world();
        let groups: [(&str, &[&str]); 4] = [
            ("exams", &["new_assessment", "assessment_graded", "exam_closing", "exam_results", "score_changed", "feedback_added", "submission_pending", "exam_admin_closed", "exam_admin_archived", "exam_admin_released"]),
            ("content", &["new_post", "new_course", "live_scheduled", "live_cancelled", "live_updated", "new_review", "content_unpublished", "content_auto_hidden"]),
            ("people", &["new_enrollment", "new_follower"]),
            ("account", &["account_active", "account_rejected", "account_suspended", "teaching_approved", "teaching_rejected", "teaching_revoked"]),
        ];
        for (_, kinds) in &groups {
            for k in *kinds {
                put(&w.conn, &w.student, k, 1_000, false);
            }
        }
        // kinds that belong to no group, and look-alikes of the account prefixes, show up only in the unfiltered list
        for k in ["something_new", "accountant", "teachingx", "newsletter"] {
            put(&w.conn, &w.student, k, 1_000, false);
        }
        let kinds_of = |l: &NotificationList| -> std::collections::BTreeSet<String> { l.items.iter().map(|n| n.kind.clone()).collect() };
        for (g, kinds) in &groups {
            let l = page(&w, &w.student, |q| { q.group = Some((*g).into()); q.limit = Some(50); });
            assert_eq!(kinds_of(&l), kinds.iter().map(|s| s.to_string()).collect(), "group {g}");
            assert_eq!(l.unread, (groups.iter().map(|(_, k)| k.len()).sum::<usize>() + 4) as i64, "the unread total ignores the group");
        }
        assert_eq!(page(&w, &w.student, |q| q.limit = Some(50)).items.len(), 26 + 4);
        assert_eq!(page(&w, &w.student, |q| q.group = Some(String::new())).items.len(), 20, "an empty group is no group");
        for bad_group in ["Exams", "everything", "exams,content", "exams'--", "*"] {
            let e = list_notifications(&w.conn, &w.student.id, &NotifQuery { group: Some(bad_group.into()), ..Default::default() }).unwrap_err();
            assert_eq!((e.0, e.1.contains("invalid_group")), (StatusCode::BAD_REQUEST, true), "{bad_group:?}");
        }
        // a group pages like the whole list
        let p1 = page(&w, &w.student, |q| { q.group = Some("exams".into()); q.limit = Some(4); });
        let p2 = page(&w, &w.student, |q| { q.group = Some("exams".into()); q.limit = Some(4); q.cursor = p1.next.clone(); });
        assert!(p1.items.iter().chain(&p2.items).all(|n| groups[0].1.contains(&n.kind.as_str())) && p2.next.is_some());
    }

    #[test]
    fn the_unread_filter_keeps_unread_ones_while_the_unread_total_stays_the_whole_truth() {
        let w = world();
        let read: Vec<String> = (0..3).map(|i| put(&w.conn, &w.student, "new_post", 100 + i, true)).collect();
        let unread: Vec<String> = (0..4).map(|i| put(&w.conn, &w.student, "new_course", 200 + i, false)).collect();
        put(&w.conn, &w.student, "new_enrollment", 300, false);
        let only = page(&w, &w.student, |q| q.unread = Some("1".into()));
        assert_eq!((only.items.len(), only.unread), (5, 5));
        assert!(only.items.iter().all(|n| !n.read));
        assert!(unread.iter().all(|u| ids(&only).contains(u)) && read.iter().all(|r| !ids(&only).contains(r)));
        let people_unread = page(&w, &w.student, |q| { q.unread = Some("true".into()); q.group = Some("people".into()); });
        assert_eq!((people_unread.items.len(), people_unread.unread), (1, 5), "the total is not the filtered count");
        let all = page(&w, &w.student, |q| q.unread = Some("0".into()));
        assert_eq!((all.items.len(), all.unread), (8, 5), "anything but 1/true leaves the read ones in");
        // paging through the unread ones only
        let p1 = page(&w, &w.student, |q| { q.unread = Some("1".into()); q.limit = Some(2); });
        let p2 = page(&w, &w.student, |q| { q.unread = Some("1".into()); q.limit = Some(2); q.cursor = p1.next.clone(); });
        let p3 = page(&w, &w.student, |q| { q.unread = Some("1".into()); q.limit = Some(2); q.cursor = p2.next.clone(); });
        assert_eq!((p1.items.len(), p2.items.len(), p3.items.len(), p3.next), (2, 2, 1, None));
        // and the notices of one user never show up for another
        put(&w.conn, &w.outsider, "new_post", 999, false);
        assert!(!page(&w, &w.student, |q| q.limit = Some(50)).items.iter().any(|n| n.created_at == 999));
        assert_eq!(page(&w, &w.outsider, |_| {}).unread, 1);
    }

    #[test]
    fn a_client_that_sends_no_query_still_gets_the_newest_twenty_with_the_old_fields() {
        let w = world();
        for i in 0..30 {
            put(&w.conn, &w.student, "new_post", i, i % 2 == 0);
        }
        let l = list_notifications(&w.conn, &w.student.id, &NotifQuery::default()).unwrap();
        assert_eq!((l.items.len(), l.unread, l.items[0].created_at, l.items[19].created_at), (20, 15, 29, 10));
        let json = serde_json::to_value(&l).unwrap();
        assert!(json["items"].is_array() && json["unread"].is_number() && json["next"].is_string(), "{json}");
        for key in ["id", "kind", "data", "link", "read", "created_at"] {
            assert!(json["items"][0].get(key).is_some(), "{key}");
        }
        // mark-all-read is untouched by the paging
        mark_read(&w.conn, &w.student.id, &None).unwrap();
        assert_eq!(list_notifications(&w.conn, &w.student.id, &NotifQuery::default()).unwrap().unread, 0);
    }

    #[test]
    fn upsert_unread_updates_the_unread_notice_in_place_and_leaves_read_ones_alone() {
        let w = world();
        let get = |kind: &str| -> Vec<(String, Value, bool, i64)> {
            w.conn
                .prepare("SELECT id, data, read_at, created_at FROM notifications WHERE user_id = ?1 AND kind = ?2 ORDER BY rowid")
                .unwrap()
                .query_map(params![w.student.id, kind], |r| Ok((r.get(0)?, serde_json::from_str(&r.get::<_, String>(1)?).unwrap(), r.get::<_, Option<i64>>(2)?.is_some(), r.get(3)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let write = |kind: &str, link: &str, now: i64, v: i64| upsert_unread(&w.conn, &w.student.id, kind, link, now, |cur| Ok(Fold::Write(json!({ "v": v, "before": cur.map(|c| c["v"].clone()) })))).unwrap();
        write("k", "/a", 10, 1);
        let first = get("k");
        assert_eq!((first.len(), first[0].1["v"].as_i64(), first[0].1["before"].is_null(), first[0].3), (1, Some(1), true, 10));
        write("k", "/a", 20, 2);
        let second = get("k");
        assert_eq!((second.len(), second[0].0.clone(), second[0].1["before"].as_i64(), second[0].3), (1, first[0].0.clone(), Some(1), 20), "same row, folded, moved to the new time");
        // another link or kind is another notice
        write("k", "/b", 30, 3);
        write("other", "/a", 30, 4);
        assert_eq!((get("k").len(), get("other").len()), (2, 1));
        // a read notice is history: the next write starts a new one, the read one is untouched
        w.conn.execute("UPDATE notifications SET read_at = 99 WHERE id = ?1", params![first[0].0]).unwrap();
        write("k", "/a", 40, 5);
        let rows: Vec<_> = get("k").into_iter().filter(|r| r.1["v"].as_i64() != Some(3)).collect();
        assert_eq!((rows.len(), rows[0].2, rows[0].1["v"].as_i64(), rows[1].2, rows[1].1["v"].as_i64()), (2, true, Some(2), false, Some(5)));
        // Skip leaves everything; Remove takes the unread one away (and is a no-op without one)
        let before = get("k").len();
        upsert_unread(&w.conn, &w.student.id, "k", "/a", 50, |_| Ok(Fold::Skip)).unwrap();
        assert_eq!(get("k").len(), before);
        upsert_unread(&w.conn, &w.student.id, "k", "/a", 50, |_| Ok(Fold::Remove)).unwrap();
        assert_eq!(get("k").len(), before - 1);
        upsert_unread(&w.conn, &w.student.id, "k", "/a", 50, |cur| { assert!(cur.is_none(), "the read one is not offered"); Ok(Fold::Remove) }).unwrap();
        assert_eq!(get("k").len(), before - 1);
        // a failing fold writes nothing
        let e = upsert_unread(&w.conn, &w.student.id, "k", "/zzz", 60, |_| Err(bad("boom"))).unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
        assert_eq!(get("k").len(), before - 1);
    }

    #[test]
    fn deleting_a_review_takes_the_comment_the_teacher_was_shown_away_too() {
        let w = world();
        let second = insert_test_user(&w.conn, "s2@x.com", "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments VALUES (?1,'s1',0)", params![second.id]).unwrap();
        let req = |c: &str| ReviewReq { target_type: "course".into(), target_id: "c1".into(), rating: 2, comment: Some(c.into()) };
        upsert_review(&w.conn, &w.student, &req("سيئة جدا ورقمي 0501234567")).unwrap();
        upsert_review(&w.conn, &second, &req("جيدة")).unwrap();
        upsert_review(&w.conn, &w.student, &ReviewReq { target_type: "teacher".into(), target_id: w.teacher.id.clone(), rating: 4, comment: Some("معلم رائع".into()) }).unwrap();
        let quoted = |w: &W| -> Vec<String> {
            w.conn.prepare("SELECT data FROM notifications WHERE kind = 'new_review' ORDER BY rowid").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(quoted(&w).len(), 3);
        assert!(quoted(&w)[0].contains("\"review\":\""), "the notice names its review: {}", quoted(&w)[0]);
        // the author withdraws the first review (the teacher has already read the notice: it goes anyway)
        w.conn.execute("UPDATE notifications SET read_at = 3 WHERE kind = 'new_review'", []).unwrap();
        let mine: String = w.conn.query_row("SELECT id FROM reviews WHERE student_id = ?1 AND target_type = 'course'", params![w.student.id], |r| r.get(0)).unwrap();
        delete_review(&w.conn, &w.student, &mine).unwrap();
        let rest = quoted(&w);
        assert_eq!(rest.len(), 2);
        assert!(rest.iter().all(|d| !d.contains("0501234567") && !d.contains("سيئة")), "{rest:?}");
        // an admin's moderation does the same, and only for that review
        let theirs: String = w.conn.query_row("SELECT id FROM reviews WHERE student_id = ?1", params![second.id], |r| r.get(0)).unwrap();
        delete_review(&w.conn, &w.admin, &theirs).unwrap();
        let rest = quoted(&w);
        assert_eq!(rest.len(), 1);
        assert!(rest[0].contains("معلم رائع"), "the teacher review's notice stays while the review does: {rest:?}");
    }

    #[test]
    fn a_new_review_tells_the_teacher_which_course_and_how_the_comment_starts() {
        let w = world();
        let long: String = "ممتازة جدا".repeat(20);
        assert!(long.chars().count() > 80);
        upsert_review(&w.conn, &w.student, &ReviewReq { target_type: "course".into(), target_id: "c1".into(), rating: 5, comment: Some(format!("  {long}  ")) }).unwrap();
        upsert_review(&w.conn, &w.student, &ReviewReq { target_type: "teacher".into(), target_id: w.teacher.id.clone(), rating: 4, comment: None }).unwrap();
        let rows: Vec<(Value, String)> = w.conn
            .prepare("SELECT data, link FROM notifications WHERE user_id = ?1 AND kind = 'new_review' ORDER BY rowid")
            .unwrap()
            .query_map(params![w.teacher.id], |r| Ok((serde_json::from_str(&r.get::<_, String>(0)?).unwrap(), r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        let (course, teacher) = (&rows[0].0, &rows[1].0);
        assert_eq!((course["rating"].as_i64(), course["target"].as_str(), course["title"].as_str()), (Some(5), Some("course"), Some("دورة")));
        assert_eq!(course["comment"].as_str().map(|c| c.chars().count()), Some(80), "the first 80 characters, counted as characters");
        assert_eq!(course["comment"].as_str().unwrap(), long.chars().take(80).collect::<String>(), "trimmed, then cut");
        assert_eq!((teacher["rating"].as_i64(), teacher["target"].as_str(), teacher["title"].as_str(), teacher["comment"].as_str()), (Some(4), Some("teacher"), Some(""), Some("")));
        assert_eq!((rows[0].1.as_str(), rows[1].1.as_str()), ("/platform/courses/c1", format!("/platform/teachers/{}", w.teacher.id).as_str()), "the target link is unchanged");
        // a short comment is kept whole; an update of the same review sends nothing new
        let w2 = world();
        upsert_review(&w2.conn, &w2.student, &ReviewReq { target_type: "course".into(), target_id: "c1".into(), rating: 3, comment: Some("جيدة".into()) }).unwrap();
        upsert_review(&w2.conn, &w2.student, &ReviewReq { target_type: "course".into(), target_id: "c1".into(), rating: 1, comment: Some("سيئة".into()) }).unwrap();
        let d: String = w2.conn.query_row("SELECT data FROM notifications WHERE kind = 'new_review'", [], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&d).unwrap()["comment"].as_str(), Some("جيدة"));
        assert_eq!(w2.conn.query_row::<i64, _, _>("SELECT count(*) FROM notifications WHERE kind = 'new_review'", [], |r| r.get(0)).unwrap(), 1);
    }
}
