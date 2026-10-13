//! Time-driven notifications. A one-minute loop in `main.rs` calls `sweep`:
//!
//! * **closing** (phase 1-8, PRD I1) — students enrolled in the subject who have not started an open exam are reminded once
//!   when it is within 24 hours of closing (and never right on top of the "new exam" announcement);
//! * **results** — when an `after_close` exam closes, every student with a submitted attempt is told once that
//!   the result is now visible (the withheld-result counterpart of `assessment_graded`);
//! * **opened** (phase 4-2) — a scheduled exam (announced before it opened) tells the enrolled students who have not
//!   touched it that it is open now;
//! * **live soon** (phase 4-2) — a scheduled live session tells its audience (students of the subject and followers of the
//!   teacher) once, within the last 15 minutes before it starts.
//!
//! `exam_reminders` and `live_reminders` make all of them exactly-once: the marker row and the notification are written in
//! one transaction, so a crash can neither lose nor repeat a notification. Because every sweep re-derives what is due from
//! the current state (not from "what happened since the last tick"), a server that was stopped simply catches up on its
//! first sweep after starting (except for "live soon", which has no meaning once the session has begun).

use crate::platform::{db_err, Res};
use crate::platform_content::{exam_standing, visible};
use crate::platform_engage::notify;
use crate::relay::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::collections::HashSet;

/// `exam_reminders` (one marker per exam, student and kind) is defined once: `CREATE TABLE IF NOT EXISTS` for a fresh
/// database, a new table of the same shape when an older one has to be rebuilt ([`migrate`]).
macro_rules! exam_reminders_ddl {
    ($create:literal, $name:literal) => {
        concat!(
            $create, " ", $name, " (
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('closing','results','opened')),
  sent_at INTEGER NOT NULL,
  PRIMARY KEY (assessment_id, student_id, kind)
);"
        )
    };
}

pub const SCHEMA: &str = concat!(
    exam_reminders_ddl!("CREATE TABLE IF NOT EXISTS", "exam_reminders"),
    "
CREATE INDEX IF NOT EXISTS idx_reminders_student ON exam_reminders(student_id);
-- one \"starting soon\" notice per live session and student (phase 4-2); rescheduling a session clears its markers
CREATE TABLE IF NOT EXISTS live_reminders (
  live_id TEXT NOT NULL REFERENCES live_sessions(id) ON DELETE CASCADE,
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  sent_at INTEGER NOT NULL,
  PRIMARY KEY (live_id, student_id)
);
CREATE INDEX IF NOT EXISTS idx_live_reminders_student ON live_reminders(student_id);
"
);

/// Widens the `kind` CHECK of an `exam_reminders` table created before phase 4-2 (`closing`, `results` only) to admit
/// `opened`. SQLite cannot alter a CHECK in place, so the table is rebuilt: a new table of the final shape, the rows copied,
/// the old one dropped, the new one renamed — in one transaction, with foreign keys switched off for the swap (and put back
/// as they were) and verified before commit; any failure rolls back untouched. A table that already admits `opened` (a fresh
/// install, or an upgrade that already ran) is left alone, so this is idempotent and safe on every start.
pub fn migrate(conn: &Connection) -> Result<(), String> {
    let e = |x: rusqlite::Error| x.to_string();
    let ddl: Option<String> = conn.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'exam_reminders'", [], |r| r.get(0)).optional().map_err(e)?;
    if ddl.map_or(true, |d| d.contains("'opened'")) {
        return Ok(());
    }
    let fk_was_on: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).map_err(e)?;
    conn.execute_batch("PRAGMA foreign_keys = OFF;").map_err(e)?;
    let result = (|| -> Result<(), String> {
        let tx = conn.unchecked_transaction().map_err(e)?;
        tx.execute_batch("DROP TABLE IF EXISTS exam_reminders_new;").map_err(e)?;
        tx.execute_batch(exam_reminders_ddl!("CREATE TABLE", "exam_reminders_new")).map_err(e)?;
        tx.execute_batch(
            "INSERT INTO exam_reminders_new(assessment_id, student_id, kind, sent_at) SELECT assessment_id, student_id, kind, sent_at FROM exam_reminders;
             DROP TABLE exam_reminders;
             ALTER TABLE exam_reminders_new RENAME TO exam_reminders;
             CREATE INDEX IF NOT EXISTS idx_reminders_student ON exam_reminders(student_id);",
        )
        .map_err(e)?;
        let violations: i64 = tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).map_err(e)?;
        if violations > 0 {
            return Err(format!("exam_reminders migration aborted: {violations} foreign key violation(s)"));
        }
        tx.commit().map_err(e)
    })();
    if fk_was_on {
        conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(e)?;
    }
    result
}

/// The reminder goes out once an open exam is this close to its closing time (PRD I1).
pub const CLOSING_LEAD_MS: i64 = 24 * 3_600_000;
/// …but not before this long after students were told about the exam - the "new exam" notice, or, for a scheduled exam, the
/// "it is open" notice at its opening time, whichever came last - so a short window does not produce "new exam" /
/// "it is open" and "closing soon" back to back.
pub const AFTER_ANNOUNCE_MS: i64 = 6 * 3_600_000;
/// How far back a sweep looks for exams that closed while the server was down.
pub const RELEASE_LOOKBACK_MS: i64 = 14 * 86_400_000;
/// A live session reminds its audience from this long before it starts until it starts.
pub const LIVE_LEAD_MS: i64 = 15 * 60_000;
/// How far back a sweep looks for exams that opened while the server was down.
pub const OPENED_LOOKBACK_MS: i64 = 24 * 3_600_000;

/// What one sweep sent.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Sweep {
    pub closing: usize,
    pub released: usize,
    pub live_soon: usize,
    pub exam_opened: usize,
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
    let notification_kind = match kind {
        "closing" => "exam_closing",
        "opened" => "exam_opened",
        _ => "exam_results",
    };
    notify(conn, student_id, notification_kind, data, link);
    Ok(true)
}

/// Active students enrolled in `subject_id` who have no attempt of any status on the exam and no `kind` marker yet: the
/// people "closing" and "opened" are for ("has not taken it": an abandoned attempt is already settled at its deadline).
fn untouched_students(conn: &Connection, subject_id: &str, exam_id: &str, kind: &str) -> Res<Vec<String>> {
    conn.prepare(
        "SELECT e.student_id FROM subject_enrollments e JOIN users st ON st.id = e.student_id
          WHERE e.subject_id = ?1 AND st.role = 'student' AND st.status = 'active'
            AND NOT EXISTS(SELECT 1 FROM attempts t WHERE t.assessment_id = ?2 AND t.student_id = e.student_id)
            AND NOT EXISTS(SELECT 1 FROM exam_reminders r WHERE r.assessment_id = ?2 AND r.student_id = e.student_id AND r.kind = ?3)",
    )
    .map_err(db_err)?
    .query_map(params![subject_id, exam_id, kind], |r| r.get(0))
    .map_err(db_err)?
    .collect::<Result<_, _>>()
    .map_err(db_err)
}

/// Reminds students who have not started an exam closing within 24 hours.
fn remind_closing(conn: &Connection, now: i64) -> Res<usize> {
    let exams: Vec<(String, String, String, i64)> = conn
        .prepare(&format!(
            "SELECT a.id, a.title, a.subject_id, a.closes_at FROM assessments a
               LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
              WHERE a.status = 'published' AND a.closes_at > ?1 AND a.closes_at <= ?1 + ?2
                AND (a.opens_at IS NULL OR a.opens_at <= ?1)
                AND MAX(COALESCE(a.published_at, 0), COALESCE(a.opens_at, 0)) + ?3 <= ?1
                AND {}",
            exam_standing("a")
        ))
        .map_err(db_err)?
        .query_map(params![now, CLOSING_LEAD_MS, AFTER_ANNOUNCE_MS], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut sent = 0;
    for (id, title, subject_id, closes_at) in exams {
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let students = untouched_students(&tx, &subject_id, &id, "closing")?;
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

/// Tells students that an exam they were told about in advance is open now (phase 4-2).
///
/// Due when a **published** exam, in good standing ([`exam_standing`]), has an opening time in `(now - 24 h, now]` (so a
/// server that was down for a while still says it, but never about something that opened yesterday) and has not closed yet;
/// and only when students were told about it BEFORE it opened (`published_at < opens_at`; `NULL` = a legacy row = long ago):
/// an exam published after its opening time was announced as "open" by the "new exam" notice itself, and saying it again
/// would be two notices for one event. Goes to active enrolled students with NO attempt of any status, once
/// (`exam_reminders` kind `opened`); [`reset_opened`] re-arms it when the opening time moves (and only then: closing and
/// reopening an exam does not send it again).
fn remind_opened(conn: &Connection, now: i64) -> Res<usize> {
    let exams: Vec<(String, String, String, i64)> = conn
        .prepare(&format!(
            "SELECT a.id, a.title, a.subject_id, a.opens_at FROM assessments a
               LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
              WHERE a.status = 'published' AND a.opens_at IS NOT NULL AND a.opens_at > ?1 - ?2 AND a.opens_at <= ?1
                AND (a.closes_at IS NULL OR a.closes_at > ?1)
                AND COALESCE(a.published_at, 0) < a.opens_at
                AND {}",
            exam_standing("a")
        ))
        .map_err(db_err)?
        .query_map(params![now, OPENED_LOOKBACK_MS], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut sent = 0;
    for (id, title, subject_id, opens_at) in exams {
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let students = untouched_students(&tx, &subject_id, &id, "opened")?;
        let link = format!("/platform/assessments/{id}");
        for st in &students {
            if deliver(&tx, &id, st, "opened", now, json!({ "title": title, "opens_at": opens_at }), &link)? {
                sent += 1;
            }
        }
        tx.commit().map_err(db_err)?;
    }
    Ok(sent)
}

/// "Starting soon" for live sessions (phase 4-2): a **scheduled** session in good standing ([`visible`]) that starts within the
/// next 15 minutes (`starts_at - 15 min <= now < starts_at`: the 15-minute mark itself is in, the start itself is out) tells
/// every ACTIVE student who is enrolled in its subject or follows its teacher, once per (session, student)
/// (`live_reminders`, marker and notice in one transaction per session). Nobody else hears of it, and a session that was
/// cancelled or hidden sends nothing (it is not `visible`).
///
/// A session created inside its own 15-minute window is skipped: its "new live session" notice just went out, and a second
/// one right behind it would be noise. Rescheduling a session clears its markers ([`reset_live`]), so the new time reminds again.
fn remind_live(conn: &Connection, now: i64) -> Res<usize> {
    type Row = (String, String, String, String, String, String, i64);
    let sessions: Vec<Row> = conn
        .prepare(&format!(
            "SELECT l.id, l.title, l.teacher_id, u.full_name, l.subject_id, s.name_ar, l.starts_at FROM live_sessions l
               JOIN users u ON u.id = l.teacher_id JOIN subjects s ON s.id = l.subject_id
              WHERE {} AND l.starts_at - ?2 <= ?1 AND ?1 < l.starts_at AND l.created_at <= l.starts_at - ?2",
            visible("l", "scheduled")
        ))
        .map_err(db_err)?
        .query_map(params![now, LIVE_LEAD_MS], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut sent = 0;
    for (id, title, teacher_id, teacher, subject_id, subject, starts_at) in sessions {
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let students: Vec<String> = tx
            .prepare(
                "SELECT st.id FROM users st
                  WHERE st.role = 'student' AND st.status = 'active'
                    AND (st.id IN (SELECT student_id FROM subject_enrollments WHERE subject_id = ?1)
                         OR st.id IN (SELECT student_id FROM teacher_follows WHERE teacher_id = ?2))
                    AND NOT EXISTS(SELECT 1 FROM live_reminders r WHERE r.live_id = ?3 AND r.student_id = st.id)",
            )
            .map_err(db_err)?
            .query_map(params![subject_id, teacher_id, id], |r| r.get(0))
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        let link = format!("/platform/live/{id}");
        for st in &students {
            let fresh = tx.execute("INSERT OR IGNORE INTO live_reminders(live_id, student_id, sent_at) VALUES (?1,?2,?3)", params![id, st, now]).map_err(db_err)?;
            if fresh == 1 {
                notify(&tx, st, "live_soon", json!({ "title": title, "teacher": teacher, "subject": subject, "starts_at": starts_at }), &link);
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
                AND {}",
            exam_standing("a")
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

/// Runs every job. A failure in one does not stop the others (it is retried by the next sweep).
pub fn sweep(conn: &Connection, now: i64) -> Sweep {
    let closing = remind_closing(conn, now).unwrap_or_else(|e| {
        eprintln!("Platform: closing reminders failed: {}", e.1);
        0
    });
    let released = announce_released(conn, now).unwrap_or_else(|e| {
        eprintln!("Platform: result notifications failed: {}", e.1);
        0
    });
    let live_soon = remind_live(conn, now).unwrap_or_else(|e| {
        eprintln!("Platform: live reminders failed: {}", e.1);
        0
    });
    let exam_opened = remind_opened(conn, now).unwrap_or_else(|e| {
        eprintln!("Platform: exam-opened notifications failed: {}", e.1);
        0
    });
    Sweep { closing, released, live_soon, exam_opened }
}

/// Entry point for the background loop: the reminders, then the reports whose young reporters have come of age
/// ([`crate::platform_ops::recount_matured_reports`]).
pub fn run(state: &crate::platform::PlatformState) -> Sweep {
    match state.conn.lock() {
        Ok(conn) => {
            let now = now_ms();
            let swept = sweep(&conn, now);
            crate::platform_ops::recount_matured_reports(&conn, &state.report_limits, now);
            swept
        }
        Err(_) => Sweep::default(),
    }
}

/// An exam that was reopened (or whose past closing time was extended) hides its results again, so the next
/// close announces them anew. Best effort: a failure only means a student might miss one notification.
pub fn reset_results(conn: &Connection, assessment_id: &str) {
    let _ = conn.execute("DELETE FROM exam_reminders WHERE assessment_id = ?1 AND kind = 'results'", params![assessment_id]);
}

/// An exam whose opening time moved tells its students "it is open" afresh (phase 4-2). Best effort.
pub fn reset_opened(conn: &Connection, assessment_id: &str) {
    let _ = conn.execute("DELETE FROM exam_reminders WHERE assessment_id = ?1 AND kind = 'opened'", params![assessment_id]);
}

/// A live session whose start moved reminds its audience again at the new time (phase 4-2). Best effort.
pub fn reset_live(conn: &Connection, live_id: &str) {
    let _ = conn.execute("DELETE FROM live_reminders WHERE live_id = ?1", params![live_id]);
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
        assert_eq!(first, Sweep { closing: 2, ..Default::default() });
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
        // …once it is open: the opening itself is announced ("it is open", phase 4-2) and the closing reminder keeps six
        // hours away from that notice, like it does from "new exam"
        let s = sweep(&w.conn, T0 + 31 * H);
        assert_eq!((s.closing, s.exam_opened), (0, 5), "the opening notice only");
        assert_eq!(sweep(&w.conn, T0 + 36 * H - 1).closing, 0);
        assert_eq!(sweep(&w.conn, T0 + 36 * H).closing, 5, "…reminded six hours after it opened");
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
        let r: crate::platform_exam_admin::ExamReq = serde_json::from_value(serde_json::json!({
            "subject_id": "s1", "title": "امتحان المعلم", "status": "published", "closes_at": T0 + 30 * H,
            "questions": [{"id": "q1", "type": "single_choice", "stem": "س", "options": ["a", "b"], "answer": "A", "score": 1}]
        })).unwrap();
        // (the builder takes the clock as a parameter; the legacy `create_assessment` uses the real one and, like the
        // builder, refuses to publish an exam whose closing time has passed — which `T0 + 30 h` is, in real time)
        let id = crate::platform_exam_admin::create_exam(&w.conn, &teacher, &r, T0).unwrap().info_id();
        // pretend it was announced long ago
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
            let g = crate::platform_exams::GradeReq { grades: [("q2".to_string(), 3.0)].into(), ..Default::default() };
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
        assert_eq!(both(T0 + 3 * H), (Sweep { released: 1, ..Default::default() }, Sweep::default()));
    }

    #[test]
    fn nothing_is_announced_for_an_exam_in_a_hidden_institution_and_it_catches_up_when_it_is_back() {
        let w = world();
        exam(&w, serde_json::json!({ "closes_at": T0 + 30 * H }), T0);
        let after = exam(&w, serde_json::json!({ "closes_at": T0 + 2 * H, "release_mode": "after_close", "duration_min": 60 }), T0);
        take(&w, &w.students[0], &after, T0 + 10 * MIN, 5 * MIN);
        w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 7 * H), Sweep::default(), "students cannot see the exams, so they are told nothing about them");
        assert_eq!((count(&w, "exam_closing"), count(&w, "exam_results")), (0, 0));
        w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        assert_eq!(sweep(&w.conn, T0 + 7 * H), Sweep { closing: 5, released: 1, ..Default::default() }, "back on: the next sweep announces what was held back");
    }
}

/// Phase 4-2: "a live session starts soon" and "the exam is open now", and the `exam_reminders` table that learned a third kind.
#[cfg(test)]
mod phase_4_2_tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user, User};
    use crate::platform_exam_admin::{act, create_exam, update_exam, ExamReq};
    use crate::platform_exams::start_attempt;

    const T0: i64 = 1_000_000_000_000;
    const H: i64 = 3_600_000;
    const MIN: i64 = 60_000;
    const LEAD: i64 = LIVE_LEAD_MS;

    struct W {
        conn: Connection,
        admin: User,
        teacher: User,
        /// enrolled in s1
        a: User,
        b: User,
        /// enrolled in s1 AND follows the teacher
        both: User,
        /// follows the teacher, not enrolled
        follower: User,
        /// enrolled in s1, suspended
        suspended: User,
        /// enrolled in s2 only
        elsewhere: User,
        /// neither
        nobody: User,
    }

    fn world() -> W {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0), ('s2','i1','رياضيات',1,0);",
        )
        .unwrap();
        let user = |email: &str, role: &str, status: &str| insert_test_user(&conn, email, role, status);
        let admin = user("adm@x.com", "admin", "active");
        let teacher = user("t@x.com", "teacher", "active");
        conn.execute("UPDATE users SET full_name = 'د. سالم' WHERE id = ?1", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at,decided_at) VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        let (a, b, both, follower, suspended, elsewhere, nobody) = (
            user("a@x.com", "student", "active"),
            user("b@x.com", "student", "active"),
            user("both@x.com", "student", "active"),
            user("f@x.com", "student", "active"),
            user("sus@x.com", "student", "suspended"),
            user("else@x.com", "student", "active"),
            user("nobody@x.com", "student", "active"),
        );
        for (u, s) in [(&a, "s1"), (&b, "s1"), (&both, "s1"), (&suspended, "s1"), (&elsewhere, "s2")] {
            conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,?2,0)", params![u.id, s]).unwrap();
        }
        for u in [&both, &follower, &suspended] {
            conn.execute("INSERT INTO teacher_follows(student_id,teacher_id,created_at) VALUES (?1,?2,0)", params![u.id, teacher.id]).unwrap();
        }
        W { conn, admin, teacher, a, b, both, follower, suspended, elsewhere, nobody }
    }

    fn session(w: &W, id: &str, starts: i64, created: i64) {
        w.conn
            .execute(
                "INSERT INTO live_sessions(id,teacher_id,subject_id,title,description,starts_at,duration_min,join_url,status,created_at) VALUES (?1,?2,'s1','بث الأسبوع',NULL,?3,60,'https://m.example.com/x','scheduled',?4)",
                params![id, w.teacher.id, starts, created],
            )
            .unwrap();
    }

    fn got(w: &W, who: &User, kind: &str) -> i64 {
        w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = ?2", params![who.id, kind], |r| r.get(0)).unwrap()
    }

    fn everyone(w: &W, kind: &str) -> [i64; 7] {
        [&w.a, &w.b, &w.both, &w.follower, &w.suspended, &w.elsewhere, &w.nobody].map(|u| got(w, u, kind))
    }

    // ───────── live: starting soon ─────────

    #[test]
    fn a_live_session_tells_its_audience_once_and_nobody_else() {
        let w = world();
        let s = T0 + 3 * H;
        session(&w, "l1", s, T0);
        assert_eq!(sweep(&w.conn, s - LEAD - MIN), Sweep::default(), "16 minutes ahead: too early");
        // a, b (enrolled), both (enrolled AND following: one notice), follower (following only); not the suspended, not the
        // student of another subject, not the stranger
        assert_eq!(sweep(&w.conn, s - 14 * MIN), Sweep { live_soon: 4, ..Default::default() });
        assert_eq!(everyone(&w, "live_soon"), [1, 1, 1, 1, 0, 0, 0]);
        // however often the loop runs
        for t in [s - 14 * MIN, s - 10 * MIN, s - MIN, s - LEAD, s - 1] {
            assert_eq!(sweep(&w.conn, t), Sweep::default(), "at {t}");
        }
        assert_eq!(everyone(&w, "live_soon"), [1, 1, 1, 1, 0, 0, 0]);
        let n: i64 = w.conn.query_row("SELECT count(*) FROM live_reminders WHERE live_id = 'l1'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 4, "one marker per notified student");
        let (data, link): (String, String) = w.conn.query_row("SELECT data, link FROM notifications WHERE user_id = ?1 AND kind = 'live_soon'", params![w.a.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let data: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(data, serde_json::json!({ "title": "بث الأسبوع", "teacher": "د. سالم", "subject": "برمجة", "starts_at": s }));
        assert_eq!(link, "/platform/live/l1");
    }

    #[test]
    fn the_window_opens_fifteen_minutes_before_the_start_and_closes_at_the_start() {
        let s = T0 + 3 * H;
        for (now, due) in [(s - LEAD - 1, false), (s - LEAD, true), (s - 1, true), (s, false), (s + 1, false), (s + 30 * MIN, false)] {
            let w = world();
            session(&w, "l1", s, T0);
            assert_eq!(sweep(&w.conn, now).live_soon, if due { 4 } else { 0 }, "{} ms before the start", s - now);
        }
    }

    #[test]
    fn a_session_created_inside_its_own_window_is_not_reminded_about() {
        let s = T0 + 3 * H;
        // created exactly when the window opens: its "new session" notice is long enough ago -> reminded
        for (created, due) in [(s - LEAD - 1, true), (s - LEAD, true), (s - LEAD + 1, false), (s - 5 * MIN, false)] {
            let w = world();
            session(&w, "l1", s, created);
            assert_eq!(sweep(&w.conn, s - 5 * MIN).live_soon, if due { 4 } else { 0 }, "created {} ms before the start", s - created);
        }
    }

    #[test]
    fn rescheduling_re_arms_the_reminder_for_the_new_time() {
        let w = world();
        let s = T0 + 3 * H;
        session(&w, "l1", s, T0);
        assert_eq!(sweep(&w.conn, s - 10 * MIN).live_soon, 4);
        // moved a day later: the markers are cleared (as `update_live` does), and the new time reminds again
        w.conn.execute("UPDATE live_sessions SET starts_at = ?1 WHERE id = 'l1'", params![s + 24 * H]).unwrap();
        reset_live(&w.conn, "l1");
        assert_eq!(sweep(&w.conn, s - 5 * MIN), Sweep::default(), "the old time no longer counts");
        assert_eq!(sweep(&w.conn, s + 24 * H - 10 * MIN).live_soon, 4);
        assert_eq!(everyone(&w, "live_soon"), [2, 2, 2, 2, 0, 0, 0]);
        // without the reset (nothing moved) there is no second reminder
        assert_eq!(sweep(&w.conn, s + 24 * H - 9 * MIN), Sweep::default());
    }

    #[test]
    fn a_cancelled_or_hidden_session_sends_nothing_and_a_restored_one_catches_up() {
        let w = world();
        let s = T0 + 3 * H;
        session(&w, "l1", s, T0);
        let at = s - 10 * MIN;
        let hide = |sql: &str| w.conn.execute(sql, []).unwrap();
        for (off, on) in [
            ("UPDATE live_sessions SET status = 'cancelled'", "UPDATE live_sessions SET status = 'scheduled'"),
            ("UPDATE teacher_subjects SET status = 'rejected'", "UPDATE teacher_subjects SET status = 'approved'"),
            ("UPDATE subjects SET is_active = 0 WHERE id = 's1'", "UPDATE subjects SET is_active = 1 WHERE id = 's1'"),
            ("UPDATE institutions SET is_active = 0", "UPDATE institutions SET is_active = 1"),
            ("UPDATE users SET status = 'suspended' WHERE id IN (SELECT id FROM users WHERE role = 'teacher')", "UPDATE users SET status = 'active' WHERE role = 'teacher'"),
        ] {
            hide(off);
            assert_eq!(sweep(&w.conn, at), Sweep::default(), "{off}");
            hide(on);
            let n: i64 = w.conn.query_row("SELECT count(*) FROM live_reminders", [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "nothing was marked while it was hidden");
        }
        assert_eq!(sweep(&w.conn, at).live_soon, 4, "visible again inside the window: the next sweep sends it");
    }

    #[test]
    fn a_student_who_joins_inside_the_window_is_reminded_at_the_next_sweep_and_nobody_twice() {
        let w = world();
        let s = T0 + 3 * H;
        session(&w, "l1", s, T0);
        assert_eq!(sweep(&w.conn, s - 12 * MIN).live_soon, 4);
        w.conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,'s1',0)", params![w.nobody.id]).unwrap();
        assert_eq!(sweep(&w.conn, s - 10 * MIN).live_soon, 1, "only the new student");
        assert_eq!(everyone(&w, "live_soon"), [1, 1, 1, 1, 0, 0, 1]);
    }

    #[test]
    fn the_markers_go_with_the_session_and_with_the_student() {
        let w = world();
        let s = T0 + 3 * H;
        session(&w, "l1", s, T0);
        sweep(&w.conn, s - 10 * MIN);
        let n = |w: &W| -> i64 { w.conn.query_row("SELECT count(*) FROM live_reminders", [], |r| r.get(0)).unwrap() };
        assert_eq!(n(&w), 4);
        w.conn.execute("DELETE FROM users WHERE id = ?1", params![w.follower.id]).unwrap();
        assert_eq!(n(&w), 3);
        w.conn.execute("DELETE FROM live_sessions WHERE id = 'l1'", []).unwrap();
        assert_eq!(n(&w), 0);
    }

    // ───────── exams: open now ─────────

    /// An admin exam of `s1` that opens at `opens`, published at `published` (`None` = legacy, never stamped), with the
    /// given `status` and `closes`.
    fn scheduled(w: &W, id: &str, opens: Option<i64>, published: Option<i64>, status: &str, closes: Option<i64>) {
        w.conn
            .execute(
                "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,status,opens_at,closes_at,published_at,created_at,updated_at)
                 VALUES (?1,NULL,?2,'s1','امتحان الفصل','[]',1,10,?3,?4,?5,?6,0,0)",
                params![id, w.admin.id, status, opens, closes, published],
            )
            .unwrap();
    }

    fn opened(w: &W, now: i64) -> usize {
        sweep(&w.conn, now).exam_opened
    }

    fn marker_count(w: &W) -> i64 {
        w.conn.query_row("SELECT count(*) FROM exam_reminders WHERE kind = 'opened'", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn an_exam_that_opens_tells_the_enrolled_students_who_have_not_touched_it_exactly_once() {
        let w = world();
        let o = T0 + 5 * H;
        scheduled(&w, "e1", Some(o), Some(T0), "published", Some(o + 5 * H));
        assert_eq!(opened(&w, o - 1), 0, "not open yet");
        // a, b, both (the suspended student is not active; the student of another subject is not enrolled)
        assert_eq!(opened(&w, o), 3, "the opening time itself is open");
        assert_eq!(everyone(&w, "exam_opened"), [1, 1, 1, 0, 0, 0, 0]);
        for t in [o, o + MIN, o + H, o + 23 * H] {
            assert_eq!(opened(&w, t), 0, "at {t}");
        }
        assert_eq!(marker_count(&w), 3);
        let (data, link): (String, String) = w.conn.query_row("SELECT data, link FROM notifications WHERE user_id = ?1 AND kind = 'exam_opened'", params![w.a.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&data).unwrap(), serde_json::json!({ "title": "امتحان الفصل", "opens_at": o }));
        assert_eq!(link, "/platform/assessments/e1");
        // a student who enrols later (still inside the day) is told at the next sweep, the rest are not told again
        w.conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,'s1',0)", params![w.nobody.id]).unwrap();
        assert_eq!(opened(&w, o + 2 * H), 1);
    }

    #[test]
    fn the_lookback_is_a_day_and_an_exam_that_closed_is_not_announced_as_open() {
        let o = T0 + 5 * H;
        for (now, due) in [(o - 1, false), (o, true), (o + 24 * H - 1, true), (o + 24 * H, false), (o + 3 * 24 * H, false)] {
            let w = world();
            scheduled(&w, "e1", Some(o), Some(T0), "published", None);
            assert_eq!(opened(&w, now), if due { 3 } else { 0 }, "{} ms after the opening", now - o);
        }
        // closing time: strictly ahead
        for (closes, due) in [(o + H, true), (o + 1, true), (o, false)] {
            let w = world();
            scheduled(&w, "e1", Some(o), Some(T0), "published", Some(closes));
            // now = o when closes = o is "closed already"; pick now = o for all
            assert_eq!(opened(&w, o), if due { 3 } else { 0 }, "closes {} ms after the opening", closes - o);
        }
    }

    #[test]
    fn only_students_who_were_told_beforehand_are_told_it_is_open() {
        let o = T0 + 5 * H;
        // published BEFORE it opened: the announcement said "opens at ..." -> now it is open
        // published exactly at / after the opening time: the "new exam" notice already said it is open
        // never stamped (a legacy row): treated as announced long ago
        for (published, due) in [(Some(o - 1), true), (Some(T0), true), (None, true), (Some(o), false), (Some(o + 1), false)] {
            let w = world();
            scheduled(&w, "e1", Some(o), published, "published", None);
            assert_eq!(opened(&w, o + MIN), if due { 3 } else { 0 }, "published_at {published:?}");
        }
        // no opening time at all: nothing to announce
        let w = world();
        scheduled(&w, "e1", None, Some(T0), "published", None);
        assert_eq!(opened(&w, o), 0);
    }

    #[test]
    fn a_student_with_any_attempt_is_left_alone() {
        let w = world();
        let o = T0 + 5 * H;
        scheduled(&w, "e1", Some(o), Some(T0), "published", None);
        w.conn.execute_batch("UPDATE assessments SET max_attempts = 3").unwrap();
        for (id, who, status) in [("t1", &w.a, "in_progress"), ("t2", &w.b, "submitted"), ("t3", &w.both, "expired")] {
            w.conn.execute("INSERT INTO attempts(id,assessment_id,student_id,started_at,status) VALUES (?1,'e1',?2,?3,?4)", params![id, who.id, o - H, status]).unwrap();
        }
        assert_eq!(opened(&w, o), 0, "all three enrolled, active students have an attempt of some status");
        let _ = start_attempt; // (the real start path is covered by the closing-reminder tests)
        assert_eq!(marker_count(&w), 0);
    }

    #[test]
    fn a_draft_closed_archived_or_hidden_exam_announces_nothing_and_a_restored_one_catches_up() {
        let o = T0 + 5 * H;
        for status in ["draft", "closed", "archived"] {
            let w = world();
            scheduled(&w, "e1", Some(o), Some(T0), status, None);
            assert_eq!(opened(&w, o), 0, "{status}");
        }
        let w = world();
        scheduled(&w, "e1", Some(o), Some(T0), "published", None);
        for (off, on) in [("UPDATE subjects SET is_active = 0 WHERE id = 's1'", "UPDATE subjects SET is_active = 1 WHERE id = 's1'"), ("UPDATE institutions SET is_active = 0", "UPDATE institutions SET is_active = 1")] {
            w.conn.execute(off, []).unwrap();
            assert_eq!(opened(&w, o), 0, "{off}");
            w.conn.execute(on, []).unwrap();
        }
        assert_eq!(opened(&w, o + H), 3, "visible again within the day");
    }

    #[test]
    fn a_teachers_exam_needs_its_teacher_in_good_standing() {
        let w = world();
        let o = T0 + 5 * H;
        w.conn
            .execute(
                "INSERT INTO assessments(id,teacher_id,created_by,subject_id,title,questions,question_count,total_points,status,opens_at,published_at,created_at,updated_at)
                 VALUES ('te',?1,?1,'s1','امتحان معلم','[]',1,10,'published',?2,?3,0,0)",
                params![w.teacher.id, o, T0],
            )
            .unwrap();
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected'", []).unwrap();
        assert_eq!(opened(&w, o), 0, "the assignment is gone: students cannot see the exam");
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved'", []).unwrap();
        assert_eq!(opened(&w, o), 3);
    }

    fn req(extra: serde_json::Value) -> ExamReq {
        let mut v = serde_json::json!({
            "subject_id": "s1", "title": "امتحان",
            "questions": [{"id": "q1", "type": "single_choice", "stem": "س1", "options": ["a", "b", "c"], "answer": "B", "score": 1}],
            "status": "published"
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        serde_json::from_value(v).unwrap()
    }

    fn exam_id(d: &impl serde::Serialize) -> String {
        serde_json::to_value(d).unwrap()["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn moving_the_opening_time_re_arms_the_announcement_and_reopening_does_not() {
        let w = world();
        let o = T0 + 5 * H;
        let id = exam_id(&create_exam(&w.conn, &w.admin, &req(serde_json::json!({ "opens_at": o, "closes_at": o + 10 * H })), T0).unwrap());
        assert_eq!(opened(&w, o), 3);
        assert_eq!(opened(&w, o + MIN), 0);
        // the opening time moves later: the markers are cleared, so the new opening is announced again
        let moved = o + 2 * H;
        update_exam(&w.conn, &w.admin, &id, &serde_json::from_value(serde_json::json!({ "opens_at": moved })).unwrap(), T0 + H).unwrap();
        assert_eq!(marker_count(&w), 0, "update_exam cleared the markers");
        assert_eq!(opened(&w, o + MIN), 0, "the old time is no longer the opening time");
        assert_eq!(opened(&w, moved), 3);
        assert_eq!(everyone(&w, "exam_opened"), [2, 2, 2, 0, 0, 0, 0]);
        // saving without moving it does not re-arm
        update_exam(&w.conn, &w.admin, &id, &serde_json::from_value(serde_json::json!({ "title": "عنوان جديد" })).unwrap(), moved + MIN).unwrap();
        assert_eq!(opened(&w, moved + 2 * MIN), 0);
        // closing and reopening it - however many times - does not say "it is open" again: it did not open again
        for cycle in 0..3 {
            let t = moved + (3 + cycle * 4) * MIN;
            act(&w.conn, &w.admin, &id, "close", t).unwrap();
            assert_eq!(opened(&w, t + MIN), 0, "closed");
            act(&w.conn, &w.admin, &id, "reopen", t + 2 * MIN).unwrap();
            assert_eq!(opened(&w, t + 3 * MIN), 0, "cycle {cycle}: reopened");
        }
        assert_eq!(everyone(&w, "exam_opened"), [2, 2, 2, 0, 0, 0, 0], "no extra notice per cycle");
        assert_eq!(marker_count(&w), 3);
    }

    #[test]
    fn the_closing_reminder_does_not_land_on_top_of_the_opening_notice() {
        // announced at T0, opens 12 h later and closes 8 h after that: the closing window (24 h) has been open since
        // before the opening, but the "it is open" notice at the opening is itself an announcement
        let w = world();
        let o = T0 + 12 * H;
        scheduled(&w, "e1", Some(o), Some(T0), "published", Some(o + 8 * H));
        let s = sweep(&w.conn, o);
        assert_eq!((s.exam_opened, s.closing), (3, 0), "just the opening notice at the opening");
        assert_eq!(everyone(&w, "exam_closing"), [0; 7]);
        assert_eq!(sweep(&w.conn, o + 6 * H - 1).closing, 0, "one millisecond before six hours after the opening");
        assert_eq!(sweep(&w.conn, o + 6 * H).closing, 3, "six hours after the opening, with 2 h to go: the reminder");
        assert_eq!(everyone(&w, "exam_closing"), [1, 1, 1, 0, 0, 0, 0]);
        // a window too short to leave six hours after the opening gets no reminder at all
        let w = world();
        scheduled(&w, "e1", Some(o), Some(T0), "published", Some(o + 5 * H));
        for t in [o, o + H, o + 4 * H, o + 5 * H - 1] {
            assert_eq!(sweep(&w.conn, t).closing, 0, "at +{}", t - o);
        }
        // an exam published after it opened is unchanged: six hours after the publication, as before
        let w = world();
        scheduled(&w, "e1", Some(T0), Some(o), "published", Some(o + 20 * H));
        assert_eq!(sweep(&w.conn, o + 6 * H - 1).closing, 0);
        assert_eq!(sweep(&w.conn, o + 6 * H).closing, 3);
    }

    #[test]
    fn the_notifications_of_a_new_exam_and_its_opening_are_never_back_to_back() {
        // an exam published after it already opened gets the "new exam" notice only
        let w = world();
        let o = T0 + 5 * H;
        let id = exam_id(&create_exam(&w.conn, &w.admin, &req(serde_json::json!({ "opens_at": o })), o + 10 * MIN).unwrap());
        assert!(!id.is_empty());
        assert_eq!(opened(&w, o + 11 * MIN), 0);
        assert_eq!(everyone(&w, "exam_opened"), [0; 7]);
        assert_eq!(everyone(&w, "new_assessment"), [1, 1, 1, 0, 0, 0, 0]);
    }

    // ───────── the table ─────────

    const OLD_EXAM_REMINDERS: &str = "
        CREATE TABLE exam_reminders (
          assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
          student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
          kind TEXT NOT NULL CHECK (kind IN ('closing','results')),
          sent_at INTEGER NOT NULL,
          PRIMARY KEY (assessment_id, student_id, kind)
        );
        CREATE INDEX idx_reminders_student ON exam_reminders(student_id);";

    /// The database as the previous server wrote it (the real fixture), with the two-kind `exam_reminders` table of phases
    /// 1-8 .. 4-1 and some markers in it.
    fn old_database() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        c.execute_batch(include_str!("../tests/fixtures/platform_v1.sql")).unwrap();
        // bring the rest of the schema to the 4-1 state first (the fixture predates exam_reminders entirely)
        crate::platform::apply_schema(&c).unwrap();
        c.execute_batch("DROP TABLE exam_reminders;").unwrap();
        c.execute_batch(OLD_EXAM_REMINDERS).unwrap();
        let (exam, student): (String, String) = c.query_row("SELECT id, teacher_id FROM assessments ORDER BY id LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        c.execute("INSERT INTO exam_reminders(assessment_id, student_id, kind, sent_at) VALUES (?1,?2,'closing',11), (?1,?2,'results',22)", params![exam, student]).unwrap();
        c.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        c
    }

    /// The stored DDL; SQLite quotes the name of a table that was renamed into place, which is not a difference.
    fn table_sql(c: &Connection) -> String {
        c.query_row::<String, _, _>("SELECT sql FROM sqlite_master WHERE name = 'exam_reminders'", [], |r| r.get(0)).unwrap().replace('"', "")
    }

    #[test]
    fn an_older_exam_reminders_table_is_widened_without_losing_a_marker() {
        let c = old_database();
        assert!(!table_sql(&c).contains("'opened'"), "the fixture really has the old CHECK");
        let rows = |c: &Connection| -> Vec<(String, i64)> {
            c.prepare("SELECT kind, sent_at FROM exam_reminders ORDER BY kind").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
        };
        let before = rows(&c);
        assert_eq!(before, vec![("closing".to_string(), 11), ("results".to_string(), 22)]);
        let (exam, student): (String, String) = c.query_row("SELECT assessment_id, student_id FROM exam_reminders LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert!(c.execute("INSERT INTO exam_reminders VALUES (?1,?2,'opened',33)", params![exam, student]).is_err(), "the old table refuses the new kind");

        crate::platform::apply_schema(&c).unwrap();

        assert!(table_sql(&c).contains("'opened'"));
        assert_eq!(rows(&c), before, "every marker survived, byte for byte");
        c.execute("INSERT INTO exam_reminders VALUES (?1,?2,'opened',33)", params![exam, student]).unwrap();
        assert!(c.execute("INSERT INTO exam_reminders VALUES (?1,?2,'bogus',1)", params![exam, student]).is_err(), "the CHECK is still there");
        assert!(c.execute("INSERT INTO exam_reminders VALUES (?1,?2,'opened',44)", params![exam, student]).is_err(), "and so is the primary key");
        let violations: i64 = c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
        assert_eq!(violations, 0);
        assert_eq!(c.query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0)).unwrap(), "ok");
        assert_eq!(c.query_row::<i64, _, _>("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap(), 1, "foreign keys are back on");
        let leftover: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'exam_reminders_new'", [], |r| r.get(0)).unwrap();
        assert_eq!(leftover, 0, "no temp table left behind");
        let index: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'idx_reminders_student'", [], |r| r.get(0)).unwrap();
        assert_eq!(index, 1, "the index is back");
        // cascade still works on the rebuilt table
        c.execute("DELETE FROM users WHERE id = ?1", params![student]).unwrap();
        assert_eq!(c.query_row::<i64, _, _>("SELECT count(*) FROM exam_reminders WHERE student_id = ?1", params![student], |r| r.get(0)).unwrap(), 0);
    }

    #[test]
    fn the_exam_reminders_migration_is_idempotent_and_a_fresh_install_needs_none() {
        let c = old_database();
        crate::platform::apply_schema(&c).unwrap();
        let once = table_sql(&c);
        crate::platform::apply_schema(&c).unwrap();
        crate::platform::apply_schema(&c).unwrap();
        assert_eq!(table_sql(&c), once);
        let fresh = create_test_db();
        assert_eq!(table_sql(&fresh), once, "an upgraded database has exactly the table of a fresh install");
        migrate(&fresh).unwrap();
        assert_eq!(table_sql(&fresh), once);
        // a database that never had the table (the pre-1-8 fixture before the schema runs) is not an error
        let bare = Connection::open_in_memory().unwrap();
        migrate(&bare).unwrap();
    }
}
