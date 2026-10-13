//! Practice mode (phase 4-3): `GET /api/platform/assessments/{id}/practice`.
//!
//! A teacher (or the admin, for an admin exam) can switch practice ON for an exam (`assessments.allow_practice`, off by
//! default). An enrolled student can then run it as an **ungraded practice**: the page gets every question together with its
//! correct answer and explanation and checks the student's answers in the browser (`utils/answerCheck.ts`).
//!
//! The endpoint is a pure read. It writes **nothing** - no attempt row (so no attempt is used and nothing is graded), no audit
//! row, no notification, no statistic - and no result, score, statistic, reminder or attempt limit anywhere on the platform
//! depends on it. The only thing it touches is the per-user request counter that keeps a runaway client at 30 requests a
//! minute (`rate_limits`, which is not exam data).
//!
//! Who may read it (every condition decided here, from the session and the database, never from the request):
//! * an active **student** (`require_role`), enrolled in the exam's subject (403 `not_enrolled`);
//! * the exam must be one students can see - `published` or `closed`, subject and institution active, its teacher in good
//!   standing (`platform_exams::publicly_visible`, the rule the exam page uses): a draft, an archived or a hidden exam answers
//!   404 `not_found`, exactly like the exam page does;
//! * the switch must be on (403 `practice_disabled`);
//! * the key must not be a secret any more at this moment: the owner's switch shows the answers to enrolled students, but
//!   never while the exam is still being sat or not yet open. So it answers 409 `attempt_running` while the student has a
//!   graded attempt of THIS exam in progress (a second tab must not read the key of the sitting it is in), and 403
//!   `practice_not_available` before the exam opens (`opens_at`) and, for `after_close` exams, until the results are
//!   released (`is_released`: the exam is closed, or its closing time and the grace after it have passed - the last
//!   attempt has landed). Those checks are pure reads; nothing is settled or written.
//!
//! A closed exam can be practised, and so can one whose graded attempts are used up: practice never touches either. The
//! questions come in the exam's ORIGINAL order and option letters (shuffling belongs to graded attempts), and `answer` is
//! exactly what is stored - a fill-in-the-blank key keeps its `a|b` alternatives.

use crate::platform::{lock, rate_limit, require_role, Res, User};
use crate::platform_exams::{end_of, get_info, is_enrolled, is_released, load_questions, points_of, publicly_visible, AssessmentInfo, GRACE_MS};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use exameow_core::exam::QuestionType;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Arc;

/// Requests per user per minute (a page asks once; this stops a loop, not a student).
const PRACTICE_PER_MINUTE: i64 = 30;

#[derive(Serialize, Debug)]
pub struct PracticeQuestion {
    pub id: String,
    #[serde(rename = "type")]
    pub qtype: QuestionType,
    pub stem: String,
    pub options: Vec<String>,
    pub points: f64,
    /// The key exactly as stored (`B`, `AC`, `صح`, `القاهرة|مصر`, a model answer...).
    pub answer: String,
    /// The explanation, as stored (empty when the author wrote none).
    pub analysis: String,
    pub chapter: Option<String>,
    /// `easy` | `medium` | `hard`.
    pub difficulty: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct PracticeExam {
    pub id: String,
    pub title: String,
    pub subject_id: String,
    pub subject_name: String,
    pub question_count: i64,
    pub total_points: f64,
    pub questions: Vec<PracticeQuestion>,
}

/// Whether `student` has a graded attempt of the exam in progress at `now`: started, not handed in, and not yet past its end
/// plus the grace (an overdue one is waiting to be settled and is not "running" - and reading that must not settle it).
fn has_running_attempt(conn: &Connection, student: &User, info: &AssessmentInfo, now: i64) -> Res<bool> {
    let started: Vec<i64> = conn
        .prepare("SELECT started_at FROM attempts WHERE assessment_id = ?1 AND student_id = ?2 AND status = 'in_progress'")
        .map_err(crate::platform::db_err)?
        .query_map(params![info.id, student.id], |r| r.get(0))
        .map_err(crate::platform::db_err)?
        .collect::<Result<_, _>>()
        .map_err(crate::platform::db_err)?;
    Ok(started.into_iter().any(|s| now <= end_of(info, s) + GRACE_MS))
}

/// The exam for practice at `now`. Read-only; see the module docs for who may call it and when.
pub(crate) fn practice_exam_at(conn: &Connection, student: &User, assessment_id: &str, now: i64) -> Res<PracticeExam> {
    // a hidden, draft or archived exam does not exist for a student (the same answer as the exam page)
    if !publicly_visible(conn, assessment_id)? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let info = get_info(conn, &student.id, assessment_id)?;
    if !is_enrolled(conn, &student.id, &info.subject_id)? {
        return Err(err(StatusCode::FORBIDDEN, "not_enrolled"));
    }
    if !info.allow_practice {
        return Err(err(StatusCode::FORBIDDEN, "practice_disabled"));
    }
    // the key is not handed out during the student's own sitting, before the exam opens, or under the results embargo
    if has_running_attempt(conn, student, &info, now)? {
        return Err(err(StatusCode::CONFLICT, "attempt_running"));
    }
    let not_open_yet = info.status == "published" && info.opens_at.map_or(false, |o| now < o);
    if not_open_yet || !is_released(&info.release_mode, &info.status, info.closes_at, now) {
        return Err(err(StatusCode::FORBIDDEN, "practice_not_available"));
    }
    let questions = load_questions(conn, assessment_id)?;
    let questions: Vec<PracticeQuestion> = questions
        .into_iter()
        .map(|q| PracticeQuestion {
            points: points_of(&q),
            difficulty: q.difficulty.as_ref().map(|d| d.to_string()),
            id: q.id,
            qtype: q.qtype,
            stem: q.stem,
            options: q.options,
            answer: q.answer,
            analysis: q.analysis,
            chapter: q.chapter,
        })
        .collect();
    Ok(PracticeExam {
        id: info.id,
        title: info.title,
        subject_id: info.subject_id,
        subject_name: info.subject_name,
        question_count: questions.len() as i64,
        total_points: info.total_points,
        questions,
    })
}

pub async fn practice_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<PracticeExam>> {
    let student = require_role(&s, &h, "student")?;
    let conn = lock(&s)?;
    rate_limit(&conn, &format!("practice:{}", student.id), 60_000, PRACTICE_PER_MINUTE)?;
    practice_exam_at(&conn, &student, &id, now_ms()).map(Json)
}

/// The practice route (merged into the app in `main.rs`; private by default through the auth gate).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/platform/assessments/{id}/practice", get(practice_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::insert_test_user;
    use rusqlite::params;
    use crate::platform_exam_admin::{act, update_exam, ExamReq};
    use crate::platform_exams::fixtures::*;
    use crate::platform_exams::{attempt_result, list_available, start_attempt, submit_attempt};
    use serde_json::json;

    fn code(r: Res<PracticeExam>) -> (u16, String) {
        fail(r)
    }

    /// Practice one millisecond after the exams of these tests were made (they sit at [`NOW`]).
    fn practice_exam(conn: &Connection, student: &User, id: &str) -> Res<PracticeExam> {
        practice_exam_at(conn, student, id, NOW + 1)
    }

    fn switch(w: &W, who: &User, id: &str, on: bool) {
        let r: ExamReq = serde_json::from_value(json!({ "allow_practice": on })).unwrap();
        update_exam(&w.conn, who, id, &r, NOW + 1).unwrap();
    }

    #[test]
    fn practice_is_off_until_the_owner_switches_it_on() {
        let w = world();
        let id = exam(&w, json!({}));
        assert_eq!(code(practice_exam(&w.conn, &w.student, &id)), (403, "practice_disabled".into()), "off by default");
        switch(&w, &w.admin, &id, true);
        let p = practice_exam(&w.conn, &w.student, &id).unwrap();
        assert_eq!((p.id.as_str(), p.title.as_str(), p.question_count, p.total_points, p.subject_id.as_str()), (id.as_str(), "امتحان", 5, 9.0, "s1"));
        switch(&w, &w.admin, &id, false);
        assert_eq!(code(practice_exam(&w.conn, &w.student, &id)), (403, "practice_disabled".into()), "and off again");
        // a teacher's exam works the same way
        let theirs = exam_by(&w, &w.teacher, json!({"title": "للمعلم", "allow_practice": true}));
        assert!(practice_exam(&w.conn, &w.student, &theirs).is_ok());
    }

    #[test]
    fn it_follows_the_exam_pages_visibility_and_needs_an_enrolled_student() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true}));
        let outsider = insert_test_user(&w.conn, "out@x.com", "student", "active");
        assert_eq!(code(practice_exam(&w.conn, &outsider, &id)), (403, "not_enrolled".into()));
        assert_eq!(code(practice_exam(&w.conn, &w.student, "ghost")), (404, "not_found".into()));
        // a draft, an archived and a hidden exam do not exist for a student
        let draft = exam(&w, json!({"title": "مسودة", "status": "draft", "allow_practice": true}));
        assert_eq!(code(practice_exam(&w.conn, &w.student, &draft)), (404, "not_found".into()));
        let old = exam(&w, json!({"title": "قديم", "allow_practice": true}));
        act(&w.conn, &w.admin, &old, "close", NOW + 2).unwrap();
        assert!(practice_exam(&w.conn, &w.student, &old).is_ok(), "a closed exam can be practised");
        act(&w.conn, &w.admin, &old, "archive", NOW + 3).unwrap();
        assert_eq!(code(practice_exam(&w.conn, &w.student, &old)), (404, "not_found".into()), "archived: gone");
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        assert_eq!(code(practice_exam(&w.conn, &w.student, &id)), (404, "not_found".into()), "a hidden institution hides it");
        w.conn.execute("UPDATE institutions SET is_active = 1", []).unwrap();
        w.conn.execute("UPDATE subjects SET is_active = 0 WHERE id = 's1'", []).unwrap();
        assert_eq!(code(practice_exam(&w.conn, &w.student, &id)), (404, "not_found".into()), "and so does a hidden subject");
        w.conn.execute("UPDATE subjects SET is_active = 1 WHERE id = 's1'", []).unwrap();
        // a teacher whose assignment was withdrawn: the teacher's exam is hidden like everywhere else
        let theirs = exam_by(&w, &w.teacher, json!({"title": "للمعلم", "allow_practice": true}));
        assert!(practice_exam(&w.conn, &w.student, &theirs).is_ok());
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1", params![w.teacher.id]).unwrap();
        assert_eq!(code(practice_exam(&w.conn, &w.student, &theirs)), (404, "not_found".into()));
    }

    #[test]
    fn it_works_when_the_graded_attempts_are_used_up_or_the_exam_is_over() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true, "max_attempts": 1, "closes_at": NOW + 10 * MIN}));
        take(&w, &w.student, &id, NOW, MIN, &objective_right());
        assert_eq!(fail(start_attempt(&w.conn, &w.student, &id, NOW + 2 * MIN)).1, "no_attempts_left");
        assert!(practice_exam(&w.conn, &w.student, &id).is_ok(), "attempts used up: practice is still there");
        // past the closing time (the exam reads as closed without anyone pressing a button)
        w.conn.execute("UPDATE assessments SET closes_at = 1 WHERE id = ?1", params![id]).unwrap();
        assert!(practice_exam(&w.conn, &w.student, &id).is_ok(), "an exam whose closing time has gone by can still be practised");
    }

    #[test]
    fn the_questions_come_in_the_original_order_and_letters_with_the_key_exactly_as_stored() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true, "shuffle_questions": true, "shuffle_options": true}));
        // many graded attempts are dealt shuffled orders; the practice read never is (a student reads it once their own
        // sitting is over - while it runs the key is not handed out, see `no_key_while_the_students_own_attempt_is_running`)
        for seed in 0..6 {
            let who = enroll(&w, &format!("sh{seed}@x.com"));
            take(&w, &who, &id, NOW, MIN, &objective_right());
            let p = practice_exam_at(&w.conn, &who, &id, NOW + 2 * MIN).unwrap();
            assert_eq!(p.questions.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), ["q1", "q2", "q3", "q4", "q5"], "original order");
            assert_eq!(p.questions[0].options, ["o1-0", "o1-1", "o1-2", "o1-3"], "original option order, so the letters mean what the key says");
        }
        let p = practice_exam(&w.conn, &w.student, &id).unwrap();
        let by = |qid: &str| p.questions.iter().find(|q| q.id == qid).unwrap();
        assert_eq!((by("q1").answer.as_str(), by("q2").answer.as_str(), by("q3").answer.as_str()), ("B", "AC", "A"));
        assert_eq!(by("q4").answer, "القاهرة|مصر", "alternatives are kept");
        assert_eq!((by("q5").answer.as_str(), by("q5").analysis.as_str(), by("q5").points), ("نموذج", "الإجابة النموذجية", 4.0));
        assert_eq!((by("q1").analysis.as_str(), by("q2").analysis.as_str()), ("شرح 1", ""), "the explanation as stored, empty when there is none");
        assert_eq!((by("q1").chapter.as_deref(), by("q1").difficulty.as_deref(), by("q4").difficulty.as_deref()), (Some("ف1"), Some("easy"), None));
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["questions"][0]["type"], json!("single_choice"));
        for key in ["id", "type", "stem", "options", "points", "answer", "analysis", "chapter", "difficulty"] {
            assert!(json["questions"][0].get(key).is_some(), "{key}");
        }
        for key in ["id", "title", "subject_id", "subject_name", "question_count", "total_points", "questions"] {
            assert!(json.get(key).is_some(), "{key}");
        }
    }

    /// The point of the whole mode: reading it leaves no trace anywhere - no attempt, no audit row, no notification, no
    /// counter, no reminder - and no result, score, list or statistic depends on it.
    #[test]
    fn it_writes_nothing_at_all_and_changes_no_result_list_or_limit() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true, "max_attempts": 2}));
        let _ = take(&w, &w.student, &id, NOW, MIN, &objective_right());
        let lists = |w: &W| {
            (
                crate::platform_student::student_exams(&w.conn, &w.student.id, None, None, NOW + 2 * MIN).unwrap().iter().map(|r| (r.attempts_used, format!("{:?}", r.state))).collect::<Vec<_>>(),
                list_available(&w.conn, &w.student.id).unwrap().len(),
                crate::platform_student_stats::my_stats(&w.conn, &w.student, NOW + 2 * MIN).unwrap(),
            )
        };
        let (db_before, lists_before) = (dump_all(&w.conn), lists(&w));
        for _ in 0..5 {
            practice_exam(&w.conn, &w.student, &id).unwrap();
        }
        assert_eq!(dump_all(&w.conn), db_before, "not one row of any table changed");
        assert_eq!(lists(&w), lists_before, "no list, count or statistic moved");
        // the graded attempts are all still there to be used
        let s = start_attempt(&w.conn, &w.student, &id, NOW + 3 * MIN).unwrap();
        assert!(!s.resumed);
        assert!(submit_attempt(&w.conn, &w.student, &s.attempt_id, &all_right(), NOW + 4 * MIN).is_ok());
        assert_eq!(serde_json::to_value(attempt_result(&w.conn, &w.student, &s.attempt_id).unwrap()).unwrap()["attempt_no"], json!(2), "the practice reads were not counted as attempts");
    }

    #[test]
    fn switching_it_on_or_off_is_what_the_owner_decides_and_nothing_else() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true}));
        let before = practice_exam(&w.conn, &w.student, &id).unwrap();
        // editing something else (absent field) leaves the switch as it was
        update_exam(&w.conn, &w.admin, &id, &serde_json::from_value::<ExamReq>(json!({"title": "عنوان جديد"})).unwrap(), NOW + 1).unwrap();
        let after = practice_exam(&w.conn, &w.student, &id).unwrap();
        assert_eq!((after.title.as_str(), after.question_count), ("عنوان جديد", before.question_count));
    }

    // ───────── the key is not handed out while it is still a secret ─────────

    fn at(w: &W, who: &User, id: &str, now: i64) -> Res<PracticeExam> {
        practice_exam_at(&w.conn, who, id, now)
    }

    #[test]
    fn no_key_while_the_students_own_attempt_is_running() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true, "duration_min": 30}));
        let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
        assert_eq!(code(at(&w, &w.student, &id, NOW + 5 * MIN)), (409, "attempt_running".into()), "a second tab cannot read the key of the sitting it is in");
        // a classmate who is not sitting it can still revise
        let other = enroll(&w, "o@x.com");
        assert!(at(&w, &other, &id, NOW + 5 * MIN).is_ok());
        // the sitting is handed in: the key is theirs to read
        submit_attempt(&w.conn, &w.student, &s.attempt_id, &objective_right(), NOW + 6 * MIN).unwrap();
        assert!(at(&w, &w.student, &id, NOW + 7 * MIN).is_ok());
        // a new sitting closes it again, up to its end plus the grace; after that it is overdue, not running
        let again = start_attempt(&w.conn, &w.student, &id, NOW + 8 * MIN).unwrap();
        let end = again.ends_at;
        assert_eq!(code(at(&w, &w.student, &id, end + GRACE_MS)), (409, "attempt_running".into()), "still inside the grace");
        let before = dump_all(&w.conn);
        assert!(at(&w, &w.student, &id, end + GRACE_MS + 1).is_ok(), "overdue: nothing is running any more");
        assert_eq!(dump_all(&w.conn), before, "…and reading that did not settle the attempt (a practice read writes nothing)");
    }

    #[test]
    fn no_key_before_the_exam_opens() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true, "opens_at": NOW + H}));
        assert_eq!(code(at(&w, &w.student, &id, NOW + 1)), (403, "practice_not_available".into()));
        assert_eq!(code(at(&w, &w.student, &id, NOW + H - 1)), (403, "practice_not_available".into()));
        assert!(at(&w, &w.student, &id, NOW + H).is_ok(), "from the opening time on");
    }

    #[test]
    fn under_the_results_embargo_the_key_waits_for_the_last_attempt_to_land() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true, "release_mode": "after_close", "closes_at": NOW + H}));
        assert_eq!(code(at(&w, &w.student, &id, NOW + 10 * MIN)), (403, "practice_not_available".into()), "open: classmates are still sitting it");
        assert_eq!(code(at(&w, &w.student, &id, NOW + H + GRACE_MS)), (403, "practice_not_available".into()), "the closing time has passed but the last save may still land");
        assert!(at(&w, &w.student, &id, NOW + H + GRACE_MS + 1).is_ok(), "released: nobody can still hand in");
        // closing by hand ends every running attempt at once, so the embargo is over at that moment
        let early = exam(&w, json!({"title": "مبكر", "allow_practice": true, "release_mode": "after_close", "closes_at": NOW + H}));
        assert_eq!(code(at(&w, &w.student, &early, NOW + 2)), (403, "practice_not_available".into()));
        act(&w.conn, &w.admin, &early, "close", NOW + 3).unwrap();
        assert!(at(&w, &w.student, &early, NOW + 4).is_ok());
        // an exam that shows results at once has no embargo to wait for
        let now_released = exam(&w, json!({"title": "فوري", "allow_practice": true}));
        assert!(at(&w, &w.student, &now_released, NOW + 2).is_ok());
    }

    /// The owner's switch is an explicit choice to show the key to enrolled students, so it also overrides
    /// `show_answers = false` (which only hides the key from the attempt review); the editor says so next to the switch.
    #[test]
    fn the_owners_switch_is_an_explicit_choice_even_when_the_review_hides_the_answers() {
        let w = world();
        let id = exam(&w, json!({"allow_practice": true, "show_answers": false}));
        assert!(at(&w, &w.student, &id, NOW + 1).is_ok());
    }
}
