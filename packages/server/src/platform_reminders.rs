//! Time-driven exam notifications (phase 1-8, PRD I1). A one-minute loop in `main.rs` calls `sweep`:
//!
//! * **closing** — students enrolled in the subject who have not started an open exam are reminded once when
//!   it is within 24 hours of closing (and never right on top of the "new exam" announcement);
//! * **results** — when an `after_close` exam closes, every student with a submitted attempt is told once that
//!   the result is now visible (the withheld-result counterpart of `assessment_graded`).
//!
//! `exam_reminders` makes both exactly-once: the marker row and the notification are written in one
//! transaction, so a crash can neither lose nor repeat a notification. Because every sweep re-derives what is
//! due from the current state (not from "what happened since the last tick"), a server that was stopped simply
//! catches up on its first sweep after starting.

use crate::platform::{db_err, Res};
use crate::platform_engage::notify;
use crate::platform_exams::TEACHER_OK;
use crate::relay::now_ms;
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::HashSet;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS exam_reminders (
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('closing','results')),
  sent_at INTEGER NOT NULL,
  PRIMARY KEY (assessment_id, student_id, kind)
);
CREATE INDEX IF NOT EXISTS idx_reminders_student ON exam_reminders(student_id);
";

/// The reminder goes out once an open exam is this close to its closing time (PRD I1).
pub const CLOSING_LEAD_MS: i64 = 24 * 3_600_000;
/// …but not before this long after students were told about the exam, so a short window does not produce
/// "new exam" and "closing soon" back to back.
pub const AFTER_ANNOUNCE_MS: i64 = 6 * 3_600_000;
/// How far back a sweep looks for exams that closed while the server was down.
pub const RELEASE_LOOKBACK_MS: i64 = 14 * 86_400_000;

/// What one sweep sent.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Sweep {
    pub closing: usize,
    pub released: usize,
}

/// Writes the marker and the notification together; `false` when this student already has the marker.
fn deliver(conn: &Connection, assessment_id: &str, student_id: &str, kind: &str, now: i64, data: serde_json::Value, link: &str) -> Res<bool> {
    let fresh = conn
        .execute(
            "INSERT OR IGNORE INTO exam_reminders(assessment_id, student_id, kind, sent_at) VALUES (?1,?2,?3,?4)",
            params![assessment_id, student_id, kind, now],
        )
        .map_err(db_err)?;
    if fresh == 0 {
        return Ok(false);
    }
    let notification_kind = if kind == "closing" { "exam_closing" } else { "exam_results" };
    notify(conn, student_id, notification_kind, data, link);
    Ok(true)
}

/// Reminds students who have not started an exam closing within 24 hours.
fn remind_closing(conn: &Connection, now: i64) -> Res<usize> {
    let exams: Vec<(String, String, String, i64)> = conn
        .prepare(&format!(
            "SELECT a.id, a.title, a.subject_id, a.closes_at FROM assessments a
               LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
              WHERE a.status = 'published' AND a.closes_at > ?1 AND a.closes_at <= ?1 + ?2
                AND (a.opens_at IS NULL OR a.opens_at <= ?1)
                AND COALESCE(a.published_at, 0) + ?3 <= ?1
                AND s.is_active = 1 AND {TEACHER_OK}"
        ))
        .map_err(db_err)?
        .query_map(params![now, CLOSING_LEAD_MS, AFTER_ANNOUNCE_MS], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut sent = 0;
    for (id, title, subject_id, closes_at) in exams {
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        // "has not taken it": no attempt of any status (an abandoned attempt is already settled at its deadline)
        let students: Vec<String> = tx
            .prepare(
                "SELECT e.student_id FROM subject_enrollments e JOIN users st ON st.id = e.student_id
                  WHERE e.subject_id = ?1 AND st.role = 'student' AND st.status = 'active'
                    AND NOT EXISTS(SELECT 1 FROM attempts t WHERE t.assessment_id = ?2 AND t.student_id = e.student_id)
                    AND NOT EXISTS(SELECT 1 FROM exam_reminders r WHERE r.assessment_id = ?2 AND r.student_id = e.student_id AND r.kind = 'closing')",
            )
            .map_err(db_err)?
            .query_map(params![subject_id, id], |r| r.get(0))
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        let link = format!("/platform/assessments/{id}");
        for st in &students {
            if deliver(&tx, &id, st, "closing", now, json!({ "title": title, "closes_at": closes_at }), &link)? {
                sent += 1;
            }
        }
        tx.commit().map_err(db_err)?;
    }
    Ok(sent)
}

/// The moment an `after_close` exam's results became visible: its closing time or the manual close,
/// whichever came first (matches `platform_exams::is_released`).
fn release_moment(status: &str, closes_at: Option<i64>, closed_at: Option<i64>, updated_at: i64) -> Option<i64> {
    let manual = matches!(status, "closed" | "archived").then(|| closed_at.unwrap_or(updated_at));
    match (manual, closes_at) {
        (Some(m), Some(c)) => Some(m.min(c)),
        (m, c) => m.or(c),
    }
}

/// Tells students with a submitted attempt that the result of a closed `after_close` exam is now visible.
/// Attempts finished after the release already showed their result at once, so they are not told again.
fn announce_released(conn: &Connection, now: i64) -> Res<usize> {
    type Row = (String, String, String, Option<i64>, Option<i64>, i64);
    let exams: Vec<Row> = conn
        .prepare(&format!(
            "SELECT a.id, a.title, a.status, a.closes_at, a.closed_at, a.updated_at FROM assessments a
               LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
              WHERE a.release_mode = 'after_close' AND a.status IN ('published','closed')
                AND (a.status = 'closed' OR a.closes_at <= ?1)
                AND COALESCE(a.closed_at, a.closes_at, a.updated_at) >= ?2
                AND s.is_active = 1 AND {TEACHER_OK}"
        ))
        .map_err(db_err)?
        .query_map(params![now, now - RELEASE_LOOKBACK_MS], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut sent = 0;
    for (id, title, status, closes_at, closed_at, updated_at) in exams {
        let Some(released_at) = release_moment(&status, closes_at, closed_at, updated_at) else { continue };
        if released_at > now || released_at < now - RELEASE_LOOKBACK_MS {
            continue;
        }
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        // newest submitted attempt first, so each student is sent to their latest result
        let rows: Vec<(String, String)> = tx
            .prepare(
                "SELECT t.student_id, t.id FROM attempts t JOIN users st ON st.id = t.student_id
                  WHERE t.assessment_id = ?1 AND t.status = 'submitted' AND t.submitted_at <= ?2 AND st.status = 'active'
                    AND NOT EXISTS(SELECT 1 FROM exam_reminders r WHERE r.assessment_id = t.assessment_id AND r.student_id = t.student_id AND r.kind = 'results')
                  ORDER BY t.submitted_at DESC, t.rowid DESC",
            )
            .map_err(db_err)?
            .query_map(params![id, released_at], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        let mut done: HashSet<String> = HashSet::new();
        for (student, attempt) in rows {
            if !done.insert(student.clone()) {
                continue;
            }
            if deliver(&tx, &id, &student, "results", now, json!({ "title": title }), &format!("/platform/attempts/{attempt}"))? {
                sent += 1;
            }
        }
        tx.commit().map_err(db_err)?;
    }
    Ok(sent)
}

/// Runs both jobs. A failure in one does not stop the other (it is retried by the next sweep).
pub fn sweep(conn: &Connection, now: i64) -> Sweep {
    let closing = remind_closing(conn, now).unwrap_or_else(|e| {
        eprintln!("Platform: closing reminders failed: {}", e.1);
        0
    });
    let released = announce_released(conn, now).unwrap_or_else(|e| {
        eprintln!("Platform: result notifications failed: {}", e.1);
        0
    });
    Sweep { closing, released }
}

/// Entry point for the background loop.
pub fn run(state: &crate::platform::PlatformState) -> Sweep {
    match state.conn.lock() {
        Ok(conn) => sweep(&conn, now_ms()),
        Err(_) => Sweep::default(),
    }
}

/// An exam that was reopened (or whose past closing time was extended) hides its results again, so the next
/// close announces them anew. Best effort: a failure only means a student might miss one notification.
pub fn reset_results(conn: &Connection, assessment_id: &str) {
    let _ = conn.execute("DELETE FROM exam_reminders WHERE assessment_id = ?1 AND kind = 'results'", params![assessment_id]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user, User};
    use crate::platform_exam_admin::{act, create_exam, update_exam, ExamReq};
    use crate::platform_exams::{start_attempt, submit_attempt};
    use std::collections::HashMap;

    const T0: i64 = 1_000_000_000_000;
    const H: i64 = 3_600_000;
    const MIN: i64 = 60_000;

    struct W {
        conn: Connection,
        admin: User,
        students: Vec<User>,
    }

    /// Five active students enrolled in `s1`, plus a suspended one in `s1`, an outsider and a student of `s2`.
    fn world() -> W {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s2','i1','أخرى',0)", []).unwrap();
        let students: Vec<User> = (0..5).map(|i| insert_test_user(&conn, &format!("st{i}@x.com"), "student", "active")).collect();
        for s in &students {
            conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![s.id]).unwrap();
        }
        let suspended = insert_test_user(&conn, "sus@x.com", "student", "suspended");
        conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![suspended.id]).unwrap();
        let other = insert_test_user(&conn, "other@x.com", "student", "active");
        conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s2',0)", params![other.id]).unwrap();
        insert_test_user(&conn, "nobody@x.com", "student", "active");
        W { conn, admin, students }
    }

    fn req(extra: serde_json::Value) -> ExamReq {
        let mut v = serde_json::json!({
            "subject_id": "s1", "title": "امتحان",
            "questions": [
                {"id": "q1", "type": "single_choice", "stem": "س1", "options": ["a", "b", "c"], "answer": "B", "score": 1},
                {"id": "q2", "type": "short_answer", "stem": "اشرح", "answer": "x", "score": 4}
            ],
            "status": "published"
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        serde_json::from_value(v).unwrap()
    }

    /// Creates (and by default publishes) an admin exam at `now`; returns its id.
    fn exam(w: &W, extra: serde_json::Value, now: i64) -> String {
        let d = create_exam(&w.conn, &w.admin, &req(extra), now).unwrap();
        serde_json::to_value(&d).unwrap()["id"].as_str().unwrap().to_string()
    }

    fn count(w: &W, kind: &str) -> i64 {
        w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = ?1", params![kind], |r| r.get(0)).unwrap()
    }
    fn count_for(w: &W, user: &User, kind: &str) -> i64 {
        w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = ?1 AND user_id = ?2", params![kind, user.id], |r| r.get(0)).unwrap()
    }
    fn markers(w: &W, kind: &str) -> i64 {
        w.conn.query_row("SELECT count(*) FROM exam_reminders WHERE kind = ?1", params![kind], |r| r.get(0)).unwrap()
    }
    fn answers() -> HashMap<String, String> {
        [("q1".to_string(), "B".to_string()), ("q2".to_string(), "إجابة".to_string())].into()
    }
    /// Starts at `at` and submits `after` ms later.
    fn take(w: &W, who: &User, exam: &str, at: i64, after: i64) -> String {
        let s = start_attempt(&w.conn, who, exam, at).unwrap();
        submit_attempt(&w.conn, who, &s.attempt_id, &answers(), at + after).unwrap();
        s.attempt_id
    }

    #[test]
    fn closing_reminder_is_sent_once_and_only_to_students_who_have_not_taken_the_exam() {
        let w = world();
        let id = exam(&w, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        // closes in 29 h: too early
        assert_eq!(sweep(&w.conn, T0 + H), Sweep::default());
        // students 0..=2 took it (one still in progress), 3 and 4 did not
        take(&w, &w.students[0], &id, T0 + 2 * H, 10 * MIN);
        take(&w, &w.students[1], &id, T0 + 2 * H, 10 * MIN);
        start_attempt(&w.conn, &w.students[2], &id, T0 + 3 * H).unwrap();
        // 23 h to go: due
        let first = sweep(&w.conn, T0 + 7 * H);
        assert_eq!(first, Sweep { closing: 2, released: 0 });
        assert_eq!((count_for(&w, &w.students[3], "exam_closing"), count_for(&w, &w.students[4], "exam_closing")), (1, 1));
        for who in &w.students[..3] {
            assert_eq!(count_for(&w, who, "exam_closing"), 0, "someone who already started or finished is left alone");
        }
        assert_eq!(count(&w, "exam_closing"), 2, "not the suspended student, not other subjects, not people without an enrolment");
        // the stored notification carries what the client needs to render it
        let (data, link): (String, String) = w.conn.query_row("SELECT data, link FROM notifications WHERE kind = 'exam_closing' LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let data: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!((data["title"].as_str(), data["closes_at"].as_i64()), (Some("امتحان"), Some(T0 + 30 * H)));
        assert_eq!(link, format!("/platform/assessments/{id}"));
        // never again, however many sweeps run afterwards
        for step in [MIN, 5 * MIN, H, 10 * H, 22 * H] {
            assert_eq!(sweep(&w.conn, T0 + 7 * H + step), Sweep::default(), "no duplicate after {step}");
        }
        assert_eq!((count(&w, "exam_closing"), markers(&w, "closing")), (2, 2));
    }

    #[test]
    fn the_reminder_is_due_exactly_24_hours_before_closing() {
        let w = world();
        // announced long before, so only the 24 h rule decides
        exam(&w, serde_json::json!({ "closes_at": T0 + 60 * H }), T0);
        assert_eq!(sweep(&w.conn, T0 + 30 * H), Sweep::default(), "30 h to go");
        assert_eq!(sweep(&w.conn, T0 + 36 * H - 1), Sweep::default(), "one millisecond early");
        assert_eq!(sweep(&w.conn, T0 + 36 * H).closing, 5, "exactly 24 h to go");
    }

    #[test]
    fn a_student_enrolling_later_is_reminded_once_too() {
        let w = world();
        exam(&w, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        assert_eq!(sweep(&w.conn, T0 + 8 * H).closing, 5);
        let late = insert_test_user(&w.conn, "late@x.com", "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![late.id]).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 9 * H).closing, 1, "only the newcomer");
        assert_eq!(count_for(&w, &late, "exam_closing"), 1);
        assert_eq!(sweep(&w.conn, T0 + 10 * H), Sweep::default());
    }

    #[test]
    fn a_stopped_server_catches_up_on_its_first_sweep_and_never_reminds_after_closing() {
        let w = world();
        exam(&w, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        // the server was down from 20 h to 29 h: the first sweep after the restart still reminds (1 h left)
        assert_eq!(sweep(&w.conn, T0 + 29 * H).closing, 5);
        // a second exam whose whole reminder period passed during the outage: nothing is sent once it has closed
        let w2 = world();
        exam(&w2, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        assert_eq!(sweep(&w2.conn, T0 + 30 * H), Sweep::default(), "closes_at itself no longer counts as open");
        assert_eq!(sweep(&w2.conn, T0 + 31 * H), Sweep::default());
        assert_eq!(count(&w2, "exam_closing"), 0);
    }

    #[test]
    fn short_windows_wait_until_six_hours_after_the_announcement() {
        let w = world();
        // 3 h window: the announcement is the only notice there will be
        let short = exam(&w, serde_json::json!({ "closes_at": T0 + 3 * H, "title": "قصير" }), T0);
        for t in [MIN, H, 2 * H, 3 * H - MIN] {
            assert_eq!(sweep(&w.conn, T0 + t), Sweep::default(), "short window at +{t}");
        }
        // 10 h window: due from the start, but held back until 6 h after the announcement
        let mid = exam(&w, serde_json::json!({ "closes_at": T0 + 10 * H, "title": "متوسط" }), T0);
        assert_eq!(sweep(&w.conn, T0 + 5 * H), Sweep::default());
        assert_eq!(sweep(&w.conn, T0 + 6 * H).closing, 5);
        let titles: Vec<String> = w.conn.prepare("SELECT DISTINCT assessment_id FROM exam_reminders").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(titles, vec![mid.clone()]);
        assert_ne!(short, mid);
    }

    #[test]
    fn only_exams_that_are_open_to_students_are_reminded() {
        let w = world();
        // not yet open
        let later = exam(&w, serde_json::json!({ "opens_at": T0 + 30 * H, "closes_at": T0 + 40 * H }), T0);
        assert_eq!(sweep(&w.conn, T0 + 20 * H), Sweep::default(), "closing within 24 h but not opened yet");
        assert_eq!(sweep(&w.conn, T0 + 31 * H).closing, 5, "…reminded once it is open");
        // no closing time → never; draft → never
        let w2 = world();
        exam(&w2, serde_json::json!({}), T0);
        exam(&w2, serde_json::json!({ "status": "draft", "closes_at": T0 + 30 * H }), T0);
        assert_eq!(sweep(&w2.conn, T0 + 100 * H), Sweep::default());
        // an exam closed by hand, one archived, one in an inactive subject
        let w3 = world();
        let closed = exam(&w3, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        act(&w3.conn, &w3.admin, &closed, "close", T0 + 2 * H).unwrap();
        assert_eq!(sweep(&w3.conn, T0 + 10 * H), Sweep::default(), "a closed exam is not reminded");
        let w4 = world();
        exam(&w4, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        w4.conn.execute("UPDATE subjects SET is_active = 0", []).unwrap();
        assert_eq!(sweep(&w4.conn, T0 + 10 * H), Sweep::default(), "an inactive subject is not reminded");
        assert_ne!(later, closed);
    }

    #[test]
    fn a_teachers_exam_is_reminded_only_while_the_teacher_is_in_good_standing() {
        let w = world();
        let teacher = insert_test_user(&w.conn, "t@x.com", "teacher", "active");
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s1','approved',0)", params![teacher.id]).unwrap();
        let r: crate::platform_exams::AssessmentReq = serde_json::from_value(serde_json::json!({
            "subject_id": "s1", "title": "امتحان المعلم", "status": "published", "closes_at": T0 + 30 * H,
            "questions": [{"id": "q1", "type": "single_choice", "stem": "س", "options": ["a", "b"], "answer": "A", "score": 1}]
        })).unwrap();
        let id = crate::platform_exams::create_assessment(&w.conn, &teacher, &r).unwrap().id();
        // `create_assessment` stamps the real clock; pretend it was announced long ago
        w.conn.execute("UPDATE assessments SET published_at = ?2 WHERE id = ?1", params![id, T0 - 100 * H]).unwrap();
        w.conn.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![teacher.id]).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 10 * H), Sweep::default(), "suspended teacher: the exam is hidden, so no reminders");
        w.conn.execute("UPDATE users SET status = 'active' WHERE id = ?1", params![teacher.id]).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 11 * H).closing, 5);
    }

    #[test]
    fn results_are_announced_once_when_an_after_close_exam_closes() {
        let w = world();
        let id = exam(&w, serde_json::json!({ "closes_at": T0 + 2 * H, "release_mode": "after_close", "duration_min": 120 }), T0);
        let a0 = take(&w, &w.students[0], &id, T0 + 10 * MIN, 5 * MIN);
        let a1 = take(&w, &w.students[1], &id, T0 + 20 * MIN, 5 * MIN);
        start_attempt(&w.conn, &w.students[2], &id, T0 + 30 * MIN).unwrap(); // never submitted
        // a late starter who finishes after the exam closed sees the result at once, so no notification
        let late = start_attempt(&w.conn, &w.students[4], &id, T0 + 2 * H - MIN).unwrap();
        // not closed yet
        assert_eq!(sweep(&w.conn, T0 + H).released, 0);
        // closed
        assert_eq!(sweep(&w.conn, T0 + 2 * H).released, 2);
        assert_eq!((count_for(&w, &w.students[0], "exam_results"), count_for(&w, &w.students[1], "exam_results")), (1, 1));
        let link: String = w.conn.query_row("SELECT link FROM notifications WHERE kind = 'exam_results' AND user_id = ?1", params![w.students[0].id], |r| r.get(0)).unwrap();
        assert_eq!(link, format!("/platform/attempts/{a0}"));
        assert_ne!(a0, a1);
        // again: nothing, however often
        for step in [MIN, H, 3 * 24 * H] {
            assert_eq!(sweep(&w.conn, T0 + 2 * H + step).released, 0);
        }
        submit_attempt(&w.conn, &w.students[4], &late.attempt_id, &answers(), T0 + 2 * H + 5 * MIN).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 2 * H + 10 * MIN).released, 0, "submitted after the close: saw the result immediately");
        assert_eq!(count(&w, "exam_results"), 2);
        assert_eq!(count_for(&w, &w.students[2], "exam_results"), 0, "never submitted");
        assert_eq!(count_for(&w, &w.students[3], "exam_results"), 0, "never took it");
    }

    #[test]
    fn immediate_release_exams_never_announce_results() {
        let w = world();
        let id = exam(&w, serde_json::json!({ "closes_at": T0 + 2 * H }), T0);
        take(&w, &w.students[0], &id, T0 + 10 * MIN, 5 * MIN);
        assert_eq!(sweep(&w.conn, T0 + 3 * H).released, 0);
        assert_eq!(count(&w, "exam_results"), 0);
    }

    #[test]
    fn the_announcement_catches_up_after_downtime_but_not_beyond_the_lookback() {
        let w = world();
        let id = exam(&w, serde_json::json!({ "closes_at": T0 + 2 * H, "release_mode": "after_close", "max_attempts": 2 }), T0);
        take(&w, &w.students[0], &id, T0 + 10 * MIN, 5 * MIN);
        // newest attempt wins for the link
        let second = take(&w, &w.students[0], &id, T0 + 40 * MIN, 5 * MIN);
        // the server was off for three days after the close
        assert_eq!(sweep(&w.conn, T0 + 2 * H + 3 * 24 * H).released, 1);
        let link: String = w.conn.query_row("SELECT link FROM notifications WHERE kind = 'exam_results'", [], |r| r.get(0)).unwrap();
        assert_eq!(link, format!("/platform/attempts/{second}"));
        // another exam that closed a month before the sweep is considered history
        let old = exam(&w, serde_json::json!({ "closes_at": T0 + 2 * H, "release_mode": "after_close", "title": "قديم" }), T0);
        take(&w, &w.students[1], &old, T0 + 10 * MIN, 5 * MIN);
        assert_eq!(sweep(&w.conn, T0 + 2 * H + 30 * 24 * H).released, 0);
    }

    #[test]
    fn closing_by_hand_announces_at_that_moment_and_reopening_re_arms_it() {
        let w = world();
        let id = exam(&w, serde_json::json!({ "closes_at": T0 + 10 * 24 * H, "release_mode": "after_close" }), T0);
        take(&w, &w.students[0], &id, T0 + H, 5 * MIN);
        assert_eq!(sweep(&w.conn, T0 + 2 * H).released, 0, "still open: results are withheld, nothing to announce");
        act(&w.conn, &w.admin, &id, "close", T0 + 3 * H).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 3 * H + MIN).released, 1);
        assert_eq!(sweep(&w.conn, T0 + 4 * H).released, 0);
        // reopened: results are hidden again, the markers are cleared…
        act(&w.conn, &w.admin, &id, "reopen", T0 + 5 * H).unwrap();
        assert_eq!(markers(&w, "results"), 0);
        assert_eq!(sweep(&w.conn, T0 + 6 * H).released, 0, "open again");
        // …and the next close announces once more
        act(&w.conn, &w.admin, &id, "close", T0 + 7 * H).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 7 * H + MIN).released, 1);
        assert_eq!(count_for(&w, &w.students[0], "exam_results"), 2);
        // extending a closing time that had passed reopens it as well
        let w2 = world();
        let id2 = exam(&w2, serde_json::json!({ "closes_at": T0 + 2 * H, "release_mode": "after_close" }), T0);
        take(&w2, &w2.students[0], &id2, T0 + 10 * MIN, 5 * MIN);
        assert_eq!(sweep(&w2.conn, T0 + 3 * H).released, 1);
        let r: ExamReq = serde_json::from_value(serde_json::json!({ "closes_at": T0 + 20 * H })).unwrap();
        update_exam(&w2.conn, &w2.admin, &id2, &r, T0 + 4 * H).unwrap();
        assert_eq!(markers(&w2, "results"), 0, "extending the closing time re-arms the announcement");
        assert_eq!(sweep(&w2.conn, T0 + 5 * H).released, 0, "open again");
        assert_eq!(sweep(&w2.conn, T0 + 21 * H).released, 1);
        // a plain edit of an open exam does not touch the markers
        let w3 = world();
        let id3 = exam(&w3, serde_json::json!({ "closes_at": T0 + 2 * H, "release_mode": "after_close" }), T0);
        take(&w3, &w3.students[0], &id3, T0 + 10 * MIN, 5 * MIN);
        sweep(&w3.conn, T0 + 3 * H);
        let t: ExamReq = serde_json::from_value(serde_json::json!({ "title": "عنوان جديد" })).unwrap();
        update_exam(&w3.conn, &w3.admin, &id3, &t, T0 + 4 * H).unwrap();
        assert_eq!(markers(&w3, "results"), 1);
    }

    #[test]
    fn release_moment_is_the_earlier_of_the_closing_time_and_the_manual_close() {
        assert_eq!(release_moment("published", Some(100), None, 500), Some(100));
        assert_eq!(release_moment("closed", Some(100), Some(60), 500), Some(60), "closed by hand before the deadline");
        assert_eq!(release_moment("closed", Some(100), Some(160), 500), Some(100), "closed by hand after the deadline passed");
        assert_eq!(release_moment("closed", None, Some(60), 500), Some(60));
        assert_eq!(release_moment("closed", None, None, 500), Some(500), "falls back to the last change");
        assert_eq!(release_moment("published", None, None, 500), None);
    }

    #[test]
    fn grading_before_the_release_is_silent_and_the_release_notification_covers_it() {
        let w = world();
        let real = now_ms();
        // closing time in real time, since `grade_attempt` consults the real clock like the results page does
        let held = exam(&w, serde_json::json!({ "closes_at": real + 30 * 24 * H, "release_mode": "after_close" }), real);
        let open = exam(&w, serde_json::json!({ "closes_at": real + 30 * 24 * H, "title": "فوري" }), real);
        let grade = |exam: &str, who: &User| {
            let attempt = take(&w, who, exam, real + MIN, MIN);
            let g = crate::platform_exams::GradeReq { grades: [("q2".to_string(), 3.0)].into() };
            crate::platform_exams::grade_attempt(&w.conn, &w.admin, &attempt, &g).unwrap();
        };
        grade(&held, &w.students[0]);
        grade(&open, &w.students[1]);
        assert_eq!(count_for(&w, &w.students[0], "assessment_graded"), 0, "the result is withheld until the close");
        assert_eq!(count_for(&w, &w.students[1], "assessment_graded"), 1, "immediate exams notify as before");
    }

    #[test]
    fn markers_disappear_with_their_exam_or_student() {
        let w = world();
        let id = exam(&w, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        sweep(&w.conn, T0 + 8 * H);
        assert_eq!(markers(&w, "closing"), 5);
        w.conn.execute("DELETE FROM users WHERE id = ?1", params![w.students[0].id]).unwrap();
        assert_eq!(markers(&w, "closing"), 4);
        w.conn.execute("DELETE FROM assessments WHERE id = ?1", params![id]).unwrap();
        assert_eq!(markers(&w, "closing"), 0);
    }

    #[test]
    fn a_sweep_is_idempotent_when_run_twice_in_the_same_instant() {
        let w = world();
        let id = exam(&w, serde_json::json!({ "closes_at": T0 + 2 * H, "release_mode": "after_close" }), T0);
        take(&w, &w.students[0], &id, T0 + 10 * MIN, 5 * MIN);
        let both = |t: i64| (sweep(&w.conn, t), sweep(&w.conn, t));
        assert_eq!(both(T0 + 3 * H), (Sweep { closing: 0, released: 1 }, Sweep::default()));
    }
}
