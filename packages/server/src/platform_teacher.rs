//! The teacher's home screen numbers (phase 3-1): one request that says what is waiting for the signed-in
//! teacher — assignment requests, content, exams, answers to grade, unread notifications.
//!
//! Everything is scoped by the authenticated teacher's id (read from the session, never from the request) and
//! computed on demand: a per-user cache would need the user in its key and the queries are all indexed lookups.
//!
//! Definitions (one place, so the dashboard and the pages it links to agree):
//! * **teaching** — the teacher's own `teacher_subjects` rows by status;
//! * **content** — the teacher's own posts/courses by status; `live_upcoming` = scheduled sessions that have not
//!   ended yet (the same "not ended" rule as the live lists);
//! * **assessments** — the teacher's *own* exams by effective phase (a published exam past its closing time counts
//!   as closed, like everywhere else); archived exams are not counted; `attempts_submitted` = submitted attempts
//!   only (the same definition as the platform statistics);
//! * **pending_grading** — ungraded written answers (`attempts.pending` of submitted attempts) in the exams this
//!   teacher may grade, i.e. exactly the grading queue: their own exams plus admin-created exams of subjects they are
//!   approved for (`platform_exams::can_grade`), and the number of such exams;
//! * **unread** — a COUNT of the caller's unread notifications (no rows are fetched);
//! * **can_create_exams** — the admin switch `teachers.can_create_exams` (unset = on).

use crate::platform::{db_err, lock, require_role, Res, User};
use crate::platform_settings::Crypto;
use crate::relay::now_ms;
use crate::routes::AppState;
use axum::{extract::State, http::HeaderMap, Json};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct Teaching {
    pub pending: i64,
    pub approved: i64,
    pub rejected: i64,
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct Content {
    pub posts_draft: i64,
    pub posts_published: i64,
    pub courses_draft: i64,
    pub courses_published: i64,
    pub live_upcoming: i64,
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct Assessments {
    pub draft: i64,
    pub published: i64,
    pub closed: i64,
    pub attempts_submitted: i64,
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct PendingGrading {
    pub answers: i64,
    pub exams: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct TeacherStats {
    pub teaching: Teaching,
    pub content: Content,
    pub assessments: Assessments,
    pub pending_grading: PendingGrading,
    pub unread: i64,
    pub can_create_exams: bool,
}

/// One number out of one query (`COALESCE`d to 0 in the SQL where a SUM may be NULL).
fn count(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Res<i64> {
    conn.query_row(sql, p, |r| r.get(0)).map_err(db_err)
}

pub fn teacher_stats(conn: &Connection, crypto: &Crypto, teacher: &User, now: i64) -> Res<TeacherStats> {
    let id = teacher.id.as_str();

    let teaching = conn
        .query_row(
            "SELECT COALESCE(SUM(status = 'pending'), 0), COALESCE(SUM(status = 'approved'), 0), COALESCE(SUM(status = 'rejected'), 0)
             FROM teacher_subjects WHERE teacher_id = ?1",
            params![id],
            |r| Ok(Teaching { pending: r.get(0)?, approved: r.get(1)?, rejected: r.get(2)? }),
        )
        .map_err(db_err)?;

    let by_status = |table: &str, status: &str| -> Res<i64> {
        // `table` is always a literal from this function
        count(conn, &format!("SELECT count(*) FROM {table} WHERE teacher_id = ?1 AND status = ?2"), params![id, status])
    };
    let content = Content {
        posts_draft: by_status("posts", "draft")?,
        posts_published: by_status("posts", "published")?,
        courses_draft: by_status("courses", "draft")?,
        courses_published: by_status("courses", "published")?,
        live_upcoming: count(
            conn,
            "SELECT count(*) FROM live_sessions WHERE teacher_id = ?1 AND status = 'scheduled' AND starts_at + duration_min * 60000 > ?2",
            params![id, now],
        )?,
    };

    let assessments = Assessments {
        draft: count(conn, "SELECT count(*) FROM assessments WHERE teacher_id = ?1 AND status = 'draft'", params![id])?,
        published: count(
            conn,
            "SELECT count(*) FROM assessments WHERE teacher_id = ?1 AND status = 'published' AND (closes_at IS NULL OR closes_at > ?2)",
            params![id, now],
        )?,
        closed: count(
            conn,
            "SELECT count(*) FROM assessments WHERE teacher_id = ?1 AND (status = 'closed' OR (status = 'published' AND closes_at <= ?2))",
            params![id, now],
        )?,
        attempts_submitted: count(
            conn,
            "SELECT count(*) FROM attempts t JOIN assessments a ON a.id = t.assessment_id WHERE a.teacher_id = ?1 AND t.status = 'submitted'",
            params![id],
        )?,
    };

    // the grading queue's scope (`platform_grading::pending_exams`), summed instead of listed
    let pending_grading = conn
        .query_row(
            "SELECT COALESCE(SUM(t.pending), 0), count(DISTINCT t.assessment_id)
             FROM attempts t JOIN assessments a ON a.id = t.assessment_id
             WHERE t.status = 'submitted' AND t.pending > 0
               AND (a.teacher_id = ?1 OR (a.teacher_id IS NULL AND EXISTS(
                    SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = ?1 AND ts.subject_id = a.subject_id AND ts.status = 'approved')))",
            params![id],
            |r| Ok(PendingGrading { answers: r.get(0)?, exams: r.get(1)? }),
        )
        .map_err(db_err)?;

    Ok(TeacherStats {
        teaching,
        content,
        assessments,
        pending_grading,
        unread: count(conn, "SELECT count(*) FROM notifications WHERE user_id = ?1 AND read_at IS NULL", params![id])?,
        can_create_exams: crate::platform_ai::exams_enabled(conn, crypto)?,
    })
}

pub async fn stats_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<TeacherStats>> {
    let teacher = require_role(&s, &h, "teacher")?;
    teacher_stats(&*lock(&s)?, &s.platform.crypto, &teacher, now_ms()).map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, ensure_role, insert_test_user};
    use axum::http::StatusCode;

    const NOW: i64 = 1_800_000_000_000;
    const H: i64 = 3_600_000;

    fn crypto() -> Crypto {
        Crypto::for_tests()
    }

    fn world() -> (Connection, User, User) {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0), ('s2','i1','قواعد',1,0), ('s3','i1','شبكات',1,0);",
        )
        .unwrap();
        let me = insert_test_user(&conn, "me@x.com", "teacher", "active");
        let other = insert_test_user(&conn, "other@x.com", "teacher", "active");
        (conn, me, other)
    }

    fn assign(conn: &Connection, t: &User, subject: &str, status: &str) {
        conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at) VALUES (?1,?2,?3,0)", params![t.id, subject, status]).unwrap();
    }

    fn post(conn: &Connection, id: &str, t: &User, status: &str) {
        conn.execute("INSERT INTO posts VALUES (?1, ?2, 's1', 'article', 'عنوان', 'نص', ?3, NULL, 0, 0, NULL)", params![id, t.id, status]).unwrap();
    }

    fn course(conn: &Connection, id: &str, t: &User, status: &str) {
        conn.execute("INSERT INTO courses VALUES (?1, ?2, 's1', 'دورة', NULL, ?3, 0, 0, NULL)", params![id, t.id, status]).unwrap();
    }

    fn live(conn: &Connection, id: &str, t: &User, starts: i64, minutes: i64, status: &str) {
        conn.execute(
            "INSERT INTO live_sessions(id,teacher_id,subject_id,title,starts_at,duration_min,join_url,status,created_at) VALUES (?1,?2,'s1','بث',?3,?4,'https://x.example/j',?5,0)",
            params![id, t.id, starts, minutes, status],
        )
        .unwrap();
    }

    /// `teacher` None = an admin exam.
    fn exam(conn: &Connection, id: &str, teacher: Option<&User>, subject: &str, status: &str, closes_at: Option<i64>) {
        conn.execute(
            "INSERT INTO assessments(id,teacher_id,subject_id,title,questions,question_count,total_points,closes_at,status,created_at,updated_at)
             VALUES (?1,?2,?3,'امتحان','[]',1,1,?4,?5,0,0)",
            params![id, teacher.map(|t| t.id.clone()), subject, closes_at, status],
        )
        .unwrap();
    }

    fn attempt(conn: &Connection, id: &str, exam: &str, student: &User, status: &str, pending: i64) {
        conn.execute("INSERT INTO attempts(id,assessment_id,student_id,started_at,status,pending) VALUES (?1,?2,?3,0,?4,?5)", params![id, exam, student.id, status, pending]).unwrap();
    }

    fn stats(conn: &Connection, t: &User) -> TeacherStats {
        teacher_stats(conn, &crypto(), t, NOW).unwrap()
    }

    #[test]
    fn a_new_teacher_sees_zeros_and_exams_enabled() {
        let (conn, me, _) = world();
        let s = stats(&conn, &me);
        assert_eq!(
            s,
            TeacherStats {
                teaching: Teaching::default(),
                content: Content::default(),
                assessments: Assessments::default(),
                pending_grading: PendingGrading::default(),
                unread: 0,
                can_create_exams: true,
            }
        );
    }

    #[test]
    fn the_json_shape_is_the_agreed_contract() {
        let (conn, me, _) = world();
        let v = serde_json::to_value(stats(&conn, &me)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "teaching": {"pending": 0, "approved": 0, "rejected": 0},
                "content": {"posts_draft": 0, "posts_published": 0, "courses_draft": 0, "courses_published": 0, "live_upcoming": 0},
                "assessments": {"draft": 0, "published": 0, "closed": 0, "attempts_submitted": 0},
                "pending_grading": {"answers": 0, "exams": 0},
                "unread": 0,
                "can_create_exams": true
            })
        );
    }

    #[test]
    fn only_the_callers_own_rows_are_counted() {
        let (conn, me, other) = world();
        for (t, who) in [(&me, "a"), (&other, "b")] {
            assign(&conn, t, "s1", "approved");
            post(&conn, &format!("p-{who}"), t, "published");
            course(&conn, &format!("c-{who}"), t, "draft");
            live(&conn, &format!("l-{who}"), t, NOW + H, 60, "scheduled");
            exam(&conn, &format!("e-{who}"), Some(t), "s1", "published", None);
        }
        assign(&conn, &other, "s2", "pending");
        let s = stats(&conn, &me);
        assert_eq!((s.teaching.approved, s.teaching.pending), (1, 0), "the other teacher's pending request is not mine");
        assert_eq!((s.content.posts_published, s.content.courses_draft, s.content.live_upcoming), (1, 1, 1));
        assert_eq!(s.assessments.published, 1);
        let o = stats(&conn, &other);
        assert_eq!((o.teaching.approved, o.teaching.pending), (1, 1));
    }

    #[test]
    fn teaching_requests_are_split_by_status() {
        let (conn, me, _) = world();
        assign(&conn, &me, "s1", "pending");
        assign(&conn, &me, "s2", "approved");
        assign(&conn, &me, "s3", "rejected");
        assert_eq!(stats(&conn, &me).teaching, Teaching { pending: 1, approved: 1, rejected: 1 });
    }

    #[test]
    fn content_is_split_into_drafts_and_published_and_live_counts_only_what_has_not_ended() {
        let (conn, me, _) = world();
        post(&conn, "p1", &me, "draft");
        post(&conn, "p2", &me, "draft");
        post(&conn, "p3", &me, "published");
        course(&conn, "c1", &me, "draft");
        course(&conn, "c2", &me, "published");
        course(&conn, "c3", &me, "published");
        live(&conn, "l1", &me, NOW + 2 * H, 60, "scheduled"); // upcoming
        live(&conn, "l2", &me, NOW - 30 * 60_000, 60, "scheduled"); // running right now: not ended
        live(&conn, "l3", &me, NOW - 2 * H, 60, "scheduled"); // ended
        live(&conn, "l4", &me, NOW + 3 * H, 60, "cancelled"); // cancelled
        let c = stats(&conn, &me).content;
        assert_eq!((c.posts_draft, c.posts_published, c.courses_draft, c.courses_published), (2, 1, 1, 2));
        assert_eq!(c.live_upcoming, 2);
        live(&conn, "l5", &me, NOW - 60_000 * 60, 60, "scheduled"); // ends exactly now: over
        assert_eq!(stats(&conn, &me).content.live_upcoming, 2, "a session ending exactly now has ended");
    }

    #[test]
    fn exams_are_counted_by_effective_phase_and_archived_ones_are_left_out() {
        let (conn, me, _) = world();
        exam(&conn, "e1", Some(&me), "s1", "draft", None);
        exam(&conn, "e2", Some(&me), "s1", "published", None);
        exam(&conn, "e3", Some(&me), "s1", "published", Some(NOW + H));
        exam(&conn, "e4", Some(&me), "s1", "published", Some(NOW - H)); // past its closing time: effectively closed
        exam(&conn, "e5", Some(&me), "s1", "published", Some(NOW)); // closing time reached: closed
        exam(&conn, "e6", Some(&me), "s1", "closed", None);
        exam(&conn, "e7", Some(&me), "s1", "archived", None);
        let a = stats(&conn, &me).assessments;
        assert_eq!((a.draft, a.published, a.closed), (1, 2, 3));
    }

    #[test]
    fn attempts_submitted_counts_submitted_attempts_of_my_exams_only() {
        let (conn, me, other) = world();
        let st = insert_test_user(&conn, "s@x.com", "student", "active");
        exam(&conn, "mine", Some(&me), "s1", "published", None);
        exam(&conn, "theirs", Some(&other), "s1", "published", None);
        exam(&conn, "admins", None, "s1", "published", None);
        for (i, status) in ["submitted", "submitted", "in_progress", "expired"].iter().enumerate() {
            attempt(&conn, &format!("m{i}"), "mine", &st, status, 0);
        }
        attempt(&conn, "t1", "theirs", &st, "submitted", 0);
        attempt(&conn, "a1", "admins", &st, "submitted", 0);
        assert_eq!(stats(&conn, &me).assessments.attempts_submitted, 2);
        assert_eq!(stats(&conn, &other).assessments.attempts_submitted, 1);
    }

    #[test]
    fn pending_grading_follows_can_grade_own_exams_and_admin_exams_of_approved_subjects() {
        let (conn, me, other) = world();
        let st = insert_test_user(&conn, "s@x.com", "student", "active");
        assign(&conn, &me, "s1", "approved");
        assign(&conn, &me, "s2", "pending"); // not approved: admin exams of s2 are not mine to grade
        exam(&conn, "own", Some(&me), "s1", "published", None);
        exam(&conn, "own2", Some(&me), "s1", "closed", None);
        exam(&conn, "admin-s1", None, "s1", "published", None);
        exam(&conn, "admin-s2", None, "s2", "published", None);
        exam(&conn, "colleague", Some(&other), "s1", "published", None);
        attempt(&conn, "a1", "own", &st, "submitted", 2);
        attempt(&conn, "a2", "own", &st, "submitted", 3);
        attempt(&conn, "a3", "own2", &st, "submitted", 1);
        attempt(&conn, "a4", "admin-s1", &st, "submitted", 4);
        attempt(&conn, "a5", "admin-s2", &st, "submitted", 9);
        attempt(&conn, "a6", "colleague", &st, "submitted", 7);
        // not waiting for grading: nothing pending, still in progress, expired
        attempt(&conn, "a7", "own", &st, "submitted", 0);
        attempt(&conn, "a8", "own", &st, "in_progress", 5);
        attempt(&conn, "a9", "own", &st, "expired", 5);
        let g = stats(&conn, &me).pending_grading;
        assert_eq!((g.answers, g.exams), (2 + 3 + 1 + 4, 3), "own + own2 + the admin exam of my approved subject; not the colleague's, not s2");
        // the same numbers the grading queue shows
        let queue = crate::platform_grading::pending_exams(&conn, &me).unwrap();
        assert_eq!(queue.len() as i64, g.exams);
        // approving s2 brings the admin exam in; the colleague's exam never counts
        conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1 AND subject_id = 's2'", params![me.id]).unwrap();
        let g = stats(&conn, &me).pending_grading;
        assert_eq!((g.answers, g.exams), (2 + 3 + 1 + 4 + 9, 4));
        // and the colleague sees only their own exam plus admin exams of subjects they are approved for (none)
        let o = stats(&conn, &other).pending_grading;
        assert_eq!((o.answers, o.exams), (7, 1));
    }

    #[test]
    fn unread_counts_only_my_unread_notifications() {
        let (conn, me, other) = world();
        for (i, (who, read)) in [(&me, false), (&me, false), (&me, true), (&other, false)].into_iter().enumerate() {
            conn.execute(
                "INSERT INTO notifications(id,user_id,kind,data,read_at,created_at) VALUES (?1,?2,'x','{}',?3,0)",
                params![format!("n{i}"), who.id, if read { Some(1i64) } else { None }],
            )
            .unwrap();
        }
        assert_eq!(stats(&conn, &me).unread, 2);
        assert_eq!(stats(&conn, &other).unread, 1);
    }

    #[test]
    fn can_create_exams_reads_the_admin_switch_and_defaults_to_on() {
        let (conn, me, _) = world();
        assert!(stats(&conn, &me).can_create_exams, "unset means enabled");
        conn.execute("INSERT INTO settings(key,value,encrypted,updated_at) VALUES ('teachers.can_create_exams','0',0,0)", []).unwrap();
        assert!(!stats(&conn, &me).can_create_exams);
        conn.execute("UPDATE settings SET value = '1' WHERE key = 'teachers.can_create_exams'", []).unwrap();
        assert!(stats(&conn, &me).can_create_exams);
    }

    #[test]
    fn only_an_active_teacher_may_ask() {
        // the handler's guard: students, admins and teachers whose account is not active get 403
        let (conn, _, _) = world();
        for (role, status) in [("student", "active"), ("admin", "active"), ("teacher", "pending"), ("teacher", "rejected"), ("teacher", "suspended")] {
            let u = insert_test_user(&conn, &format!("{role}-{status}@x.com"), role, status);
            assert_eq!(ensure_role(u, "teacher").unwrap_err().0, StatusCode::FORBIDDEN, "{role}/{status}");
        }
        let ok = insert_test_user(&conn, "ok@x.com", "teacher", "active");
        assert!(ensure_role(ok, "teacher").is_ok());
    }
}
