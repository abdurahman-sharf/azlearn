//! Student engagement: lesson progress, ratings & reviews, in-app notifications.
//! Other platform modules call `notify` / `notify_audience` when something worth telling happens.

use crate::platform::{bad, db_err, lock, new_id, opt_text, require_active, require_role, Res, User, INSTITUTION_ACTIVE, SUBJECT_ACTIVE};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, State},
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

#[derive(Serialize, Debug)]
pub struct Notification {
    id: String,
    kind: String,
    data: Value,
    link: Option<String>,
    read: bool,
    created_at: i64,
}

#[derive(Serialize)]
pub struct NotificationList {
    unread: i64,
    items: Vec<Notification>,
}

fn list_notifications(conn: &Connection, user_id: &str) -> Res<NotificationList> {
    let unread: i64 = conn
        .query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND read_at IS NULL", params![user_id], |r| r.get(0))
        .map_err(db_err)?;
    let items = conn
        .prepare("SELECT id, kind, data, link, read_at, created_at FROM notifications WHERE user_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 50")
        .map_err(db_err)?
        .query_map(params![user_id], |r| {
            let data: String = r.get(2)?;
            Ok(Notification {
                id: r.get(0)?,
                kind: r.get(1)?,
                data: serde_json::from_str(&data).unwrap_or(Value::Null),
                link: r.get(3)?,
                read: r.get::<_, Option<i64>>(4)?.is_some(),
                created_at: r.get(5)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(NotificationList { unread, items })
}

pub async fn notifications_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<NotificationList>> {
    let user = require_active(&state, &headers)?;
    list_notifications(&*lock(&state)?, &user.id).map(Json)
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
        notify(conn, &teacher_id, "new_review", json!({ "rating": r.rating, "target": r.target_type }), &link);
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
        conn.execute("INSERT INTO courses VALUES ('c1', ?1, 's1', 'دورة', NULL, 'published', 0, 0)", params![teacher.id]).unwrap();
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
        let count = |u: &User| list_notifications(&w.conn, &u.id).unwrap().items.len();
        assert_eq!((count(&w.student), count(&follower), count(&suspended), count(&w.outsider), count(&w.teacher)), (1, 1, 0, 0, 0));
        let list = list_notifications(&w.conn, &w.student.id).unwrap();
        assert_eq!(list.unread, 1);
        mark_read(&w.conn, &w.outsider.id, &None).unwrap(); // cannot touch others'
        assert_eq!(list_notifications(&w.conn, &w.student.id).unwrap().unread, 1);
        mark_read(&w.conn, &w.student.id, &Some(vec![list.items[0].id.clone()])).unwrap();
        assert_eq!(list_notifications(&w.conn, &w.student.id).unwrap().unread, 0);
        assert_eq!(list_notifications(&w.conn, &w.student.id).unwrap().items[0].data["title"], "t");
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
}
