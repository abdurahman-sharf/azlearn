//! Answer-key correction (phase 3-3): fixing a wrong key, or dropping a faulty question, AFTER students have sat the
//! exam — without a re-sit and without losing the grades a teacher wrote by hand.
//!
//! One function ([`correct_question`]) serves both route sets (`/teacher/exams/{id}/questions/{qid}/correct` and
//! `/admin/exams/{id}/questions/{qid}/correct`). Who may call it: the **owner** of the exam only — a teacher on their
//! own exam, an admin on an admin-created exam (an admin only moderates a teacher's exam) — and never on an archived or
//! locked exam. Two modes:
//!
//! * `set` — a new answer and/or points and/or explanation for the question. The answer is checked exactly as when the
//!   exam was saved (`clean_exam_question`), so the key stays gradable.
//! * `void` — the question stops counting for everybody: its points become 0 (it stays listed; `set` with `score > 0`
//!   brings it back).
//!
//! Every **submitted** attempt is re-evaluated for that one question from the answers it stored (always in the original
//! option letters, so shuffled exams need no mapping). Objective questions are re-marked against the new key; a short
//! answer keeps the grade its teacher gave (a still-ungraded one stays pending), clamped to the new maximum.
//! In-progress and expired attempts are left alone: an in-progress attempt is simply graded with the new key when it is
//! submitted. All writes (the exam, every attempt, the notifications, the audit row) happen in ONE transaction;
//! `dry_run` computes the same numbers and writes nothing, so the UI can say "this changes N attempts" first.

use crate::platform::{bad, db_err, lock, rate_limit, require_admin, require_role, Res, User};
use crate::platform_bank::clean_exam_question;
use crate::platform_engage::{upsert_unread, Fold};
use crate::platform_exams::{get_info, grade_one, is_released, load_questions, points_of, round2, Outcome};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use exameow_core::exam::{Question, QuestionType};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Deserialize, Default, Debug, Clone)]
pub struct CorrectReq {
    /// `set` or `void`.
    pub(crate) mode: Option<String>,
    /// `set`: the new answer, in the question's own form (choice letters, `A`/`B` for true/false, text for the rest).
    pub(crate) answer: Option<String>,
    /// `set`: the new points (0–100; 0 voids the question).
    pub(crate) score: Option<f64>,
    /// `set`: the new explanation.
    pub(crate) analysis: Option<String>,
    /// Compute and report, write nothing.
    pub(crate) dry_run: Option<bool>,
}

#[derive(Serialize, Debug)]
pub struct CorrectResult {
    pub(crate) mode: String,
    /// The question as it is now (after a dry run: as it would be).
    pub(crate) question: Question,
    pub(crate) old_total: f64,
    pub(crate) new_total: f64,
    /// Submitted attempts of the exam (the ones re-evaluated).
    pub(crate) attempts_total: i64,
    /// Attempts whose score changed, and how many distinct students they belong to.
    pub(crate) affected_attempts: i64,
    pub(crate) affected_students: i64,
    pub(crate) up: i64,
    pub(crate) down: i64,
    /// `attempts_total - affected_attempts`.
    pub(crate) unchanged: i64,
    /// Attempts whose outcome for this question changed (its mark, points or maximum), whether or not the total did.
    pub(crate) changed_outcomes: i64,
    pub(crate) dry_run: bool,
}

/// The question after the correction (validated), or the error to answer with.
fn next_question(old: &Question, mode: &str, r: &CorrectReq) -> Res<Question> {
    let mut q = old.clone();
    if mode == "void" {
        // Voiding must work on a question whose stored key is itself the problem, so the answer is not re-checked.
        q.score = Some(0.0);
        return Ok(q);
    }
    if r.answer.is_none() && r.score.is_none() && r.analysis.is_none() {
        return Err(bad("invalid_question")); // nothing to set
    }
    if let Some(s) = r.score {
        if !s.is_finite() || !(0.0..=100.0).contains(&s) {
            return Err(bad("invalid_points"));
        }
        q.score = Some(round2(s));
    }
    if let Some(a) = &r.answer {
        q.answer = a.clone();
    }
    if let Some(a) = &r.analysis {
        q.analysis = a.clone();
    }
    // the answer is normalised / validated exactly like a question being saved into an exam
    clean_exam_question(&q)
}

/// One submitted attempt re-evaluated for the corrected question.
struct Change {
    attempt_id: String,
    student_id: String,
    old_score: f64,
    new_score: f64,
    old_pending: i64,
    new_pending: i64,
    outcomes: Vec<Outcome>,
    outcome_changed: bool,
}

impl Change {
    fn score_changed(&self) -> bool {
        (self.new_score - self.old_score).abs() > 1e-9
    }
    /// Something stored for this attempt has to be written.
    fn dirty(&self) -> bool {
        self.outcome_changed || self.score_changed() || self.new_pending != self.old_pending
    }
}

/// The outcome of `new_q` for a student who wrote `answer`, given the `old` outcome stored for the previous version of
/// the question. A manual grade is the only thing that cannot be recomputed, so it is carried over — through a void too:
/// while the question is worth 0 points the grade is set aside in `kept`, and counting the question again hands it back
/// (clamped to the new maximum), so voiding and restoring never costs a teacher the grading they already did.
fn regrade(new_q: &Question, old: Option<&Outcome>, answer: Option<&str>) -> Outcome {
    let mut fresh = grade_one(new_q, answer);
    let written = answer.map_or(false, |a| !a.trim().is_empty());
    if new_q.qtype != QuestionType::ShortAnswer || !written {
        return fresh; // objective questions are recomputed; a blank answer is simply 0
    }
    // the grade a teacher gave this written answer: the live one, or the one an earlier void set aside
    let manual = old.and_then(|o| if o.max > 0.0 { o.correct.map(|_| o.points) } else { o.kept });
    match manual {
        Some(p) if fresh.max > 0.0 => {
            let points = round2(p.clamp(0.0, fresh.max));
            fresh = Outcome::auto(fresh.id, Some(points >= fresh.max), points, fresh.max);
        }
        Some(p) => fresh.kept = Some(p), // voided: nothing counts now, but the grade is remembered
        None => {}
    }
    // what a grader wrote by hand travels with the grade: who graded it and when, and the comment on the answer
    // (a comment can exist on an answer that is still pending, and survives a void like the grade does)
    if let Some(o) = old {
        fresh.graded_by = o.graded_by.clone();
        fresh.graded_at = o.graded_at;
        fresh.feedback = o.feedback.clone();
    }
    fresh
}

fn same_outcome(a: &Outcome, b: &Outcome) -> bool {
    a.correct == b.correct && (a.points - b.points).abs() < 1e-9 && (a.max - b.max).abs() < 1e-9 && a.kept == b.kept && a.graded_by == b.graded_by && a.graded_at == b.graded_at && a.feedback == b.feedback
}

fn plan(conn: &Connection, exam_id: &str, new_q: &Question) -> Res<Vec<Change>> {
    let rows: Vec<(String, String, f64, i64, String, String)> = conn
        .prepare("SELECT id, student_id, score, pending, answers, results FROM attempts WHERE assessment_id = ?1 AND status = 'submitted' ORDER BY id")
        .map_err(db_err)?
        .query_map(params![exam_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for (attempt_id, student_id, old_score, old_pending, answers_raw, results_raw) in rows {
        let answers: HashMap<String, String> = serde_json::from_str(&answers_raw).unwrap_or_default();
        // unreadable stored results would be overwritten with a guess: fail the whole correction instead
        let mut outcomes: Vec<Outcome> = serde_json::from_str(&results_raw).map_err(db_err)?;
        let new = regrade(new_q, outcomes.iter().find(|o| o.id == new_q.id), answers.get(&new_q.id).map(String::as_str));
        let outcome_changed = match outcomes.iter_mut().find(|o| o.id == new_q.id) {
            Some(slot) => {
                let changed = !same_outcome(slot, &new);
                *slot = new;
                changed
            }
            None => {
                outcomes.push(new);
                true
            }
        };
        let new_score = round2(outcomes.iter().map(|o| o.points).sum());
        let new_pending = outcomes.iter().filter(|o| o.correct.is_none()).count() as i64;
        out.push(Change { attempt_id, student_id, old_score, new_score, old_pending, new_pending, outcomes, outcome_changed });
    }
    Ok(out)
}

fn clip(s: &str) -> String {
    s.chars().take(300).collect()
}

/// Tells a student their score changed - once per attempt, however often it changes. An UNREAD notice for the same
/// attempt is updated in place and keeps the score the student last saw as its `old` value (flipping a key back and
/// forth, or re-grading an answer twice, would otherwise bury a whole class in notices); when the score ends up where it
/// started, the notice goes away. `cause` is `None` for an answer-key correction (phase 3-3) and `Some("grade")` for a
/// correction of a manual grade (phase 3-4); a notice that was updated by the other cause takes the latest one. Shared by
/// both paths; built on the one coalescing helper of `platform_engage`.
pub(crate) fn tell_score_change(conn: &Connection, student_id: &str, attempt_id: &str, title: &str, old_score: f64, new_score: f64, total: f64, cause: Option<&str>, now: i64) -> Res<()> {
    let link = format!("/platform/attempts/{attempt_id}");
    upsert_unread(conn, student_id, "score_changed", &link, now, |current| {
        // the score the student last saw: the `old` of the unread notice, when there is one
        let old = current.as_ref().and_then(|d| d["old"].as_f64()).unwrap_or(old_score);
        if current.is_some() && (new_score - old).abs() <= 1e-9 {
            return Ok(Fold::Remove);
        }
        let mut data = json!({ "title": title, "old": old, "new": new_score, "total": total });
        if let Some(c) = cause {
            data["cause"] = json!(c);
        }
        Ok(Fold::Write(data))
    })
}

/// Corrects one question of an exam; see the module docs. `dry_run` writes nothing.
pub fn correct_question(conn: &Connection, user: &User, exam_id: &str, question_id: &str, r: &CorrectReq, now: i64) -> Res<CorrectResult> {
    let info = get_info(conn, &user.id, exam_id)?;
    // only the owner decides what the key is; a moderator (an admin on a teacher's exam) or a colleague cannot
    if !info.owned_by(user) {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    if info.status == "archived" {
        return Err(err(StatusCode::CONFLICT, "archived"));
    }
    if info.is_locked() {
        return Err(err(StatusCode::CONFLICT, "locked"));
    }
    let mode = r.mode.as_deref().unwrap_or("");
    if !["set", "void"].contains(&mode) {
        return Err(bad("invalid_mode"));
    }
    let mut questions = load_questions(conn, exam_id)?;
    let idx = questions.iter().position(|q| q.id == question_id).ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let old_q = questions[idx].clone();
    let new_q = next_question(&old_q, mode, r)?;
    questions[idx] = new_q.clone();
    let old_total = info.total_points;
    let new_total = round2(questions.iter().map(points_of).sum());

    let changes = plan(conn, exam_id, &new_q)?;
    let affected: Vec<&Change> = changes.iter().filter(|c| c.score_changed()).collect();
    let result = CorrectResult {
        mode: mode.to_string(),
        question: new_q.clone(),
        old_total,
        new_total,
        attempts_total: changes.len() as i64,
        affected_attempts: affected.len() as i64,
        affected_students: affected.iter().map(|c| c.student_id.as_str()).collect::<HashSet<_>>().len() as i64,
        up: affected.iter().filter(|c| c.new_score > c.old_score).count() as i64,
        down: affected.iter().filter(|c| c.new_score < c.old_score).count() as i64,
        unchanged: (changes.len() - affected.len()) as i64,
        changed_outcomes: changes.iter().filter(|c| c.outcome_changed).count() as i64,
        dry_run: r.dry_run == Some(true),
    };
    if result.dry_run {
        return Ok(result);
    }

    // One transaction: the exam, every attempt, the notifications and the audit row land together or not at all — the
    // notification and audit writes are checked like the others (the shared `notify`/`audit` helpers deliberately
    // swallow their errors, which would let a correction commit without its trail).
    let released = is_released(&info.release_mode, &info.status, info.closes_at, now);
    conn.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
    let written = (|| -> Res<()> {
        if serde_json::to_string(&old_q).map_err(db_err)? != serde_json::to_string(&new_q).map_err(db_err)? {
            conn.execute(
                "UPDATE assessments SET questions = ?2, total_points = ?3, updated_at = ?4 WHERE id = ?1",
                params![exam_id, serde_json::to_string(&questions).map_err(db_err)?, new_total, now],
            )
            .map_err(db_err)?;
        }
        for c in changes.iter().filter(|c| c.dirty()) {
            conn.execute(
                "UPDATE attempts SET results = ?2, score = ?3, pending = ?4 WHERE id = ?1 AND status = 'submitted'",
                params![c.attempt_id, serde_json::to_string(&c.outcomes).map_err(db_err)?, c.new_score, c.new_pending],
            )
            .map_err(db_err)?;
            // Tell the student only when the result is out now; one whose result is withheld (`after_close`) hears
            // about it with the release notification, as before.
            if c.score_changed() && released {
                tell_score_change(conn, &c.student_id, &c.attempt_id, &info.title, c.old_score, c.new_score, new_total, None, now)?;
            }
        }
        let detail = json!({
            "question_id": question_id,
            "mode": mode,
            "old_answer": clip(&old_q.answer),
            "new_answer": clip(&new_q.answer),
            "old_score": points_of(&old_q),
            "new_score": points_of(&new_q),
            "attempts_total": result.attempts_total,
            "affected_attempts": result.affected_attempts,
            "affected_students": result.affected_students,
            "up": result.up,
            "down": result.down,
            "changed_outcomes": result.changed_outcomes,
        });
        conn.execute(
            "INSERT INTO audit_log(actor_id, target_id, action, detail, created_at) VALUES (?1, ?2, 'exam_key_correct', ?3, ?4)",
            params![user.id, exam_id, detail.to_string(), now],
        )
        .map_err(db_err)?;
        Ok(())
    })();
    match written {
        Ok(()) => conn.execute_batch("COMMIT").map_err(db_err)?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    Ok(result)
}

// ───────── handlers (one function, two route sets) ─────────

fn run(s: &AppState, user: &User, id: &str, qid: &str, r: &CorrectReq) -> Res<Json<CorrectResult>> {
    let conn = lock(s)?;
    // Re-marking every attempt is the heaviest write the builder has: a few per minute per person is plenty.
    rate_limit(&conn, &format!("keyfix:{}", user.id), 60_000, 30)?;
    correct_question(&conn, user, id, qid, r, now_ms()).map(Json)
}

pub async fn admin_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path((id, qid)): Path<(String, String)>, Json(r): Json<CorrectReq>) -> Res<Json<CorrectResult>> {
    let u = require_admin(&s, &h)?;
    run(&s, &u, &id, &qid, &r)
}

pub async fn teacher_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path((id, qid)): Path<(String, String)>, Json(r): Json<CorrectReq>) -> Res<Json<CorrectResult>> {
    let u = require_role(&s, &h, "teacher")?;
    run(&s, &u, &id, &qid, &r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use crate::platform_exam_admin::{act, create_exam, ExamReq};
    use crate::platform_exams::{attempt_result, grade_attempt, results_summary, start_attempt_with, submit_attempt, GradeReq, PublicQuestion, StartRes};
    use rand::{rngs::StdRng, SeedableRng};
    use serde_json::{json, Value};

    const NOW: i64 = 1_000_000_000_000;

    struct W {
        conn: Connection,
        admin: User,
        teacher: User,
        other: User,
        students: Vec<User>,
        exam: String,
    }

    fn base_questions() -> Value {
        json!([
            {"id": "q1", "type": "single_choice", "stem": "س1", "options": ["o1-0", "o1-1", "o1-2"], "answer": "B", "score": 1},
            {"id": "q2", "type": "multi_choice", "stem": "س2", "options": ["o2-0", "o2-1", "o2-2", "o2-3"], "answer": "AC", "score": 2},
            {"id": "q3", "type": "true_false", "stem": "س3", "answer": "A", "score": 1},
            {"id": "q4", "type": "fill_blank", "stem": "س4", "answer": "القاهرة|مصر", "score": 1},
            {"id": "q5", "type": "short_answer", "stem": "س5", "answer": "نموذج", "score": 4}
        ])
    }

    /// A published exam of `teacher` in `s1`, four enrolled students, the teacher and another teacher approved.
    fn world_with(owner_is_admin: bool, extra: Value) -> W {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let other = insert_test_user(&conn, "o@x.com", "teacher", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        for t in [&teacher, &other] {
            conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','approved',0,0)", params![t.id]).unwrap();
        }
        let students: Vec<User> = (0..4).map(|i| insert_test_user(&conn, &format!("st{i}@x.com"), "student", "active")).collect();
        for s in &students {
            conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![s.id]).unwrap();
        }
        let mut v = json!({"subject_id": "s1", "title": "امتحان المفتاح", "questions": base_questions(), "status": "published", "max_attempts": 3});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        let r: ExamReq = serde_json::from_value(v).unwrap();
        let owner = if owner_is_admin { &admin } else { &teacher };
        let exam = create_exam(&conn, owner, &r, NOW).unwrap().info_id();
        W { conn, admin, teacher, other, students, exam }
    }

    fn world() -> W {
        world_with(false, json!({}))
    }

    fn ans(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// One student sits the exam (unshuffled) and submits `answers`.
    fn sit(w: &W, who: usize, answers: &[(&str, &str)], at: i64) -> String {
        let st = start_attempt_with(&w.conn, &w.students[who], &w.exam, at, &mut StdRng::seed_from_u64(7)).unwrap();
        submit_attempt(&w.conn, &w.students[who], &st.attempt_id, &ans(answers), at + 1).unwrap();
        st.attempt_id
    }

    /// The standard cast: s0 mostly right, s1 mostly wrong, s2 mixed, s3 left everything blank.
    fn class(w: &W) -> Vec<String> {
        vec![
            sit(w, 0, &[("q1", "B"), ("q2", "AC"), ("q3", "A"), ("q4", "مصر"), ("q5", "إجابة")], NOW + 1_000),
            sit(w, 1, &[("q1", "C"), ("q2", "AB"), ("q3", "B"), ("q4", "الجيزة")], NOW + 2_000),
            sit(w, 2, &[("q1", "C"), ("q2", "AC"), ("q3", "A"), ("q4", "القاهره"), ("q5", "x")], NOW + 3_000),
            sit(w, 3, &[], NOW + 4_000),
        ]
    }

    fn req(v: Value) -> CorrectReq {
        serde_json::from_value(v).unwrap()
    }

    fn fix(w: &W, who: &User, qid: &str, v: Value) -> Res<CorrectResult> {
        correct_question(&w.conn, who, &w.exam, qid, &req(v), NOW + 50_000)
    }

    fn scores(w: &W, attempts: &[String]) -> Vec<f64> {
        attempts.iter().map(|a| w.conn.query_row("SELECT score FROM attempts WHERE id = ?1", params![a], |r| r.get(0)).unwrap()).collect()
    }

    fn pendings(w: &W, attempts: &[String]) -> Vec<i64> {
        attempts.iter().map(|a| w.conn.query_row("SELECT pending FROM attempts WHERE id = ?1", params![a], |r| r.get(0)).unwrap()).collect()
    }

    fn outcome(w: &W, attempt: &str, qid: &str) -> Outcome {
        let raw: String = w.conn.query_row("SELECT results FROM attempts WHERE id = ?1", params![attempt], |r| r.get(0)).unwrap();
        serde_json::from_str::<Vec<Outcome>>(&raw).unwrap().into_iter().find(|o| o.id == qid).unwrap()
    }

    fn total(w: &W) -> f64 {
        w.conn.query_row("SELECT total_points FROM assessments WHERE id = ?1", params![w.exam], |r| r.get(0)).unwrap()
    }

    fn count(w: &W, sql: &str) -> i64 {
        w.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn fail<T: std::fmt::Debug>(r: Res<T>) -> (u16, String) {
        let e = r.unwrap_err();
        (e.0.as_u16(), serde_json::from_str::<Value>(&e.1).unwrap()["error"].as_str().unwrap_or("").to_string())
    }

    /// Every row of the tables a correction may touch, as text: equal dumps = nothing was written.
    fn dump(conn: &Connection) -> String {
        let mut out = String::new();
        for t in ["assessments", "attempts", "notifications", "audit_log"] {
            let mut st = conn.prepare(&format!("SELECT * FROM {t} ORDER BY 1")).unwrap();
            let n = st.column_count();
            let rows = st
                .query_map([], |r| {
                    let mut s = String::new();
                    for i in 0..n {
                        s += &format!("{:?}|", r.get::<_, rusqlite::types::Value>(i).unwrap());
                    }
                    Ok(s)
                })
                .unwrap();
            for r in rows {
                out += &r.unwrap();
                out.push('\n');
            }
        }
        out
    }

    // ───── set: the new key regrades exactly the affected attempts ─────

    #[test]
    fn a_new_answer_regrades_only_the_attempts_that_change_for_every_objective_type() {
        let w = world();
        let at = class(&w);
        assert_eq!(scores(&w, &at), vec![5.0, 0.0, 3.0, 0.0], "starting point");
        // (question, new key, expected score change per student)
        let steps: Vec<(&str, &str, [f64; 4])> = vec![
            ("q1", "C", [-1.0, 1.0, 1.0, 0.0]),     // single choice B → C
            ("q2", "AB", [-2.0, 2.0, -2.0, 0.0]),   // multi choice AC → AB
            ("q3", "B", [-1.0, 1.0, -1.0, 0.0]),    // true/false A → B
            ("q4", "الجيزة", [-1.0, 1.0, 0.0, 0.0]), // fill blank (alternatives replaced)
        ];
        for (qid, key, delta) in steps {
            let before = scores(&w, &at);
            let r = fix(&w, &w.teacher, qid, json!({"mode": "set", "answer": key})).unwrap();
            let after = scores(&w, &at);
            let got: Vec<f64> = before.iter().zip(&after).map(|(b, a)| a - b).collect();
            assert_eq!(got, delta.to_vec(), "{qid} → {key}");
            let changed = delta.iter().filter(|d| **d != 0.0).count() as i64;
            assert_eq!((r.attempts_total, r.affected_attempts, r.affected_students, r.unchanged), (4, changed, changed, 4 - changed), "{qid}");
            assert_eq!((r.up, r.down), (delta.iter().filter(|d| **d > 0.0).count() as i64, delta.iter().filter(|d| **d < 0.0).count() as i64), "{qid}");
            assert_eq!((r.old_total, r.new_total, r.dry_run, r.mode.as_str()), (9.0, 9.0, false, "set"), "the points did not change, only the key");
            assert_eq!(r.question.answer, if qid == "q2" { "AB" } else { key }, "the stored key is the normalised answer");
            // the stored outcome carries the right mark and points
            for (i, a) in at.iter().enumerate() {
                let o = outcome(&w, a, qid);
                assert_eq!(o.max, if qid == "q2" { 2.0 } else { 1.0 });
                assert!(o.correct == Some(o.points > 0.0), "{qid}/{i}: correct ⇔ points earned");
            }
        }
        // the exam itself carries the new key
        let stored: String = w.conn.query_row("SELECT questions FROM assessments WHERE id = ?1", params![w.exam], |r| r.get(0)).unwrap();
        let qs: Vec<Question> = serde_json::from_str(&stored).unwrap();
        assert_eq!(qs.iter().map(|q| q.answer.as_str()).collect::<Vec<_>>(), vec!["C", "AB", "B", "الجيزة", "نموذج"]);
        // a repeat is a no-op
        let again = fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!((again.affected_attempts, again.up, again.down, again.changed_outcomes, again.unchanged), (0, 0, 0, 0, 4));
    }

    #[test]
    fn new_points_rescale_what_each_student_earned_and_the_pass_flags_follow() {
        let w = world_with(false, json!({
            "questions": [
                {"id": "q1", "type": "single_choice", "stem": "أ", "options": ["x", "y"], "answer": "A", "score": 5},
                {"id": "q2", "type": "single_choice", "stem": "ب", "options": ["x", "y"], "answer": "A", "score": 5}
            ],
            "pass_mark": 50
        }));
        let at = vec![
            sit(&w, 0, &[("q1", "A"), ("q2", "B")], NOW + 1000),
            sit(&w, 1, &[("q1", "B"), ("q2", "A")], NOW + 2000),
            sit(&w, 2, &[("q1", "A"), ("q2", "A")], NOW + 3000),
        ];
        let passed = |a: &str| -> Value { serde_json::to_value(attempt_result(&w.conn, &w.teacher, a).unwrap()).unwrap()["passed"].clone() };
        assert_eq!((scores(&w, &at), total(&w)), (vec![5.0, 5.0, 10.0], 10.0));
        assert_eq!((passed(&at[0]), passed(&at[1]), passed(&at[2])), (json!(true), json!(true), json!(true)), "5 of 10 is exactly 50%");
        assert_eq!(results_summary(&w.conn, &w.teacher, &w.exam, NOW + 9_000).unwrap().passed_for_tests(), 3);
        // q1 is now worth 9: s0 has 9 of 14 (64%), s1 has 5 of 14 (36%) → fails, s2 14 of 14
        let r = fix(&w, &w.teacher, "q1", json!({"mode": "set", "score": 9})).unwrap();
        assert_eq!((r.old_total, r.new_total, r.up, r.down, r.affected_attempts), (10.0, 14.0, 2, 0, 2));
        assert_eq!(scores(&w, &at), vec![9.0, 5.0, 14.0]);
        assert_eq!((total(&w), outcome(&w, &at[0], "q1").max, outcome(&w, &at[0], "q1").points), (14.0, 9.0, 9.0));
        assert_eq!((passed(&at[0]), passed(&at[1]), passed(&at[2])), (json!(true), json!(false), json!(true)), "the pass mark is a percentage of the NEW total");
        assert_eq!(results_summary(&w.conn, &w.teacher, &w.exam, NOW + 9_000).unwrap().passed_for_tests(), 2);
        // and a smaller value shrinks it again; fractions are rounded to two places
        let r = fix(&w, &w.teacher, "q1", json!({"mode": "set", "score": 2.504})).unwrap();
        assert_eq!((r.new_total, r.question.score), (7.5, Some(2.5)));
        assert_eq!(scores(&w, &at), vec![2.5, 5.0, 7.5]);
    }

    impl crate::platform_exams::ResultsSummary {
        fn passed_for_tests(&self) -> i64 {
            serde_json::to_value(self).unwrap()["passed"].as_i64().unwrap()
        }
    }

    #[test]
    fn the_result_has_exactly_the_agreed_fields() {
        let w = world();
        class(&w);
        let v = serde_json::to_value(fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C", "dry_run": true})).unwrap()).unwrap();
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> = ["mode", "question", "old_total", "new_total", "attempts_total", "affected_attempts", "affected_students", "up", "down", "unchanged", "changed_outcomes", "dry_run"].into_iter().collect();
        assert_eq!(keys, expected);
        assert_eq!((v["mode"].as_str(), v["dry_run"].as_bool(), v["question"]["answer"].as_str(), v["question"]["id"].as_str(), v["question"]["type"].as_str()), (Some("set"), Some(true), Some("C"), Some("q1"), Some("single_choice")));
        assert_eq!((v["attempts_total"].as_i64(), v["affected_attempts"].as_i64(), v["up"].as_i64(), v["down"].as_i64(), v["unchanged"].as_i64()), (Some(4), Some(3), Some(2), Some(1), Some(1)));
    }

    #[test]
    fn only_the_explanation_can_change_without_touching_a_single_grade() {
        let w = world();
        let at = class(&w);
        let before = scores(&w, &at);
        let r = fix(&w, &w.teacher, "q1", json!({"mode": "set", "analysis": "  شرح جديد  "})).unwrap();
        assert_eq!((r.question.analysis.as_str(), r.affected_attempts, r.changed_outcomes), ("شرح جديد", 0, 0));
        assert_eq!(scores(&w, &at), before);
        let stored: Vec<Question> = serde_json::from_str(&w.conn.query_row::<String, _, _>("SELECT questions FROM assessments WHERE id = ?1", params![w.exam], |r| r.get(0)).unwrap()).unwrap();
        assert_eq!(stored[0].analysis, "شرح جديد");
    }

    // ───── manual grades ─────

    #[test]
    fn a_manual_grade_survives_every_correction_is_clamped_to_the_new_maximum_and_pending_stays_pending() {
        let w = world();
        let at = class(&w);
        // s0 and s2 wrote something for q5 (4 points); the teacher grades s0 with 3 and leaves s2 for later
        grade_attempt(&w.conn, &w.teacher, &at[0], &GradeReq { grades: [("q5".to_string(), 3.0)].into(), ..Default::default() }).unwrap();
        assert_eq!((scores(&w, &at), pendings(&w, &at)), (vec![8.0, 0.0, 3.0, 0.0], vec![0, 0, 1, 0]));
        // 1. an objective correction leaves the manual grade and the pending answer exactly as they were
        fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!((outcome(&w, &at[0], "q5").points, outcome(&w, &at[0], "q5").correct), (3.0, Some(false)));
        assert_eq!((outcome(&w, &at[2], "q5").points, outcome(&w, &at[2], "q5").correct), (0.0, None), "still waiting");
        assert_eq!(pendings(&w, &at), vec![0, 0, 1, 0]);
        // 2. changing the reference answer of the short question regrades nothing
        let r = fix(&w, &w.teacher, "q5", json!({"mode": "set", "answer": "مرجع جديد"})).unwrap();
        assert_eq!((r.affected_attempts, r.changed_outcomes), (0, 0));
        assert_eq!(outcome(&w, &at[0], "q5").points, 3.0);
        // 3. lowering its points clamps the grade (3 → 2) and, now that it is full marks, marks it right
        let r = fix(&w, &w.teacher, "q5", json!({"mode": "set", "score": 2})).unwrap();
        let o = outcome(&w, &at[0], "q5");
        assert_eq!((o.points, o.max, o.correct), (2.0, 2.0, Some(true)));
        assert_eq!(outcome(&w, &at[2], "q5").correct, None, "pending is still pending");
        assert_eq!((r.old_total, r.new_total, r.down, r.up), (9.0, 7.0, 1, 0), "only the graded student's total moved");
        assert_eq!(pendings(&w, &at), vec![0, 0, 1, 0]);
        // 4. raising it again keeps the 2 points the teacher gave (out of the new 6): the teacher regrades if they wish
        fix(&w, &w.teacher, "q5", json!({"mode": "set", "score": 6})).unwrap();
        let o = outcome(&w, &at[0], "q5");
        assert_eq!((o.points, o.max, o.correct), (2.0, 6.0, Some(false)));
        // the teacher can still grade the pending answer afterwards, within the new maximum
        let g = grade_attempt(&w.conn, &w.teacher, &at[2], &GradeReq { grades: [("q5".to_string(), 6.0)].into(), ..Default::default() }).unwrap();
        assert_eq!(serde_json::to_value(&g).unwrap()["pending"], json!(0));
        assert_eq!(grade_attempt(&w.conn, &w.teacher, &at[2], &GradeReq { grades: [("q5".to_string(), 7.0)].into(), ..Default::default() }).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn voiding_a_short_answer_settles_it_and_restoring_it_hands_the_hand_given_grades_back() {
        let w = world();
        let at = class(&w);
        grade_attempt(&w.conn, &w.teacher, &at[0], &GradeReq { grades: [("q5".to_string(), 3.0)].into(), ..Default::default() }).unwrap();
        assert_eq!((scores(&w, &at), pendings(&w, &at)), (vec![8.0, 0.0, 3.0, 0.0], vec![0, 0, 1, 0]));
        let r = fix(&w, &w.teacher, "q5", json!({"mode": "void"})).unwrap();
        assert_eq!((r.old_total, r.new_total, r.question.score), (9.0, 5.0, Some(0.0)));
        assert_eq!(pendings(&w, &at), vec![0, 0, 0, 0], "nothing left to grade");
        for a in &at {
            let o = outcome(&w, a, "q5");
            assert_eq!((o.max, o.points, o.correct), (0.0, 0.0, Some(true)), "settled for everybody, written or not");
        }
        assert_eq!(scores(&w, &at), vec![5.0, 0.0, 3.0, 0.0], "while the question is dropped the 3 points do not count");
        assert_eq!(outcome(&w, &at[0], "q5").kept, Some(3.0), "…but the grade the teacher gave is set aside, not thrown away");
        assert_eq!(outcome(&w, &at[2], "q5").kept, None, "an ungraded answer has nothing to keep");
        // restoring: the hand-given grade comes back, an answer that was never graded asks for grading again, blank ones are 0
        let back = fix(&w, &w.teacher, "q5", json!({"mode": "set", "score": 4, "dry_run": true})).unwrap();
        assert_eq!((back.affected_attempts, back.up, back.down), (1, 1, 0), "the preview shows the grade coming back");
        fix(&w, &w.teacher, "q5", json!({"mode": "set", "score": 4})).unwrap();
        assert_eq!((scores(&w, &at), pendings(&w, &at)), (vec![8.0, 0.0, 3.0, 0.0], vec![0, 0, 1, 0]), "exactly where it was before the void");
        let o = outcome(&w, &at[0], "q5");
        assert_eq!((o.max, o.points, o.correct, o.kept), (4.0, 3.0, Some(false), None));
        let o = outcome(&w, &at[3], "q5");
        assert_eq!((o.max, o.points, o.correct), (4.0, 0.0, Some(false)));
        assert_eq!(total(&w), 9.0);
    }

    #[test]
    fn a_hand_given_grade_survives_zero_points_voiding_twice_and_a_different_maximum_when_it_comes_back() {
        let w = world();
        let at = class(&w);
        grade_attempt(&w.conn, &w.teacher, &at[0], &GradeReq { grades: [("q5".to_string(), 3.0)].into(), ..Default::default() }).unwrap();
        grade_attempt(&w.conn, &w.teacher, &at[2], &GradeReq { grades: [("q5".to_string(), 1.0)].into(), ..Default::default() }).unwrap();
        assert_eq!(scores(&w, &at), vec![8.0, 0.0, 4.0, 0.0]);
        // setting the points to 0 is the same as voiding; doing it twice, or voiding afterwards, keeps what was set aside
        fix(&w, &w.teacher, "q5", json!({"mode": "set", "score": 0})).unwrap();
        fix(&w, &w.teacher, "q5", json!({"mode": "void"})).unwrap();
        fix(&w, &w.teacher, "q5", json!({"mode": "set", "analysis": "شرح"})).unwrap();
        assert_eq!((outcome(&w, &at[0], "q5").kept, outcome(&w, &at[2], "q5").kept), (Some(3.0), Some(1.0)));
        assert_eq!(scores(&w, &at), vec![5.0, 0.0, 3.0, 0.0]);
        // it comes back out of a smaller maximum: 3 is clamped to 2, 1 stays 1
        fix(&w, &w.teacher, "q5", json!({"mode": "set", "score": 2})).unwrap();
        assert_eq!(scores(&w, &at), vec![7.0, 0.0, 4.0, 0.0]);
        let (a, b) = (outcome(&w, &at[0], "q5"), outcome(&w, &at[2], "q5"));
        assert_eq!((a.points, a.max, a.correct, a.kept), (2.0, 2.0, Some(true), None));
        assert_eq!((b.points, b.max, b.correct, b.kept), (1.0, 2.0, Some(false), None));
        // the stored JSON carries `kept` only while a grade is set aside
        let raw: String = w.conn.query_row("SELECT results FROM attempts WHERE id = ?1", params![at[0]], |r| r.get(0)).unwrap();
        assert!(!raw.contains("kept"), "{raw}");
        fix(&w, &w.teacher, "q5", json!({"mode": "void"})).unwrap();
        let raw: String = w.conn.query_row("SELECT results FROM attempts WHERE id = ?1", params![at[0]], |r| r.get(0)).unwrap();
        assert!(raw.contains("\"kept\":2.0"), "{raw}");
        // outcomes written before `kept` existed still load
        let old: Outcome = serde_json::from_str(r#"{"id":"q1","correct":true,"points":1.0,"max":1.0}"#).unwrap();
        assert_eq!(old.kept, None);
    }

    // ───── void ─────

    #[test]
    fn voiding_drops_a_question_for_everybody_and_setting_points_again_brings_it_back() {
        let w = world();
        let at = class(&w);
        let r = fix(&w, &w.teacher, "q2", json!({"mode": "void"})).unwrap();
        assert_eq!((r.mode.as_str(), r.question.score, r.old_total, r.new_total), ("void", Some(0.0), 9.0, 7.0));
        assert_eq!((r.affected_attempts, r.down, r.up, r.unchanged), (2, 2, 0, 2), "s0 and s2 had it right, s1 wrong, s3 blank");
        assert_eq!(scores(&w, &at), vec![3.0, 0.0, 1.0, 0.0]);
        assert_eq!(total(&w), 7.0);
        for a in &at {
            let o = outcome(&w, a, "q2");
            assert_eq!((o.max, o.points, o.correct), (0.0, 0.0, Some(true)));
        }
        let listed = crate::platform_exam_admin::get_exam(&w.conn, &w.teacher, &w.exam, NOW).unwrap();
        assert_eq!((listed.row.info.question_count, listed.row.info.total_points, listed.questions.len()), (5, 7.0, 5), "the question stays listed");
        // an attempt that starts after the voiding is marked the same way: nothing to earn, nothing lost
        let late = sit(&w, 3, &[("q1", "B"), ("q2", "ABCD")], NOW + 9_000);
        assert_eq!(outcome(&w, &late, "q2").correct, Some(true));
        assert_eq!(scores(&w, &[late.clone()]), vec![1.0]);
        // voiding twice changes nothing further
        let twice = fix(&w, &w.teacher, "q2", json!({"mode": "void"})).unwrap();
        assert_eq!((twice.affected_attempts, twice.changed_outcomes), (0, 0));
        // points again: the answers each student gave decide, from what was stored
        let back = fix(&w, &w.teacher, "q2", json!({"mode": "set", "score": 2})).unwrap();
        assert_eq!((back.new_total, back.up, back.down), (9.0, 2, 0));
        assert_eq!(scores(&w, &at), vec![5.0, 0.0, 3.0, 0.0]);
        assert_eq!(scores(&w, &[late]), vec![1.0], "the late sitter had it wrong (ABCD)");
        // setting 0 points is the same as voiding
        let zero = fix(&w, &w.teacher, "q2", json!({"mode": "set", "score": 0})).unwrap();
        assert_eq!((zero.new_total, zero.question.score), (7.0, Some(0.0)));
    }

    // ───── not-submitted attempts ─────

    #[test]
    fn in_progress_and_expired_attempts_are_untouched_and_an_open_one_is_graded_with_the_new_key() {
        let w = world_with(false, json!({"duration_min": 10}));
        let done = sit(&w, 0, &[("q1", "B")], NOW + 1_000);
        let open = start_attempt_with(&w.conn, &w.students[1], &w.exam, NOW + 2_000, &mut StdRng::seed_from_u64(1)).unwrap();
        // an attempt that ran out of time without saving anything
        let gone = start_attempt_with(&w.conn, &w.students[2], &w.exam, NOW + 3_000, &mut StdRng::seed_from_u64(1)).unwrap();
        w.conn.execute("UPDATE attempts SET status = 'expired' WHERE id = ?1", params![gone.attempt_id]).unwrap();
        let (open_before, gone_before): (String, String) = (
            w.conn.query_row("SELECT status || answers || results || score FROM attempts WHERE id = ?1", params![open.attempt_id], |r| r.get(0)).unwrap(),
            w.conn.query_row("SELECT status || answers || results || score FROM attempts WHERE id = ?1", params![gone.attempt_id], |r| r.get(0)).unwrap(),
        );
        let r = fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!((r.attempts_total, r.affected_attempts, r.down), (1, 1, 1), "only the submitted attempt counts");
        assert_eq!(scores(&w, &[done]), vec![0.0]);
        let after = |id: &str| -> String { w.conn.query_row("SELECT status || answers || results || score FROM attempts WHERE id = ?1", params![id], |r| r.get(0)).unwrap() };
        assert_eq!((after(&open.attempt_id), after(&gone.attempt_id)), (open_before, gone_before), "not a byte changed");
        // the open attempt, submitted now, is graded with the corrected key
        let res = submit_attempt(&w.conn, &w.students[1], &open.attempt_id, &ans(&[("q1", "C")]), NOW + 5_000).unwrap();
        assert_eq!(res.score, 1.0);
    }

    // ───── shuffling ─────

    /// The displayed letter showing the option whose text is `text`.
    fn letter_of(q: &PublicQuestion, text: &str) -> char {
        let v = serde_json::to_value(q).unwrap();
        let i = v["options"].as_array().unwrap().iter().position(|o| o == text).unwrap();
        (b'A' + i as u8) as char
    }

    #[test]
    fn attempts_with_shuffled_options_are_regraded_by_their_original_letters() {
        let w = world_with(false, json!({"shuffle_options": true, "shuffle_questions": true}));
        let mut attempts = vec![];
        for (i, seed) in [11u64, 12, 13, 14].into_iter().enumerate() {
            let st: StartRes = start_attempt_with(&w.conn, &w.students[i], &w.exam, NOW + 1000 * (i as i64 + 1), &mut StdRng::seed_from_u64(seed)).unwrap();
            let q1 = st.questions.iter().find(|q| q.id == "q1").unwrap();
            // students 0,1 pick the option TEXT "o1-1" (the original B); 2 picks "o1-2" (C); 3 picks "o1-0" (A)
            let text = ["o1-1", "o1-1", "o1-2", "o1-0"][i];
            let shown = letter_of(q1, text).to_string();
            submit_attempt(&w.conn, &w.students[i], &st.attempt_id, &ans(&[("q1", &shown)]), NOW + 1000 * (i as i64 + 1) + 1).unwrap();
            attempts.push(st.attempt_id);
        }
        assert_eq!(scores(&w, &attempts), vec![1.0, 1.0, 0.0, 0.0], "marked by option text, whatever the screen showed");
        let stored: Vec<String> = attempts.iter().map(|a| w.conn.query_row("SELECT answers FROM attempts WHERE id = ?1", params![a], |r| r.get::<_, String>(0)).unwrap()).collect();
        assert!(stored[0].contains("\"q1\":\"B\"") && stored[2].contains("\"q1\":\"C\""), "stored in the original letters: {stored:?}");
        let r = fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!((r.up, r.down, r.affected_attempts), (1, 2, 3));
        assert_eq!(scores(&w, &attempts), vec![0.0, 0.0, 1.0, 0.0], "the one who picked the text of C is now right, whatever letter it was shown under");
    }

    // ───── dry run, atomicity ─────

    #[test]
    fn a_dry_run_writes_nothing_and_reports_the_numbers_the_real_run_will_produce() {
        for body in [json!({"mode": "set", "answer": "C"}), json!({"mode": "void"}), json!({"mode": "set", "score": 3}), json!({"mode": "set", "answer": "A", "score": 2, "analysis": "جديد"})] {
            let w = world();
            class(&w);
            let before = dump(&w.conn);
            let mut dry = body.clone();
            dry["dry_run"] = json!(true);
            let preview = fix(&w, &w.teacher, "q1", dry).unwrap();
            assert!(preview.dry_run);
            assert_eq!(dump(&w.conn), before, "{body}: a dry run changes no row of the exam, the attempts, the notifications or the audit log");
            let real = fix(&w, &w.teacher, "q1", body.clone()).unwrap();
            assert!(!real.dry_run);
            let key = |r: &CorrectResult| (r.mode.clone(), r.old_total, r.new_total, r.attempts_total, r.affected_attempts, r.affected_students, r.up, r.down, r.unchanged, r.changed_outcomes);
            assert_eq!(key(&preview), key(&real), "{body}: the preview is exactly what happens");
            assert_eq!(serde_json::to_value(&preview.question).unwrap(), serde_json::to_value(&real.question).unwrap());
            assert_ne!(dump(&w.conn), before, "{body}: the real run did write");
        }
    }

    #[test]
    fn a_failure_in_the_middle_rolls_everything_back() {
        let w = world();
        let at = class(&w);
        let before = dump(&w.conn);
        // the last attempt (by id order) refuses to be updated, after the earlier ones already were
        let last = {
            let mut ids = at[..3].to_vec(); // s0, s1, s2 change for q1 B → C; s3 (blank) does not
            ids.sort();
            ids.pop().unwrap()
        };
        w.conn.execute_batch(&format!("CREATE TRIGGER boom BEFORE UPDATE ON attempts WHEN NEW.id = '{last}' BEGIN SELECT RAISE(ABORT, 'boom'); END;")).unwrap();
        let e = fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap_err();
        assert_eq!(e.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(dump(&w.conn), before, "the exam, every attempt, the notifications and the audit log are exactly as they were");
        // the connection is usable (no transaction left open) and the same correction goes through once the fault is gone
        w.conn.execute_batch("DROP TRIGGER boom").unwrap();
        w.conn.execute("INSERT INTO audit_log(actor_id, target_id, action, detail, created_at) VALUES ('x','y','probe','',0)", []).unwrap();
        let ok = fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!(ok.affected_attempts, 3);
        assert_eq!(count(&w, "SELECT count(*) FROM audit_log WHERE action = 'exam_key_correct'"), 1);
    }

    // ───── notifications & audit ─────

    #[test]
    fn students_are_told_once_per_changed_attempt_only_when_their_result_is_out() {
        let w = world();
        let at = class(&w);
        let r = fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!(r.affected_attempts, 3);
        let notes: Vec<(String, String, String, String)> = w
            .conn
            .prepare("SELECT user_id, kind, data, link FROM notifications WHERE kind = 'score_changed' ORDER BY link")
            .unwrap()
            .query_map([], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(notes.len(), 3, "one per attempt whose score changed; s3's unchanged score tells nobody anything");
        assert!(notes.iter().all(|n| n.1 == "score_changed"));
        assert!(!notes.iter().any(|n| n.0 == w.students[3].id), "the unchanged student hears nothing");
        let first = notes.iter().find(|n| n.3 == format!("/platform/attempts/{}", at[0])).unwrap();
        assert_eq!(first.0, w.students[0].id);
        let d: Value = serde_json::from_str(&first.2).unwrap();
        assert_eq!(d, json!({"title": "امتحان المفتاح", "old": 5.0, "new": 4.0, "total": 9.0}));
        // a repeat changes no score, so tells nobody again
        fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!(count(&w, "SELECT count(*) FROM notifications WHERE kind = 'score_changed'"), 3);
    }

    #[test]
    fn a_withheld_result_is_corrected_silently_and_announced_by_the_release_as_before() {
        let w = world_with(false, json!({"release_mode": "after_close", "closes_at": NOW + 1_000_000}));
        let at = class(&w);
        assert_eq!(count(&w, "SELECT count(*) FROM notifications WHERE kind = 'score_changed'"), 0);
        // before the closing time: the results are embargoed, so the correction is stored but nobody is told
        let early = correct_question(&w.conn, &w.teacher, &w.exam, "q1", &req(json!({"mode": "set", "answer": "C"})), NOW + 10_000).unwrap();
        assert_eq!(early.affected_attempts, 3);
        assert_eq!(scores(&w, &at), vec![4.0, 1.0, 4.0, 0.0], "the grades are corrected");
        assert_eq!(count(&w, "SELECT count(*) FROM notifications WHERE kind = 'score_changed'"), 0, "…but a student who may not see the result yet is not told it changed");
        // after the closing time the results are out: now a change is announced
        let late = correct_question(&w.conn, &w.teacher, &w.exam, "q1", &req(json!({"mode": "set", "answer": "B"})), NOW + 2_000_000).unwrap();
        assert_eq!(late.affected_attempts, 3);
        assert_eq!(count(&w, "SELECT count(*) FROM notifications WHERE kind = 'score_changed'"), 3);
    }

    fn score_notes(w: &W) -> Vec<(String, Value, bool)> {
        w.conn
            .prepare("SELECT link, data, read_at IS NOT NULL FROM notifications WHERE kind = 'score_changed' ORDER BY link, created_at")
            .unwrap()
            .query_map([], |x| Ok((x.get::<_, String>(0)?, serde_json::from_str::<Value>(&x.get::<_, String>(1)?).unwrap(), x.get::<_, bool>(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn flipping_the_key_back_and_forth_leaves_one_notice_per_attempt_and_none_when_the_score_is_back() {
        let w = world();
        let at = class(&w);
        for key in ["C", "B", "C", "A", "C"] {
            fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": key})).unwrap();
        }
        // s0 5 → … → 4, s1 0 → 1 (the key ends on C), s2 3 → 4: exactly one notice each, remembering the score they first saw
        let notes = score_notes(&w);
        assert_eq!(notes.len(), 3, "{notes:?}");
        let of = |i: usize| notes.iter().find(|n| n.0 == format!("/platform/attempts/{}", at[i])).unwrap().1.clone();
        assert_eq!(of(0), json!({"title": "امتحان المفتاح", "old": 5.0, "new": 4.0, "total": 9.0}));
        assert_eq!(of(1), json!({"title": "امتحان المفتاح", "old": 0.0, "new": 1.0, "total": 9.0}));
        assert_eq!(of(2), json!({"title": "امتحان المفتاح", "old": 3.0, "new": 4.0, "total": 9.0}));
        // putting the key back where it started puts every score back: nothing is left to tell
        fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "B"})).unwrap();
        assert!(score_notes(&w).is_empty(), "scores are where the students last saw them: the notices are withdrawn");
        // a notice the student already read is history: the next change is a new notice with its own old value
        fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap();
        assert_eq!(score_notes(&w).len(), 3);
        w.conn.execute("UPDATE notifications SET read_at = 1 WHERE kind = 'score_changed'", []).unwrap();
        fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "A"})).unwrap(); // nobody picked A: s1 1 → 0, s2 4 → 3, s0 stays at 4
        let notes = score_notes(&w);
        assert_eq!(notes.len(), 5, "3 read ones + 2 new: {notes:?}");
        let unread: Vec<&(String, Value, bool)> = notes.iter().filter(|n| !n.2).collect();
        assert_eq!(unread.len(), 2);
        for (i, old, new) in [(1usize, 1.0, 0.0), (2, 4.0, 3.0)] {
            let n = unread.iter().find(|n| n.0.ends_with(&at[i])).unwrap();
            assert_eq!((n.1["old"].as_f64(), n.1["new"].as_f64()), (Some(old), Some(new)), "attempt {i}");
        }
        assert!(!unread.iter().any(|n| n.0.ends_with(&at[0])), "s0 stayed at 4: nothing new to tell");
        // each notice stays tied to its attempt and to the student who sat it
        for n in &notes {
            let attempt = n.0.rsplit('/').next().unwrap();
            let owner: String = w.conn.query_row("SELECT user_id FROM notifications WHERE link = ?1 AND kind = 'score_changed' LIMIT 1", params![n.0], |r| r.get(0)).unwrap();
            let sat_by: String = w.conn.query_row("SELECT student_id FROM attempts WHERE id = ?1", params![attempt], |r| r.get(0)).unwrap();
            assert_eq!(owner, sat_by);
        }
    }

    #[test]
    fn a_failing_notification_or_audit_write_rolls_the_whole_correction_back() {
        for table in ["notifications", "audit_log"] {
            let w = world();
            class(&w);
            let before = dump(&w.conn);
            w.conn.execute_batch(&format!("CREATE TRIGGER boom BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'boom'); END;")).unwrap();
            let e = fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap_err();
            assert_eq!(e.0, StatusCode::INTERNAL_SERVER_ERROR, "{table}");
            assert_eq!(dump(&w.conn), before, "{table}: the exam and every attempt are exactly as they were — no correction without its trail");
            w.conn.execute_batch("DROP TRIGGER boom").unwrap();
            assert_eq!(fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "C"})).unwrap().affected_attempts, 3, "{table}: and it goes through once the fault is gone");
        }
    }

    #[test]
    fn arabic_true_false_words_and_diacritics_are_regraded_like_at_submission() {
        let w = world();
        // the students wrote words rather than the A/B letters, and a vowelled spelling of a fill-blank answer
        let at = vec![
            sit(&w, 0, &[("q3", "صحيح"), ("q4", "مِصْر")], NOW + 1_000),
            sit(&w, 1, &[("q3", "خطأ"), ("q4", "الجيزة")], NOW + 2_000),
            sit(&w, 2, &[("q3", "نعم"), ("q4", "الْقَاهِرَةُ")], NOW + 3_000),
            sit(&w, 3, &[("q3", "لا"), ("q4", "مصر")], NOW + 4_000),
        ];
        assert_eq!(scores(&w, &at), vec![2.0, 0.0, 2.0, 1.0], "marked right at submission: 'صحيح' and 'نعم' are A, a vowelled 'مصر' matches");
        // true/false key A → B (given as a word): the A-sayers lose, the B-sayers win
        let r = fix(&w, &w.teacher, "q3", json!({"mode": "set", "answer": "خطأ"})).unwrap();
        assert_eq!(r.question.answer, "B");
        assert_eq!((scores(&w, &at), r.up, r.down), (vec![1.0, 1.0, 1.0, 2.0], 2, 2));
        // fill-blank key with vowels and a different alternative list: compared diacritic-blind on both sides
        let r = fix(&w, &w.teacher, "q4", json!({"mode": "set", "answer": "مَصْرُ | الجِيزَة"})).unwrap();
        assert_eq!((r.up, r.down), (1, 1), "'الجيزة' is now right too, the vowelled 'القاهرة' no longer is");
        assert_eq!(scores(&w, &at), vec![1.0, 2.0, 0.0, 2.0]);
    }

    #[test]
    fn a_real_correction_is_audited_with_who_what_and_how_many() {
        let w = world();
        class(&w);
        fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "c", "dry_run": true})).unwrap();
        assert_eq!(count(&w, "SELECT count(*) FROM audit_log WHERE action = 'exam_key_correct'"), 0, "a preview is not an action");
        fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "c", "score": 2})).unwrap();
        let (actor, target, detail): (String, String, String) =
            w.conn.query_row("SELECT actor_id, target_id, detail FROM audit_log WHERE action = 'exam_key_correct'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((actor, target), (w.teacher.id.clone(), w.exam.clone()));
        let d: Value = serde_json::from_str(&detail).unwrap();
        assert_eq!((d["question_id"].as_str(), d["mode"].as_str(), d["old_answer"].as_str(), d["new_answer"].as_str(), d["old_score"].as_f64(), d["new_score"].as_f64()), (Some("q1"), Some("set"), Some("B"), Some("C"), Some(1.0), Some(2.0)));
        assert_eq!((d["attempts_total"].as_i64(), d["affected_attempts"].as_i64(), d["up"].as_i64(), d["down"].as_i64()), (Some(4), Some(3), Some(2), Some(1)));
        // a void is recorded as one too
        fix(&w, &w.teacher, "q3", json!({"mode": "void"})).unwrap();
        assert_eq!(count(&w, "SELECT count(*) FROM audit_log WHERE action = 'exam_key_correct'"), 2);
    }

    // ───── permissions & validation ─────

    #[test]
    fn only_the_owner_of_a_live_unlocked_exam_corrects_its_key() {
        let w = world();
        class(&w);
        let before = dump(&w.conn);
        let body = json!({"mode": "set", "answer": "C"});
        // a colleague, an admin on a teacher's exam, a student: all forbidden, nothing changes
        for (who, label) in [(&w.other, "another teacher"), (&w.admin, "an admin on a teacher's exam"), (&w.students[0], "a student")] {
            assert_eq!(fail(fix(&w, who, "q1", body.clone())), (403, "forbidden".into()), "{label}");
            assert_eq!(fail(fix(&w, who, "q1", json!({"mode": "set", "answer": "C", "dry_run": true}))), (403, "forbidden".into()), "{label}: not even a preview");
        }
        assert_eq!(dump(&w.conn), before);
        // a missing exam / question
        assert_eq!(fail(correct_question(&w.conn, &w.teacher, "ghost", "q1", &req(body.clone()), NOW)), (404, "not_found".into()));
        assert_eq!(fail(fix(&w, &w.teacher, "ghost", body.clone())), (404, "not_found".into()));
        // an admin exam belongs to the admin: a teacher approved for the subject grades it but cannot touch its key
        let admin_exam = world_with(true, json!({}));
        class(&admin_exam);
        assert_eq!(fail(fix(&admin_exam, &admin_exam.teacher, "q1", body.clone())), (403, "forbidden".into()));
        assert_eq!(fail(fix(&admin_exam, &admin_exam.other, "q1", body.clone())), (403, "forbidden".into()));
        let ok = fix(&admin_exam, &admin_exam.admin, "q1", body.clone()).unwrap();
        assert_eq!(ok.affected_attempts, 3, "any admin owns an admin-created exam");
        // a closed exam can still be corrected (that is when mistakes are found) ...
        let closed = world();
        class(&closed);
        crate::platform_exam_admin::lifecycle(&closed.conn, &closed.teacher, &closed.exam, "close", NOW).unwrap();
        assert_eq!(fix(&closed, &closed.teacher, "q1", body.clone()).unwrap().affected_attempts, 3);
        // ... an archived one is read-only ...
        crate::platform_exam_admin::lifecycle(&closed.conn, &closed.teacher, &closed.exam, "archive", NOW).unwrap();
        assert_eq!(fail(fix(&closed, &closed.teacher, "q1", body.clone())), (409, "archived".into()));
        // ... and one an admin closed is locked for the owner
        let locked = world();
        class(&locked);
        act(&locked.conn, &locked.admin, &locked.exam, "close", NOW).unwrap();
        assert_eq!(fail(fix(&locked, &locked.teacher, "q1", body.clone())), (409, "locked".into()));
        assert_eq!(fail(fix(&locked, &locked.admin, "q1", body.clone())), (403, "forbidden".into()), "the admin moderates, the owner decides the key");
        act(&locked.conn, &locked.admin, &locked.exam, "reopen", NOW).unwrap();
        assert!(fix(&locked, &locked.teacher, "q1", body).is_ok(), "an admin reopening it gives the key back to its owner");
    }

    #[test]
    fn a_bad_body_is_refused_whole_and_the_answer_is_validated_like_when_the_exam_was_saved() {
        let w = world();
        class(&w);
        let before = dump(&w.conn);
        let cases: Vec<(&str, &str, Value, &str)> = vec![
            ("no mode", "q1", json!({"answer": "C"}), "invalid_mode"),
            ("unknown mode", "q1", json!({"mode": "accept_all"}), "invalid_mode"),
            ("set with nothing to set", "q1", json!({"mode": "set"}), "invalid_question"),
            ("negative points", "q1", json!({"mode": "set", "score": -1}), "invalid_points"),
            ("101 points", "q1", json!({"mode": "set", "score": 101}), "invalid_points"),
            ("choice letter outside the options", "q1", json!({"mode": "set", "answer": "D"}), "invalid_answer"),
            ("two letters for a single choice", "q1", json!({"mode": "set", "answer": "AB"}), "invalid_answer"),
            ("prose for a choice", "q1", json!({"mode": "set", "answer": "القاهرة"}), "invalid_answer"),
            ("multi choice letter outside", "q2", json!({"mode": "set", "answer": "AE"}), "invalid_answer"),
            ("true/false nobody can grade", "q3", json!({"mode": "set", "answer": "ربما"}), "invalid_answer"),
            ("blank fill-blank", "q4", json!({"mode": "set", "answer": "   "}), "invalid_question"),
            ("blank short answer", "q5", json!({"mode": "set", "answer": ""}), "invalid_question"),
            ("explanation too long", "q1", json!({"mode": "set", "analysis": "ش".repeat(6001)}), "invalid_question"),
        ];
        for (label, qid, body, code) in cases {
            for dry in [false, true] {
                let mut b = body.clone();
                b["dry_run"] = json!(dry);
                assert_eq!(fail(fix(&w, &w.teacher, qid, b)), (400, code.into()), "{label} (dry_run {dry})");
            }
        }
        // NaN / infinity cannot even be written in JSON, but a direct caller can pass them
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let r = CorrectReq { mode: Some("set".into()), score: Some(bad), ..Default::default() };
            assert_eq!(fail(correct_question(&w.conn, &w.teacher, &w.exam, "q1", &r, NOW)), (400, "invalid_points".into()), "{bad}");
        }
        assert_eq!(fail(fix(&w, &w.teacher, "q9", json!({"mode": "void"}))), (404, "not_found".into()), "unknown question");
        assert_eq!(dump(&w.conn), before, "every refused request left the data exactly as it was");
        // what IS accepted is brought to the canonical form
        let ok = |qid: &str, a: &str| fix(&w, &w.teacher, qid, json!({"mode": "set", "answer": a})).unwrap().question.answer;
        assert_eq!(ok("q1", "c"), "C", "lower case");
        assert_eq!(ok("q1", "B. الثاني"), "B", "a leading letter with its text");
        assert_eq!(ok("q2", "c, a"), "AC", "sorted letters");
        assert_eq!(ok("q3", "خطأ"), "B", "true/false words become A/B");
        assert_eq!(ok("q4", "  مصر | القاهرة "), "مصر | القاهرة", "fill-blank text is trimmed, alternatives kept");
        assert_eq!(ok("q5", " أي شيء "), "أي شيء");
    }

    #[test]
    fn voiding_works_even_when_the_stored_key_is_the_problem() {
        let w = world();
        let at = class(&w);
        // an old exam whose key points outside its options: no grader could ever mark it
        let mut qs = crate::platform_exams::load_questions(&w.conn, &w.exam).unwrap();
        qs[0].answer = "Z".into();
        w.conn.execute("UPDATE assessments SET questions = ?2 WHERE id = ?1", params![w.exam, serde_json::to_string(&qs).unwrap()]).unwrap();
        assert_eq!(fail(fix(&w, &w.teacher, "q1", json!({"mode": "set", "score": 3}))), (400, "invalid_answer".into()), "re-saving a broken key is refused: give a real one");
        let r = fix(&w, &w.teacher, "q1", json!({"mode": "void"})).unwrap();
        assert_eq!((r.new_total, r.question.score), (8.0, Some(0.0)));
        assert_eq!(scores(&w, &at).len(), 4);
        assert!(fix(&w, &w.teacher, "q1", json!({"mode": "set", "answer": "B", "score": 1})).is_ok(), "and a real key restores it");
    }

    #[test]
    fn a_class_of_voided_questions_leaves_a_valid_exam_with_nothing_to_divide() {
        let w = world();
        let at = class(&w);
        for q in ["q1", "q2", "q3", "q4", "q5"] {
            fix(&w, &w.teacher, q, json!({"mode": "void"})).unwrap();
        }
        assert_eq!((total(&w), scores(&w, &at), pendings(&w, &at)), (0.0, vec![0.0; 4], vec![0; 4]));
        for a in &at {
            let v = serde_json::to_value(attempt_result(&w.conn, &w.teacher, a).unwrap()).unwrap();
            assert_eq!((v["score"].as_f64(), v["total"].as_f64(), v["passed"].is_null(), v["pending"].as_i64()), (Some(0.0), Some(0.0), true, Some(0)));
        }
        let s = results_summary(&w.conn, &w.teacher, &w.exam, NOW + 9_000).unwrap();
        let v = serde_json::to_value(&s).unwrap();
        assert!(v["average"].as_f64().unwrap().is_finite() && v["highest"].as_f64().unwrap().is_finite());
    }
}
