//! Grading and results (phase 1-7): the pending-grading queue and quick grading sheet, per-exam
//! analytics, the students table and XLSX/CSV export.
//!
//! Who may use it: any admin; the owning teacher of a teacher's exam; for an admin-created exam any
//! active teacher approved for its subject (`platform_exams::can_grade`). Students never reach any of it.

use crate::platform::{audit, bad, db_err, like_pattern, lock, require_active, Res, User};
use crate::platform_exams::{can_grade, get_info, grade_attempt_at, load_questions, passed_flag, points_of, round2, settle_assessment, AssessmentInfo, GradeReq, Outcome};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use exameow_core::exam::{Question, QuestionType};
use exameow_core::export::{export_table_csv, export_tables_xlsx, Cell, Sheet};
use rusqlite::{params, params_from_iter, types::Value, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Questions answered by fewer graded attempts than this are not flagged weak/easy (too little data).
const MIN_SAMPLE: i64 = 5;
const WEAK_BELOW: f64 = 0.30;
const EASY_ABOVE: f64 = 0.90;
const MAX_BATCH: usize = 500;
const MAX_ATTEMPTS_LOADED: i64 = 20_000;

fn require_grader(conn: &Connection, user: &User, id: &str) -> Res<()> {
    if !can_grade(conn, user, id)? {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(())
}

// ───────── pending-grading queue ─────────

#[derive(Serialize, Debug, PartialEq)]
pub struct PendingExam {
    assessment_id: String,
    title: String,
    subject_id: String,
    subject_name: String,
    pending_attempts: i64,
    pending_answers: i64,
    oldest_submitted_at: Option<i64>,
    /// Submitted attempts of the exam, graded or not (the context of "N waiting").
    attempts_total: i64,
    /// The caller owns the exam by the codebase's one ownership rule (`AssessmentInfo::owned_by`): a teacher owns the
    /// exams whose `teacher_id` is theirs, an admin owns the exams created by an admin (no teacher). False for an admin's
    /// exam a teacher grades by subject approval, and for a teacher's exam an admin looks at. The queue tags those
    /// "not your exam" for teachers only (an admin sees teachers' exams as a matter of course).
    owned: bool,
}

/// Exams with manual grading still to do that this user may grade, oldest waiting first.
pub fn pending_exams(conn: &Connection, user: &User) -> Res<Vec<PendingExam>> {
    pending_rows(conn, user, None)
}

/// [`pending_exams`] restricted to one subject. A blank id means "no filter"; an id that matches nothing gives an empty
/// list (the id is only ever compared, never used as a pattern).
pub fn pending_exams_for_subject(conn: &Connection, user: &User, subject_id: &str) -> Res<Vec<PendingExam>> {
    pending_rows(conn, user, Some(subject_id).filter(|s| !s.trim().is_empty()))
}

fn pending_rows(conn: &Connection, user: &User, subject_id: Option<&str>) -> Res<Vec<PendingExam>> {
    if user.role != "admin" && user.role != "teacher" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    conn.prepare(
        "SELECT a.id, a.title, s.name_ar, count(*), sum(t.pending), min(t.submitted_at), a.subject_id,
                (SELECT count(*) FROM attempts x WHERE x.assessment_id = a.id AND x.status = 'submitted'),
                ((?1 = 'teacher' AND a.teacher_id IS NOT NULL AND a.teacher_id = ?2) OR (?1 = 'admin' AND a.teacher_id IS NULL))
         FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN subjects s ON s.id = a.subject_id
         WHERE t.status = 'submitted' AND t.pending > 0 AND (?3 IS NULL OR a.subject_id = ?3) AND (
               ?1 = 'admin' OR (?1 = 'teacher' AND (a.teacher_id = ?2 OR (a.teacher_id IS NULL AND EXISTS(
                 SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = ?2 AND ts.subject_id = a.subject_id AND ts.status = 'approved')))))
         GROUP BY a.id ORDER BY min(t.submitted_at), a.id LIMIT 200",
    )
    .map_err(db_err)?
    .query_map(params![user.role, user.id, subject_id], |r| {
        Ok(PendingExam {
            assessment_id: r.get(0)?,
            title: r.get(1)?,
            subject_name: r.get(2)?,
            pending_attempts: r.get(3)?,
            pending_answers: r.get(4)?,
            oldest_submitted_at: r.get(5)?,
            subject_id: r.get(6)?,
            attempts_total: r.get(7)?,
            owned: r.get(8)?,
        })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

// ───────── quick grading sheet ─────────

#[derive(Serialize, Debug)]
pub struct GradingAnswer {
    attempt_id: String,
    student_name: String,
    answer: String,
    /// `None` while the answer waits for its first grade.
    points: Option<f64>,
    submitted_at: Option<i64>,
    /// The comment already written on this answer, if any.
    feedback: Option<String>,
    /// When a grader last gave the points (first grade or a correction); `None` while ungraded.
    graded_at: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct GradingQuestion {
    id: String,
    stem: String,
    /// The reference (model) answer the grader compares against.
    reference: String,
    max: f64,
    pending: i64,
    answers: Vec<GradingAnswer>,
}

#[derive(Serialize, Debug)]
pub struct GradingSheet {
    assessment_id: String,
    title: String,
    subject_name: String,
    /// The exam shows its students the per-question view. When it does not, they never see the comments written here
    /// (and are not told about them); the sheet says so to the grader.
    show_answers: bool,
    questions: Vec<GradingQuestion>,
}

/// Every written short answer of an exam, grouped by question (blank answers are 0 automatically and not listed).
pub fn grading_sheet(conn: &Connection, user: &User, id: &str, only_pending: bool) -> Res<GradingSheet> {
    require_grader(conn, user, id)?;
    let info = get_info(conn, &user.id, id)?;
    let questions = load_questions(conn, id)?;
    // a voided question (0 points) has nothing left to grade
    let short: Vec<&Question> = questions.iter().filter(|q| q.qtype == QuestionType::ShortAnswer && points_of(q) > 0.0).collect();
    let mut out: Vec<GradingQuestion> = short
        .iter()
        .map(|q| GradingQuestion { id: q.id.clone(), stem: q.stem.clone(), reference: q.answer.clone(), max: points_of(q), pending: 0, answers: vec![] })
        .collect();
    let rows: Vec<(String, String, String, String, Option<i64>)> = conn
        .prepare(
            "SELECT t.id, u.full_name, t.answers, t.results, t.submitted_at FROM attempts t JOIN users u ON u.id = t.student_id
             WHERE t.assessment_id = ?1 AND t.status = 'submitted' ORDER BY t.submitted_at, t.id LIMIT 2000",
        )
        .map_err(db_err)?
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    for (attempt_id, name, answers_raw, results_raw, submitted_at) in rows {
        let answers: HashMap<String, String> = serde_json::from_str(&answers_raw).unwrap_or_default();
        let outcomes: Vec<Outcome> = serde_json::from_str(&results_raw).unwrap_or_default();
        for q in out.iter_mut() {
            let Some(text) = answers.get(&q.id).filter(|a| !a.trim().is_empty()) else { continue };
            let outcome = outcomes.iter().find(|o| o.id == q.id);
            let graded = outcome.filter(|o| o.correct.is_some()).map(|o| o.points);
            if graded.is_none() {
                q.pending += 1;
            }
            if only_pending && graded.is_some() {
                continue;
            }
            q.answers.push(GradingAnswer {
                attempt_id: attempt_id.clone(),
                student_name: name.clone(),
                answer: text.clone(),
                points: graded,
                submitted_at,
                feedback: outcome.and_then(|o| o.feedback.clone()),
                graded_at: outcome.filter(|o| o.correct.is_some()).and_then(|o| o.graded_at),
            });
        }
    }
    Ok(GradingSheet { assessment_id: id.to_string(), title: info.title, subject_name: info.subject_name, show_answers: info.show_answers, questions: out })
}

#[derive(Deserialize, Debug, Clone)]
pub struct BatchItem {
    attempt_id: String,
    question_id: String,
    /// New points. Optional so that a comment can be edited alone; an item needs `points` or `feedback` (or both).
    #[serde(default)]
    points: Option<f64>,
    /// New comment: absent leaves it, an empty text clears it.
    #[serde(default)]
    feedback: Option<String>,
}

#[derive(Deserialize)]
pub struct BatchReq {
    grades: Vec<BatchItem>,
}

#[derive(Serialize, Debug)]
pub struct BatchResult {
    updated: usize,
}

/// Grades many answers in one transaction: either every grade is stored or none is. Every attempt goes through
/// `platform_exams::grade_attempt` - the rules (what can be graded, the audit rows of corrections, the notices) live
/// there once, and its savepoint joins this transaction.
pub fn grade_batch(conn: &Connection, user: &User, id: &str, r: &BatchReq) -> Res<BatchResult> {
    grade_batch_at(conn, user, id, r, now_ms())
}

pub fn grade_batch_at(conn: &Connection, user: &User, id: &str, r: &BatchReq, now: i64) -> Res<BatchResult> {
    require_grader(conn, user, id)?;
    if r.grades.is_empty() || r.grades.len() > MAX_BATCH {
        return Err(bad("invalid_selection"));
    }
    // attempts are handled in id order, so which error is reported first does not depend on hashing
    let mut by_attempt: std::collections::BTreeMap<&str, GradeReq> = std::collections::BTreeMap::new();
    for g in &r.grades {
        if g.points.is_none() && g.feedback.is_none() {
            return Err(bad("invalid_selection")); // an item that changes nothing
        }
        let req = by_attempt.entry(g.attempt_id.as_str()).or_default();
        if req.grades.contains_key(&g.question_id) || req.feedback.contains_key(&g.question_id) {
            return Err(bad("invalid_selection")); // the same answer twice in one request
        }
        if let Some(p) = g.points {
            req.grades.insert(g.question_id.clone(), p);
        }
        if let Some(f) = &g.feedback {
            req.feedback.insert(g.question_id.clone(), f.clone());
        }
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
    let result = (|| -> Res<()> {
        for (attempt_id, req) in &by_attempt {
            let belongs: bool = conn
                .query_row("SELECT EXISTS(SELECT 1 FROM attempts WHERE id = ?1 AND assessment_id = ?2)", params![attempt_id, id], |x| x.get(0))
                .map_err(db_err)?;
            if !belongs {
                return Err(err(StatusCode::NOT_FOUND, "not_found"));
            }
            grade_attempt_at(conn, user, attempt_id, req, now)?;
        }
        audit(conn, &user.id, id, "grades_saved", &format!("{} answer(s)", r.grades.len()));
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT").map_err(db_err)?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    Ok(BatchResult { updated: r.grades.len() })
}

// ───────── analytics ─────────

#[derive(Serialize, Debug)]
pub struct Bin {
    /// Percent range `[from, to)`; the last bin includes 100.
    from: i64,
    to: i64,
    count: i64,
}

#[derive(Serialize, Debug)]
pub struct QStat {
    id: String,
    position: usize,
    #[serde(rename = "type")]
    qtype: String,
    stem: String,
    max: f64,
    /// Attempts whose answer to this question has a final grade.
    graded: i64,
    answered: i64,
    /// Share of the available points earned over graded attempts (0..1).
    rate: Option<f64>,
    avg_points: Option<f64>,
    weak: bool,
    easy: bool,
    /// Worth 0 points (dropped by an answer-key correction, or set to 0): it counts for nobody, so it has no rate and
    /// is never flagged weak / easy.
    voided: bool,
}

#[derive(Serialize, Debug)]
pub struct Analytics {
    info: AssessmentInfo,
    /// The caller may correct this exam's answer key right now (see `AssessmentInfo::can_correct_key`): the results
    /// page needs this one bit, and reads it from here instead of fetching every attempt.
    can_correct: bool,
    enrolled: i64,
    /// Distinct students with at least one submitted attempt.
    participants: i64,
    participation_pct: Option<f64>,
    attempts_submitted: i64,
    /// Students in the score statistics: their best fully-graded attempt counts.
    graded_students: i64,
    /// Students whose submitted attempts all still wait for manual grading.
    awaiting_grading: i64,
    average: Option<f64>,
    average_pct: Option<f64>,
    median: Option<f64>,
    highest: Option<f64>,
    lowest: Option<f64>,
    passed: i64,
    failed: i64,
    distribution: Vec<Bin>,
    avg_duration_sec: Option<f64>,
    tab_leave_students: i64,
    tab_leaves_total: i64,
    questions: Vec<QStat>,
}

struct Row {
    student: String,
    score: f64,
    pending: i64,
    started: i64,
    submitted: Option<i64>,
    leaves: i64,
    answers: HashMap<String, String>,
    outcomes: Vec<Outcome>,
}

pub fn analytics(conn: &Connection, user: &User, id: &str, now: i64) -> Res<Analytics> {
    require_grader(conn, user, id)?;
    let info = get_info(conn, &user.id, id)?;
    settle_assessment(conn, id, info.duration_min, now)?;
    let questions = load_questions(conn, id)?;
    let enrolled: i64 = conn.query_row("SELECT count(*) FROM subject_enrollments WHERE subject_id = ?1", params![info.subject_id], |r| r.get(0)).map_err(db_err)?;
    let rows: Vec<Row> = conn
        .prepare("SELECT student_id, score, pending, started_at, submitted_at, tab_leaves, answers, results FROM attempts WHERE assessment_id = ?1 AND status = 'submitted' ORDER BY submitted_at, id LIMIT ?2")
        .map_err(db_err)?
        .query_map(params![id, MAX_ATTEMPTS_LOADED], |r| {
            let (a, o): (String, String) = (r.get(6)?, r.get(7)?);
            Ok(Row { student: r.get(0)?, score: r.get(1)?, pending: r.get(2)?, started: r.get(3)?, submitted: r.get(4)?, leaves: r.get(5)?, answers: serde_json::from_str(&a).unwrap_or_default(), outcomes: serde_json::from_str(&o).unwrap_or_default() })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let total = info.total_points;
    // best fully-graded attempt per student (ties: the later one)
    let mut best: HashMap<&str, &Row> = HashMap::new();
    let mut students: HashMap<&str, bool> = HashMap::new(); // student -> has a graded attempt
    for r in &rows {
        let graded = r.pending == 0;
        *students.entry(r.student.as_str()).or_insert(false) |= graded;
        if graded && best.get(r.student.as_str()).map_or(true, |b| r.score >= b.score) {
            best.insert(r.student.as_str(), r);
        }
    }
    let mut scores: Vec<f64> = best.values().map(|r| r.score).collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = scores.len();
    let avg = (n > 0).then(|| round2(scores.iter().sum::<f64>() / n as f64));
    let median = (n > 0).then(|| if n % 2 == 1 { scores[n / 2] } else { round2((scores[n / 2 - 1] + scores[n / 2]) / 2.0) });
    let pct = |s: f64| if total > 0.0 { s / total * 100.0 } else { 0.0 };
    let mut distribution: Vec<Bin> = (0..5).map(|i| Bin { from: i * 20, to: (i + 1) * 20, count: 0 }).collect();
    for s in &scores {
        let b = ((pct(*s) + 1e-9) / 20.0).floor().clamp(0.0, 4.0) as usize;
        distribution[b].count += 1;
    }
    let (mut passed, mut failed) = (0, 0);
    if info.pass_mark.is_some() {
        for r in best.values() {
            match passed_flag(r.score, total, r.pending, info.pass_mark) {
                Some(true) => passed += 1,
                Some(false) => failed += 1,
                None => {}
            }
        }
    }
    let durations: Vec<f64> = rows.iter().filter_map(|r| r.submitted.map(|s| (s - r.started).max(0) as f64 / 1000.0)).collect();
    let mut qstats: Vec<QStat> = vec![];
    for (i, q) in questions.iter().enumerate() {
        let max = points_of(q);
        let (mut graded, mut answered, mut earned) = (0i64, 0i64, 0.0f64);
        for r in &rows {
            if r.answers.get(&q.id).map_or(false, |a| !a.trim().is_empty()) {
                answered += 1;
            }
            if let Some(o) = r.outcomes.iter().find(|o| o.id == q.id).filter(|o| o.correct.is_some()) {
                graded += 1;
                earned += o.points;
            }
        }
        let rate = (graded > 0 && max > 0.0).then(|| round2(earned / (graded as f64 * max)));
        qstats.push(QStat {
            id: q.id.clone(),
            position: i + 1,
            qtype: q.qtype.to_string(),
            stem: q.stem.chars().take(160).collect(),
            max,
            graded,
            answered,
            weak: rate.map_or(false, |x| graded >= MIN_SAMPLE && x < WEAK_BELOW),
            easy: rate.map_or(false, |x| graded >= MIN_SAMPLE && x > EASY_ABOVE),
            rate,
            avg_points: (graded > 0).then(|| round2(earned / graded as f64)),
            voided: max <= 0.0,
        });
    }
    let leave_students = rows.iter().filter(|r| r.leaves > 0).map(|r| r.student.as_str()).collect::<std::collections::HashSet<_>>().len() as i64;
    Ok(Analytics {
        can_correct: info.can_correct_key(user),
        participation_pct: (enrolled > 0).then(|| round2(students.len() as f64 / enrolled as f64 * 100.0)),
        participants: students.len() as i64,
        attempts_submitted: rows.len() as i64,
        graded_students: n as i64,
        awaiting_grading: students.values().filter(|g| !**g).count() as i64,
        average: avg,
        average_pct: avg.map(|a| round2(pct(a))),
        median,
        highest: scores.last().copied(),
        lowest: scores.first().copied(),
        passed,
        failed,
        distribution,
        avg_duration_sec: (!durations.is_empty()).then(|| round2(durations.iter().sum::<f64>() / durations.len() as f64)),
        tab_leave_students: leave_students,
        tab_leaves_total: rows.iter().map(|r| r.leaves).sum(),
        questions: qstats,
        enrolled,
        info,
    })
}

// ───────── students table ─────────

#[derive(Deserialize, Default)]
pub struct TableQuery {
    /// name | score | duration | tab_leaves | submitted_at | status
    sort: Option<String>,
    /// asc | desc
    dir: Option<String>,
    /// passed | failed | pending | expired | in_progress
    result: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    /// `attempts` (default: one row per attempt) | `students` (one row per student, see [`students_table`]).
    view: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct StudentRow {
    attempt_id: String,
    student_name: String,
    /// Only admins see e-mail addresses.
    email: Option<String>,
    attempt_no: i64,
    status: String,
    score: f64,
    total: f64,
    percent: Option<f64>,
    passed: Option<bool>,
    pending: i64,
    duration_sec: Option<i64>,
    tab_leaves: i64,
    started_at: i64,
    submitted_at: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct TablePage {
    items: Vec<StudentRow>,
    total: i64,
}

const PASSED_SQL: &str = "(t.status = 'submitted' AND t.pending = 0 AND a.pass_mark IS NOT NULL AND a.total_points > 0 AND t.score * 100.0 >= a.pass_mark * a.total_points - 1e-7)";
const FAILED_SQL: &str = "(t.status = 'submitted' AND t.pending = 0 AND a.pass_mark IS NOT NULL AND a.total_points > 0 AND t.score * 100.0 < a.pass_mark * a.total_points - 1e-7)";

pub fn attempts_table(conn: &Connection, user: &User, id: &str, f: &TableQuery, now: i64, unpaged: bool) -> Res<TablePage> {
    require_grader(conn, user, id)?;
    let info = get_info(conn, &user.id, id)?;
    settle_assessment(conn, id, info.duration_min, now)?;
    let mut args: Vec<Value> = vec![Value::Text(id.to_string())];
    let mut conds = vec!["t.assessment_id = ?1".to_string()];
    match f.result.as_deref().unwrap_or("") {
        "" | "all" => {}
        "passed" => conds.push(PASSED_SQL.into()),
        "failed" => conds.push(FAILED_SQL.into()),
        "pending" => conds.push("(t.status = 'submitted' AND t.pending > 0)".into()),
        "expired" => conds.push("t.status = 'expired'".into()),
        "in_progress" => conds.push("t.status = 'in_progress'".into()),
        _ => return Err(bad("invalid_filter")),
    }
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        args.push(Value::Text(format!("%{}%", q.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))));
        conds.push(format!("lower(u.full_name) LIKE ?{} ESCAPE '\\'", args.len()));
    }
    let wh = conds.join(" AND ");
    let order = match f.sort.as_deref().unwrap_or("submitted_at") {
        "name" => "u.full_name COLLATE NOCASE",
        "score" => "t.score",
        "duration" => "(t.submitted_at - t.started_at)",
        "tab_leaves" => "t.tab_leaves",
        "submitted_at" => "COALESCE(t.submitted_at, t.started_at)",
        "status" => "t.status",
        _ => return Err(bad("invalid_filter")),
    };
    let dir = match f.dir.as_deref().unwrap_or("desc") {
        "asc" => "ASC",
        "desc" => "DESC",
        _ => return Err(bad("invalid_filter")),
    };
    let total: i64 = conn
        .query_row(&format!("SELECT count(*) FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN users u ON u.id = t.student_id WHERE {wh}"), params_from_iter(args.iter()), |r| r.get(0))
        .map_err(db_err)?;
    let (limit, offset) = if unpaged { (10_000, 0) } else { (f.limit.unwrap_or(25).clamp(1, 100), f.offset.unwrap_or(0).clamp(0, 10_000_000)) };
    let show_email = user.role == "admin";
    let items = conn
        .prepare(&format!(
            "SELECT t.id, u.full_name, u.email, (SELECT count(*) FROM attempts x WHERE x.assessment_id = t.assessment_id AND x.student_id = t.student_id AND (x.started_at < t.started_at OR (x.started_at = t.started_at AND x.id <= t.id))),
                    t.status, t.score, a.total_points, {PASSED_SQL}, {FAILED_SQL}, t.pending, t.started_at, t.submitted_at, t.tab_leaves
             FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN users u ON u.id = t.student_id
             WHERE {wh} ORDER BY {order} {dir}, t.id LIMIT {limit} OFFSET {offset}"
        ))
        .map_err(db_err)?
        .query_map(params_from_iter(args.iter()), |r| {
            let (status, score, tot): (String, f64, f64) = (r.get(4)?, r.get(5)?, r.get(6)?);
            let (is_pass, is_fail): (bool, bool) = (r.get(7)?, r.get(8)?);
            let (started, submitted): (i64, Option<i64>) = (r.get(10)?, r.get(11)?);
            Ok(StudentRow {
                attempt_id: r.get(0)?,
                student_name: r.get(1)?,
                email: show_email.then(|| r.get::<_, String>(2).unwrap_or_default()),
                attempt_no: r.get(3)?,
                percent: (status == "submitted" && tot > 0.0).then(|| round2(score / tot * 100.0)),
                passed: if is_pass { Some(true) } else if is_fail { Some(false) } else { None },
                duration_sec: submitted.filter(|_| status == "submitted").map(|s| ((s - started) / 1000).max(0)),
                status,
                score,
                total: tot,
                pending: r.get(9)?,
                tab_leaves: r.get(12)?,
                started_at: started,
                submitted_at: submitted,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(TablePage { items, total })
}

// ───────── one row per student ─────────

#[derive(Serialize, Debug)]
pub struct StudentSummary {
    student_id: String,
    student_name: String,
    /// Only admins see e-mail addresses.
    email: Option<String>,
    /// Attempts of any status.
    attempts: i64,
    best_score: Option<f64>,
    best_percent: Option<f64>,
    best_attempt_id: Option<String>,
    last_attempt_id: String,
    last_status: String,
    last_submitted_at: Option<i64>,
    /// The best attempt's pass flag; `None` without a pass mark, while that attempt waits for grading, or without one.
    passed: Option<bool>,
    /// Written answers waiting for a grade, over all of the student's submitted attempts.
    pending: i64,
    /// Tab/focus losses, summed over all of the student's attempts.
    tab_leaves: i64,
}

#[derive(Serialize, Debug)]
pub struct StudentsPage {
    items: Vec<StudentSummary>,
    total: i64,
}

struct AttemptFacts {
    id: String,
    status: String,
    score: f64,
    pending: i64,
    started_at: i64,
    submitted_at: Option<i64>,
    tab_leaves: i64,
}

struct StudentAcc {
    name: String,
    email: String,
    attempts: Vec<AttemptFacts>,
}

/// The student's best attempt: among SUBMITTED attempts the highest score, a fully graded attempt always ahead of one
/// that still waits for grading (its partial score would be a guess - the same rule the analytics use); a tie goes to
/// the later one.
fn best_attempt(attempts: &[AttemptFacts]) -> Option<&AttemptFacts> {
    attempts.iter().filter(|a| a.status == "submitted").max_by(|a, b| {
        (a.pending == 0, a.score, a.submitted_at.unwrap_or(a.started_at), &a.id)
            .partial_cmp(&(b.pending == 0, b.score, b.submitted_at.unwrap_or(b.started_at), &b.id))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// "Per student" view of the results table: one row per student who has an attempt of any status. Same access rule as
/// the attempts table (`can_grade`); e-mail addresses only for admins.
///
/// * best attempt, `passed`: see [`best_attempt`];
/// * filter `result`: `passed` / `failed` (the best attempt's verdict) or `pending` (at least one written answer of any
///   submitted attempt still waits for a grade); anything else is a 400 `invalid_filter`, like an unknown `sort`
///   (`name` | `best` | `attempts` | `last`) or `dir`;
/// * `q` searches the name (case-insensitive; `%` and `_` are ordinary characters); `limit` 1..100 (default 25), `offset`.
pub fn students_table(conn: &Connection, user: &User, id: &str, f: &TableQuery, now: i64) -> Res<StudentsPage> {
    require_grader(conn, user, id)?;
    let result = f.result.as_deref().unwrap_or("");
    if !["", "all", "passed", "failed", "pending"].contains(&result) {
        return Err(bad("invalid_filter"));
    }
    let sort = f.sort.as_deref().unwrap_or("last");
    if !["name", "best", "attempts", "last"].contains(&sort) {
        return Err(bad("invalid_filter"));
    }
    let desc = match f.dir.as_deref().unwrap_or("desc") {
        "asc" => false,
        "desc" => true,
        _ => return Err(bad("invalid_filter")),
    };
    let info = get_info(conn, &user.id, id)?;
    settle_assessment(conn, id, info.duration_min, now)?;
    let rows: Vec<(String, String, String, AttemptFacts)> = conn
        .prepare(
            "SELECT t.student_id, u.full_name, u.email, t.id, t.status, t.score, t.pending, t.started_at, t.submitted_at, t.tab_leaves
             FROM attempts t JOIN users u ON u.id = t.student_id WHERE t.assessment_id = ?1 ORDER BY t.started_at, t.id LIMIT ?2",
        )
        .map_err(db_err)?
        .query_map(params![id, MAX_ATTEMPTS_LOADED], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                AttemptFacts { id: r.get(3)?, status: r.get(4)?, score: r.get(5)?, pending: r.get(6)?, started_at: r.get(7)?, submitted_at: r.get(8)?, tab_leaves: r.get(9)? },
            ))
        })
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut by_student: HashMap<String, StudentAcc> = HashMap::new();
    for (student, name, email, a) in rows {
        by_student.entry(student).or_insert_with(|| StudentAcc { name, email, attempts: vec![] }).attempts.push(a);
    }
    let needle = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase);
    let show_email = user.role == "admin";
    let total_points = info.total_points;
    // each row travels with the moment of its latest attempt (handed in, or started) - what `sort=last` orders by
    let mut items: Vec<(StudentSummary, i64)> = by_student
        .into_iter()
        .filter(|(_, acc)| needle.as_ref().map_or(true, |n| acc.name.to_lowercase().contains(n.as_str())))
        .map(|(student_id, acc)| {
            let best = best_attempt(&acc.attempts);
            // `attempts` is ordered by start time, so the last element is the latest attempt
            let last = acc.attempts.last().expect("a student row has at least one attempt");
            let summary = StudentSummary {
                student_name: acc.name.clone(),
                email: show_email.then(|| acc.email.clone()),
                attempts: acc.attempts.len() as i64,
                best_score: best.map(|b| b.score),
                best_percent: best.filter(|_| total_points > 0.0).map(|b| round2(b.score / total_points * 100.0)),
                best_attempt_id: best.map(|b| b.id.clone()),
                last_attempt_id: last.id.clone(),
                last_status: last.status.clone(),
                last_submitted_at: last.submitted_at,
                passed: best.and_then(|b| passed_flag(b.score, total_points, b.pending, info.pass_mark)),
                pending: acc.attempts.iter().filter(|a| a.status == "submitted").map(|a| a.pending).sum(),
                tab_leaves: acc.attempts.iter().map(|a| a.tab_leaves).sum(),
                student_id,
            };
            (summary, last.submitted_at.unwrap_or(last.started_at))
        })
        .filter(|(s, _)| match result {
            "passed" => s.passed == Some(true),
            "failed" => s.passed == Some(false),
            "pending" => s.pending > 0,
            _ => true,
        })
        .collect();
    items.sort_by(|(a, a_last), (b, b_last)| {
        use std::cmp::Ordering::*;
        let ord = match sort {
            "name" => a.student_name.to_lowercase().cmp(&b.student_name.to_lowercase()),
            "attempts" => a.attempts.cmp(&b.attempts),
            "last" => a_last.cmp(b_last),
            // "best": a student without a submitted attempt is always last, whichever way the list runs
            _ => match (a.best_score, b.best_score) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Equal),
                (Some(_), None) => return Less,
                (None, Some(_)) => return Greater,
                (None, None) => Equal,
            },
        };
        let ord = if desc { ord.reverse() } else { ord };
        ord.then_with(|| a.student_name.to_lowercase().cmp(&b.student_name.to_lowercase())).then_with(|| a.student_id.cmp(&b.student_id))
    });
    let total = items.len() as i64;
    let limit = f.limit.unwrap_or(25).clamp(1, 100) as usize;
    let offset = f.offset.unwrap_or(0).clamp(0, 10_000_000) as usize;
    Ok(StudentsPage { items: items.into_iter().map(|(s, _)| s).skip(offset).take(limit).collect(), total })
}

// ───────── who has not taken it ─────────

#[derive(Deserialize, Default, Debug)]
pub struct AbsentQuery {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct AbsentStudent {
    student_id: String,
    student_name: String,
}

#[derive(Serialize, Debug)]
pub struct AbsentPage {
    items: Vec<AbsentStudent>,
    /// Absent students matching the search.
    total: i64,
    /// Active students enrolled in the exam's subject (the whole class, whatever the search).
    enrolled: i64,
}

/// Active students enrolled in the exam's subject who have no attempt of ANY status on it (an unfinished or expired
/// attempt counts as having come), by name. **Names only** - never an e-mail address, not even for an admin. Same access
/// rule as the other results (`can_grade`), **plus** - for a teacher - a live approved assignment for the subject: this
/// is the subject's class list as it is today (students who enrolled long after the results were written, and may
/// never have sat the exam), not results the teacher already owns, so a teacher whose assignment was taken away keeps
/// their old results but not the roster. It works for an exam whose subject or institution is switched off too (the
/// approval is all that is asked), since the grader still owns the results.
pub fn absent_students(conn: &Connection, user: &User, id: &str, f: &AbsentQuery) -> Res<AbsentPage> {
    require_grader(conn, user, id)?;
    let info = get_info(conn, &user.id, id)?;
    if user.role != "admin" && !crate::platform_content::is_approved_for(conn, &user.id, &info.subject_id)? {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let class = "FROM subject_enrollments e JOIN users u ON u.id = e.student_id AND u.role = 'student' AND u.status = 'active' WHERE e.subject_id = ?1";
    let enrolled: i64 = conn.query_row(&format!("SELECT count(*) {class}"), params![info.subject_id], |r| r.get(0)).map_err(db_err)?;
    let mut args: Vec<Value> = vec![Value::Text(info.subject_id.clone()), Value::Text(id.to_string())];
    let mut conds = "AND NOT EXISTS(SELECT 1 FROM attempts t WHERE t.assessment_id = ?2 AND t.student_id = u.id)".to_string();
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        args.push(Value::Text(like_pattern(q)));
        conds.push_str(&format!(" AND lower(u.full_name) LIKE ?{} ESCAPE '\\'", args.len()));
    }
    let total: i64 = conn.query_row(&format!("SELECT count(*) {class} {conds}"), params_from_iter(args.iter()), |r| r.get(0)).map_err(db_err)?;
    let limit = f.limit.unwrap_or(25).clamp(1, 100);
    let offset = f.offset.unwrap_or(0).clamp(0, 10_000_000);
    let items = conn
        .prepare(&format!("SELECT u.id, u.full_name {class} {conds} ORDER BY u.full_name COLLATE NOCASE, u.id LIMIT {limit} OFFSET {offset}"))
        .map_err(db_err)?
        .query_map(params_from_iter(args.iter()), |r| Ok(AbsentStudent { student_id: r.get(0)?, student_name: r.get(1)? }))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(AbsentPage { items, total, enrolled })
}

// ───────── export ─────────

/// `2026-10-08 14:30` (UTC) from epoch milliseconds, without a date library.
fn fmt_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

struct Labels {
    results: &'static str,
    questions: &'static str,
    results_header: [&'static str; 11],
    questions_header: [&'static str; 8],
    passed: &'static str,
    failed: &'static str,
    pending: &'static str,
    expired: &'static str,
    in_progress: &'static str,
    weak: &'static str,
    easy: &'static str,
    voided: &'static str,
}

fn labels(lang: &str) -> Labels {
    if lang == "ar" {
        Labels {
            results: "النتائج",
            questions: "الأسئلة",
            results_header: ["الطالب", "البريد", "المحاولة", "الدرجة", "من", "النسبة %", "النتيجة", "الحالة", "المدة (دقيقة)", "مغادرات التبويب", "وقت التسليم (UTC)"],
            questions_header: ["#", "النوع", "السؤال", "الدرجة", "عدد المصحَّح", "عدد المجيبين", "نسبة الإجابة الصحيحة %", "ملاحظة"],
            passed: "ناجح",
            failed: "راسب",
            pending: "بانتظار التصحيح",
            expired: "منتهية",
            in_progress: "جارية",
            weak: "ضعيف",
            easy: "سهل",
            voided: "ملغى",
        }
    } else {
        Labels {
            results: "Results",
            questions: "Questions",
            results_header: ["Student", "Email", "Attempt", "Score", "Out of", "Percent %", "Result", "Status", "Duration (min)", "Tab leaves", "Submitted (UTC)"],
            questions_header: ["#", "Type", "Question", "Max points", "Graded", "Answered", "Correct rate %", "Flag"],
            passed: "Passed",
            failed: "Failed",
            pending: "Awaiting grading",
            expired: "Expired",
            in_progress: "In progress",
            weak: "Weak",
            easy: "Easy",
            voided: "Voided",
        }
    }
}

#[derive(Debug)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub filename: String,
}

fn results_rows(rows: &[StudentRow], l: &Labels, with_email: bool) -> Vec<Vec<Cell>> {
    let mut out: Vec<Vec<Cell>> = vec![l.results_header.iter().enumerate().filter(|(i, _)| with_email || *i != 1).map(|(_, h)| Cell::from(*h)).collect()];
    for r in rows {
        let verdict = match (&r.status[..], r.passed, r.pending) {
            ("expired", _, _) => l.expired,
            ("in_progress", _, _) => l.in_progress,
            (_, _, p) if p > 0 => l.pending,
            (_, Some(true), _) => l.passed,
            (_, Some(false), _) => l.failed,
            _ => "",
        };
        let mut row: Vec<Cell> = vec![r.student_name.clone().into()];
        if with_email {
            row.push(r.email.clone().unwrap_or_default().into());
        }
        row.extend([
            Cell::Num(r.attempt_no as f64),
            Cell::Num(r.score),
            Cell::Num(r.total),
            r.percent.map_or(Cell::Text(String::new()), Cell::Num),
            verdict.into(),
            r.status.clone().into(),
            r.duration_sec.map_or(Cell::Text(String::new()), |s| Cell::Num(round2(s as f64 / 60.0))),
            Cell::Num(r.tab_leaves as f64),
            r.submitted_at.map(fmt_utc).unwrap_or_default().into(),
        ]);
        out.push(row);
    }
    out
}

fn question_rows(a: &Analytics, l: &Labels) -> Vec<Vec<Cell>> {
    let mut out: Vec<Vec<Cell>> = vec![l.questions_header.iter().map(|h| Cell::from(*h)).collect()];
    for q in &a.questions {
        out.push(vec![
            Cell::Num(q.position as f64),
            q.qtype.clone().into(),
            q.stem.clone().into(),
            Cell::Num(q.max),
            Cell::Num(q.graded as f64),
            Cell::Num(q.answered as f64),
            q.rate.map_or(Cell::Text(String::new()), |r| Cell::Num(round2(r * 100.0))),
            if q.voided { l.voided } else if q.weak { l.weak } else if q.easy { l.easy } else { "" }.into(),
        ]);
    }
    out
}

/// `format`: xlsx (results + questions sheets) or csv; `part` (csv only): results | questions.
pub fn export(conn: &Connection, user: &User, id: &str, format: &str, part: &str, lang: &str, now: i64) -> Res<Exported> {
    let l = labels(lang);
    let table = attempts_table(conn, user, id, &TableQuery { sort: Some("name".into()), dir: Some("asc".into()), ..Default::default() }, now, true)?;
    let with_email = user.role == "admin";
    let short = id.chars().take(8).collect::<String>();
    audit(conn, &user.id, id, "results_exported", format);
    match format {
        "xlsx" => {
            let a = analytics(conn, user, id, now)?;
            let rtl = lang == "ar";
            let bytes = export_tables_xlsx(&[
                Sheet { name: l.results.into(), rows: results_rows(&table.items, &l, with_email), rtl },
                Sheet { name: l.questions.into(), rows: question_rows(&a, &l), rtl },
            ])
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "export_failed"))?;
            Ok(Exported { bytes, mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", filename: format!("exam-{short}-results.xlsx") })
        }
        "csv" => {
            let rows = match part {
                "questions" => question_rows(&analytics(conn, user, id, now)?, &l),
                "results" | "" => results_rows(&table.items, &l, with_email),
                _ => return Err(bad("invalid_filter")),
            };
            let bytes = export_table_csv(&rows).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "export_failed"))?;
            let tag = if part == "questions" { "questions" } else { "results" };
            Ok(Exported { bytes, mime: "text/csv; charset=utf-8", filename: format!("exam-{short}-{tag}.csv") })
        }
        _ => Err(bad("invalid_filter")),
    }
}

// ───────── handlers ─────────

#[derive(Deserialize, Default)]
pub struct PendingQuery {
    subject_id: Option<String>,
}

pub async fn pending_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<PendingQuery>) -> Res<Json<Vec<PendingExam>>> {
    let u = require_active(&s, &h)?;
    let conn = lock(&s)?;
    match q.subject_id.as_deref() {
        Some(subject) => pending_exams_for_subject(&conn, &u, subject),
        None => pending_exams(&conn, &u),
    }
    .map(Json)
}

#[derive(Deserialize)]
pub struct SheetQuery {
    pending: Option<bool>,
}

pub async fn sheet_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Query(q): Query<SheetQuery>) -> Res<Json<GradingSheet>> {
    let u = require_active(&s, &h)?;
    grading_sheet(&*lock(&s)?, &u, &id, q.pending.unwrap_or(false)).map(Json)
}

pub async fn batch_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<BatchReq>) -> Res<Json<BatchResult>> {
    let u = require_active(&s, &h)?;
    grade_batch(&*lock(&s)?, &u, &id, &r).map(Json)
}

pub async fn analytics_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<Analytics>> {
    let u = require_active(&s, &h)?;
    analytics(&*lock(&s)?, &u, &id, now_ms()).map(Json)
}

/// The results table in the view the request asks for: `attempts` (default, one row per attempt) or `students` (one
/// row per student). The access check comes first, so a non-grader learns nothing - not even which views exist.
pub fn results_view(conn: &Connection, user: &User, id: &str, q: &TableQuery, now: i64) -> Res<serde_json::Value> {
    require_grader(conn, user, id)?;
    let page = match q.view.as_deref().unwrap_or("attempts") {
        "attempts" | "" => serde_json::to_value(attempts_table(conn, user, id, q, now, false)?),
        "students" => serde_json::to_value(students_table(conn, user, id, q, now)?),
        _ => return Err(bad("invalid_filter")),
    };
    page.map_err(db_err)
}

pub async fn table_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Query(q): Query<TableQuery>) -> Res<Json<serde_json::Value>> {
    let u = require_active(&s, &h)?;
    results_view(&*lock(&s)?, &u, &id, &q, now_ms()).map(Json)
}

pub async fn absent_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Query(q): Query<AbsentQuery>) -> Res<Json<AbsentPage>> {
    let u = require_active(&s, &h)?;
    absent_students(&*lock(&s)?, &u, &id, &q).map(Json)
}

#[derive(Deserialize)]
pub struct ExportQuery {
    format: Option<String>,
    part: Option<String>,
    lang: Option<String>,
}

pub async fn export_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Query(q): Query<ExportQuery>) -> Result<Response, crate::relay::Err> {
    let u = require_active(&s, &h)?;
    // a heavy-ish operation: a few per minute is plenty
    crate::platform::rate_limit(&*lock(&s)?, &format!("export:{}", u.id), 60_000, 10)?;
    let e = export(&*lock(&s)?, &u, &id, q.format.as_deref().unwrap_or("xlsx"), q.part.as_deref().unwrap_or("results"), q.lang.as_deref().unwrap_or("ar"), now_ms())?;
    Ok((
        [
            (header::CONTENT_TYPE, e.mime.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", e.filename)),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        e.bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user, new_id};
    use crate::platform_exams::{start_attempt, submit_attempt};
    use serde_json::json;

    const NOW: i64 = 1_000_000_000_000;

    struct W {
        conn: Connection,
        admin: User,
        students: Vec<User>,
        exam: String,
    }

    fn ans(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// 5 objective questions (1 point, correct answer B) + 1 short answer (5 points) = 10 points, pass mark 50%.
    fn exam_json(extra: serde_json::Value) -> serde_json::Value {
        let mut qs: Vec<serde_json::Value> = (1..=5).map(|i| json!({"id": format!("q{i}"), "type": "single_choice", "stem": format!("س{i}"), "options": ["a", "b", "c"], "answer": "B", "score": 1})).collect();
        qs.push(json!({"id": "s1", "type": "short_answer", "stem": "اشرح", "answer": "مرجع", "score": 5}));
        let mut v = json!({"subject_id": "s1", "title": "امتحان التحليل", "questions": qs, "status": "published", "pass_mark": 50, "max_attempts": 2});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    fn world(n_students: usize) -> W {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s2','i1','أخرى',0)", []).unwrap();
        let students: Vec<User> = (0..n_students).map(|i| insert_test_user(&conn, &format!("st{i}@x.com"), "student", "active")).collect();
        for s in &students {
            conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![s.id]).unwrap();
        }
        let req: crate::platform_exam_admin::ExamReq = serde_json::from_value(exam_json(json!({}))).unwrap();
        let exam = crate::platform_exam_admin::create_exam(&conn, &admin, &req, NOW).unwrap();
        let id = serde_json::to_value(&exam).unwrap()["id"].as_str().unwrap().to_string();
        W { conn, admin, students, exam: id }
    }

    fn approve(w: &W, email: &str, subject: &str) -> User {
        let t = insert_test_user(&w.conn, email, "teacher", "active");
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,?2,'approved',0)", params![t.id, subject]).unwrap();
        t
    }

    /// Starts and submits one attempt: `right` = ids answered correctly (B), others wrong (A); `short` = written answer.
    fn attempt(w: &W, who: usize, right: &[&str], short: Option<&str>, minutes: i64) -> String {
        let u = &w.students[who];
        // distinct start times (minutes are unique per attempt) keep attempt numbering deterministic
        let started = NOW + minutes;
        let s = start_attempt(&w.conn, u, &w.exam, started).unwrap();
        let mut a: Vec<(String, String)> = (1..=5).map(|i| (format!("q{i}"), if right.contains(&format!("q{i}").as_str()) { "B" } else { "A" }.to_string())).collect();
        if let Some(t) = short {
            a.push(("s1".into(), t.into()));
        }
        let map: HashMap<String, String> = a.into_iter().collect();
        submit_attempt(&w.conn, u, &s.attempt_id, &map, started + minutes * 60_000).unwrap();
        s.attempt_id
    }

    fn grade(w: &W, attempt: &str, pts: f64) {
        crate::platform_exams::grade_attempt(&w.conn, &w.admin, attempt, &GradeReq { grades: [("s1".to_string(), pts)].into(), ..Default::default() }).unwrap();
    }

    /// The analytics scenario from the design notes: scores 10, 5, 2, 1, best-of(3, 9), plus one awaiting grading.
    fn scenario() -> W {
        let w = world(8);
        let a = attempt(&w, 0, &["q1", "q2", "q3", "q4", "q5"], Some("إجابة كاملة"), 10);
        grade(&w, &a, 5.0);
        let b = attempt(&w, 1, &["q1", "q2", "q3", "q4"], Some("إجابة جزئية"), 20);
        grade(&w, &b, 1.0);
        attempt(&w, 2, &["q1", "q2"], None, 30);
        attempt(&w, 3, &["q1"], None, 40);
        let e1 = attempt(&w, 4, &["q1", "q2", "q3"], Some("ضعيفة"), 50);
        grade(&w, &e1, 0.0);
        let e2 = attempt(&w, 4, &["q1", "q2", "q3", "q4"], Some("جيدة"), 60);
        grade(&w, &e2, 5.0);
        attempt(&w, 5, &["q1", "q2", "q3", "q4"], Some("لم تُصحَّح"), 70);
        w
    }

    #[test]
    fn who_may_grade_follows_ownership_and_approval() {
        let w = world(1);
        let approved = approve(&w, "t1@x.com", "s1");
        let other_subject = approve(&w, "t2@x.com", "s2");
        let unapproved = insert_test_user(&w.conn, "t3@x.com", "teacher", "active");
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s1','pending',0)", params![unapproved.id]).unwrap();
        assert!(can_grade(&w.conn, &w.admin, &w.exam).unwrap(), "admins grade everything");
        assert!(can_grade(&w.conn, &approved, &w.exam).unwrap(), "an approved teacher of the subject grades an admin exam (G2)");
        assert!(!can_grade(&w.conn, &other_subject, &w.exam).unwrap(), "a teacher of another subject cannot");
        assert!(!can_grade(&w.conn, &unapproved, &w.exam).unwrap(), "a pending assignment is not enough");
        assert!(!can_grade(&w.conn, &w.students[0], &w.exam).unwrap(), "students never grade");
        // a teacher's own exam belongs to that teacher alone (other approved teachers of the subject do not get in)
        let treq: crate::platform_exams::AssessmentReq = serde_json::from_value(json!({"subject_id": "s1", "title": "امتحاني", "questions": exam_json(json!({}))["questions"], "status": "published"})).unwrap();
        let tid = crate::platform_exams::create_assessment(&w.conn, &approved, &treq).unwrap().id();
        let colleague = approve(&w, "t4@x.com", "s1");
        assert!(can_grade(&w.conn, &approved, &tid).unwrap() && can_grade(&w.conn, &w.admin, &tid).unwrap());
        assert!(!can_grade(&w.conn, &colleague, &tid).unwrap(), "a colleague cannot grade someone else's exam");
        for f in [grading_sheet(&w.conn, &colleague, &tid, false).map(|_| ()), analytics(&w.conn, &colleague, &tid, NOW).map(|_| ()), attempts_table(&w.conn, &colleague, &tid, &TableQuery::default(), NOW, false).map(|_| ()), export(&w.conn, &colleague, &tid, "csv", "results", "en", NOW).map(|_| ())] {
            assert_eq!(f.unwrap_err().0, StatusCode::FORBIDDEN);
        }
        assert_eq!(grading_sheet(&w.conn, &w.students[0], &w.exam, false).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(pending_exams(&w.conn, &w.students[0]).unwrap_err().0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn the_pending_queue_groups_by_exam_and_shows_each_grader_only_their_own() {
        let w = world(4);
        let approved = approve(&w, "t1@x.com", "s1");
        let stranger = approve(&w, "t2@x.com", "s2");
        attempt(&w, 0, &["q1"], Some("أ"), 5);
        attempt(&w, 1, &["q1"], Some("ب"), 3);
        attempt(&w, 2, &["q1"], None, 4); // blank short answer → nothing to grade
        let q = pending_exams(&w.conn, &w.admin).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].pending_attempts, q[0].pending_answers, q[0].oldest_submitted_at), (2, 2, Some(NOW + 3 + 3 * 60_000)));
        assert_eq!(pending_exams(&w.conn, &approved).unwrap().len(), 1, "an approved teacher sees the admin exam's queue");
        assert!(pending_exams(&w.conn, &stranger).unwrap().is_empty(), "a teacher of another subject sees nothing");
        // grading the second student empties that student's part; grading both clears the queue
        let sheet = grading_sheet(&w.conn, &w.admin, &w.exam, true).unwrap();
        for a in &sheet.questions[0].answers {
            grade_batch(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![BatchItem { attempt_id: a.attempt_id.clone(), question_id: "s1".into(), points: Some(2.0), feedback: None }] }).unwrap();
        }
        assert!(pending_exams(&w.conn, &w.admin).unwrap().is_empty());
    }

    #[test]
    fn the_grading_sheet_lists_written_short_answers_with_their_state() {
        let w = world(3);
        let a = attempt(&w, 0, &[], Some("أولى"), 1);
        attempt(&w, 1, &[], Some("ثانية"), 2);
        attempt(&w, 2, &[], Some("   "), 3); // whitespace only counts as blank
        grade(&w, &a, 4.0);
        let all = grading_sheet(&w.conn, &w.admin, &w.exam, false).unwrap();
        assert_eq!(all.questions.len(), 1, "only the short-answer question is gradable");
        let q = &all.questions[0];
        assert_eq!((q.id.as_str(), q.reference.as_str(), q.max, q.pending, q.answers.len()), ("s1", "مرجع", 5.0, 1, 2));
        assert_eq!(q.answers.iter().map(|a| (a.answer.as_str(), a.points)).collect::<Vec<_>>(), vec![("أولى", Some(4.0)), ("ثانية", None)], "ordered by submission time");
        let pend = grading_sheet(&w.conn, &w.admin, &w.exam, true).unwrap();
        assert_eq!(pend.questions[0].answers.len(), 1);
        assert_eq!(pend.questions[0].answers[0].answer, "ثانية");
        assert_eq!((pend.title.as_str(), pend.subject_name.as_str()), ("امتحان التحليل", "برمجة"));
    }

    #[test]
    fn batch_grading_is_atomic_validated_corrects_grades_and_notifies_once() {
        let w = world(2);
        let a = attempt(&w, 0, &["q1"], Some("إجابة"), 1);
        let b = attempt(&w, 1, &["q1"], Some("إجابة"), 2);
        let item = |att: &str, q: &str, p: f64| BatchItem { attempt_id: att.into(), question_id: q.into(), points: Some(p), feedback: None };
        let run = |items: Vec<BatchItem>| grade_batch(&w.conn, &w.admin, &w.exam, &BatchReq { grades: items });
        let notified = || -> i64 { w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'assessment_graded'", [], |r| r.get(0)).unwrap() };
        let score = |att: &str| -> (f64, i64) { w.conn.query_row("SELECT score, pending FROM attempts WHERE id = ?1", params![att], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() };
        // one bad item sinks the whole batch, including the valid one before it
        let before = (score(&a), score(&b));
        assert_eq!(run(vec![item(&a, "s1", 3.0), item(&b, "s1", 9.0)]).unwrap_err().0, StatusCode::BAD_REQUEST, "above the question's maximum");
        assert_eq!((score(&a), score(&b), notified()), (before.0, before.1, 0), "nothing was stored and nobody was notified");
        for (label, items, status) in [
            ("objective question", vec![item(&a, "q1", 1.0)], StatusCode::BAD_REQUEST),
            ("unknown question", vec![item(&a, "zz", 1.0)], StatusCode::BAD_REQUEST),
            ("negative points", vec![item(&a, "s1", -1.0)], StatusCode::BAD_REQUEST),
            ("NaN points", vec![item(&a, "s1", f64::NAN)], StatusCode::BAD_REQUEST),
            ("unknown attempt", vec![item("ghost", "s1", 1.0)], StatusCode::NOT_FOUND),
            ("duplicate answer in one request", vec![item(&a, "s1", 1.0), item(&a, "s1", 2.0)], StatusCode::BAD_REQUEST),
            ("empty batch", vec![], StatusCode::BAD_REQUEST),
        ] {
            assert_eq!(run(items).unwrap_err().0, status, "{label}");
        }
        assert_eq!(run((0..501).map(|_| item(&a, "s1", 1.0)).collect()).unwrap_err().0, StatusCode::BAD_REQUEST, "more than 500 items");
        // valid: both graded in one go → both attempts complete → two notifications
        assert_eq!(run(vec![item(&a, "s1", 3.0), item(&b, "s1", 5.0)]).unwrap().updated, 2);
        assert_eq!((score(&a), score(&b), notified()), ((4.0, 0), (6.0, 0), 2), "q1 right (1) + short answer; both complete");
        // correcting a grade later changes the score but does not notify again
        run(vec![item(&a, "s1", 1.0)]).unwrap();
        assert_eq!((score(&a).0, notified()), (2.0, 2));
        // an attempt of ANOTHER exam cannot be smuggled in
        let other = {
            let req: crate::platform_exam_admin::ExamReq = serde_json::from_value(exam_json(json!({"title": "آخر"}))).unwrap();
            serde_json::to_value(crate::platform_exam_admin::create_exam(&w.conn, &w.admin, &req, NOW).unwrap()).unwrap()["id"].as_str().unwrap().to_string()
        };
        assert_eq!(grade_batch(&w.conn, &w.admin, &other, &BatchReq { grades: vec![item(&a, "s1", 1.0)] }).unwrap_err().0, StatusCode::NOT_FOUND, "the attempt does not belong to that exam");
        let log: String = w.conn.query_row("SELECT group_concat(action) FROM audit_log WHERE action = 'grades_saved'", [], |r| r.get(0)).unwrap();
        assert!(log.contains("grades_saved"));
    }

    #[test]
    fn a_blank_short_answer_cannot_be_graded_by_hand() {
        let w = world(1);
        let a = attempt(&w, 0, &["q1"], None, 1);
        let r = grade_batch(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![BatchItem { attempt_id: a, question_id: "s1".into(), points: Some(5.0), feedback: None }] });
        assert_eq!(r.unwrap_err().0, StatusCode::BAD_REQUEST, "nothing was written, so there is nothing to give points for");
    }

    #[test]
    fn analytics_are_exact_for_the_known_scenario() {
        let w = scenario();
        let a = analytics(&w.conn, &w.admin, &w.exam, NOW + 99 * 60_000).unwrap();
        assert_eq!((a.enrolled, a.participants, a.participation_pct, a.attempts_submitted), (8, 6, Some(75.0), 7));
        assert_eq!((a.graded_students, a.awaiting_grading), (5, 1), "the student whose only attempt waits for grading is kept out of the statistics");
        assert_eq!((a.highest, a.lowest, a.median, a.average, a.average_pct), (Some(10.0), Some(1.0), Some(5.0), Some(5.4), Some(54.0)), "scores 1, 2, 5, 9 (best of 3 and 9), 10");
        assert_eq!(a.distribution.iter().map(|b| b.count).collect::<Vec<_>>(), vec![1, 1, 1, 0, 2], "10% | 20% (boundary goes up) | 50% | – | 90% and 100% (100 is in the last bin)");
        assert_eq!((a.distribution[0].from, a.distribution[4].to), (0, 100));
        assert_eq!((a.passed, a.failed), (3, 2), "10, 5 (exactly 50% passes) and 9 pass; 2 and 1 fail");
        assert_eq!(a.avg_duration_sec, Some((10 + 20 + 30 + 40 + 50 + 60 + 70) as f64 * 60.0 / 7.0 * 100.0).map(|x: f64| (x).round() / 100.0), "mean of the seven attempt durations");
        let q = |id: &str| a.questions.iter().find(|q| q.id == id).unwrap();
        assert_eq!((q("q1").rate, q("q1").easy, q("q1").weak, q("q1").graded), (Some(1.0), true, false, 7), "everyone got q1 right → easy");
        assert_eq!((q("q2").rate, q("q2").easy), (Some(0.86), false), "6 of 7 = 0.857 is below the 0.9 'easy' line");
        assert_eq!((q("q4").rate, q("q4").weak, q("q4").easy), (Some(0.57), false, false));
        assert_eq!((q("q5").rate, q("q5").weak), (Some(0.14), true), "only one of 7 got q5 → weak");
        assert_eq!((q("s1").graded, q("s1").answered, q("s1").avg_points, q("s1").weak), (6, 5, Some(1.83), false), "6 graded (the pending one is excluded); 5 wrote something");
        assert_eq!(a.questions.iter().map(|q| q.position).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!((a.tab_leave_students, a.tab_leaves_total), (0, 0));
        // with fewer than five graded answers nothing is flagged
        let small = world(3);
        let x = attempt(&small, 0, &["q1"], None, 1);
        let _ = x;
        let sa = analytics(&small.conn, &small.admin, &small.exam, NOW + 9 * 60_000).unwrap();
        assert!(sa.questions.iter().all(|q| !q.weak && !q.easy), "a single attempt is too little data to call a question weak or easy");
        // an exam nobody took has no statistics, not zeros
        let empty = world(2);
        let ea = analytics(&empty.conn, &empty.admin, &empty.exam, NOW).unwrap();
        assert_eq!((ea.average, ea.median, ea.highest, ea.participants, ea.participation_pct, ea.avg_duration_sec), (None, None, None, 0, Some(0.0), None));
        assert_eq!(ea.distribution.iter().map(|b| b.count).sum::<i64>(), 0);
    }

    #[test]
    fn analytics_count_tab_leaves_and_settle_overdue_attempts_first() {
        let w = world(2);
        let a = attempt(&w, 0, &["q1"], None, 1);
        w.conn.execute("UPDATE attempts SET tab_leaves = 4 WHERE id = ?1", params![a]).unwrap();
        let b = attempt(&w, 1, &[], None, 2);
        w.conn.execute("UPDATE attempts SET tab_leaves = 1 WHERE id = ?1", params![b]).unwrap();
        let r = analytics(&w.conn, &w.admin, &w.exam, NOW + 10 * 60_000).unwrap();
        assert_eq!((r.tab_leave_students, r.tab_leaves_total), (2, 5));
    }

    #[test]
    fn the_students_table_sorts_filters_searches_pages_and_hides_emails_from_teachers() {
        let w = scenario();
        let t = approve(&w, "t@x.com", "s1");
        let q = |f: TableQuery| attempts_table(&w.conn, &w.admin, &w.exam, &f, NOW + 99 * 60_000, false).unwrap();
        let all = q(TableQuery { sort: Some("score".into()), dir: Some("desc".into()), ..Default::default() });
        assert_eq!(all.total, 7);
        assert_eq!(all.items.iter().map(|r| r.score).collect::<Vec<_>>(), vec![10.0, 9.0, 5.0, 4.0, 3.0, 2.0, 1.0]);
        assert_eq!(q(TableQuery { sort: Some("score".into()), dir: Some("asc".into()), ..Default::default() }).items[0].score, 1.0);
        let top = &all.items[0];
        assert_eq!((top.percent, top.passed, top.attempt_no, top.duration_sec, top.email.is_some()), (Some(100.0), Some(true), 1, Some(600), true), "10 minutes = 600 s; admin sees e-mail");
        assert!(all.items.iter().any(|r| r.attempt_no == 2 && r.score == 9.0), "the student's second attempt is numbered 2");
        let f = |res: &str| q(TableQuery { result: Some(res.into()), ..Default::default() });
        assert_eq!((f("passed").total, f("failed").total, f("pending").total, f("expired").total), (3, 3, 1, 0), "10, 5, 9 pass; 2, 1 and the 3-point first attempt fail; 1 pending");
        assert!(f("pending").items[0].passed.is_none(), "no verdict while grading is pending");
        assert_eq!(attempts_table(&w.conn, &w.admin, &w.exam, &TableQuery { result: Some("nonsense".into()), ..Default::default() }, NOW, false).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(attempts_table(&w.conn, &w.admin, &w.exam, &TableQuery { sort: Some("password".into()), ..Default::default() }, NOW, false).unwrap_err().0, StatusCode::BAD_REQUEST, "only whitelisted sort keys");
        assert_eq!(attempts_table(&w.conn, &w.admin, &w.exam, &TableQuery { sort: Some("name; DROP TABLE users".into()), ..Default::default() }, NOW, false).unwrap_err().0, StatusCode::BAD_REQUEST);
        // search by name is case-insensitive and wildcard-safe
        w.conn.execute("UPDATE users SET full_name = 'سارة أحمد' WHERE email = 'st0@x.com'", []).unwrap();
        w.conn.execute("UPDATE users SET full_name = '100% عمر_ا' WHERE email = 'st1@x.com'", []).unwrap();
        assert_eq!(q(TableQuery { q: Some("سارة".into()), ..Default::default() }).total, 1);
        assert_eq!(q(TableQuery { q: Some("%".into()), ..Default::default() }).total, 1, "% only matches a literal percent sign");
        assert_eq!(q(TableQuery { q: Some("_".into()), ..Default::default() }).total, 1, "_ only matches a literal underscore");
        // paging is stable and complete
        let p1 = q(TableQuery { sort: Some("name".into()), dir: Some("asc".into()), limit: Some(3), ..Default::default() });
        let p2 = q(TableQuery { sort: Some("name".into()), dir: Some("asc".into()), limit: Some(3), offset: Some(3), ..Default::default() });
        let p3 = q(TableQuery { sort: Some("name".into()), dir: Some("asc".into()), limit: Some(3), offset: Some(6), ..Default::default() });
        let ids: Vec<&String> = p1.items.iter().chain(&p2.items).chain(&p3.items).map(|r| &r.attempt_id).collect();
        assert_eq!((ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len(), p1.total), (7, 7, 7));
        // an approved teacher sees the rows but never the e-mail addresses
        let tt = attempts_table(&w.conn, &t, &w.exam, &TableQuery::default(), NOW + 99 * 60_000, false).unwrap();
        assert_eq!(tt.total, 7);
        assert!(tt.items.iter().all(|r| r.email.is_none()), "teachers do not get students' e-mail addresses");
    }

    #[test]
    fn exports_contain_the_table_neutralise_formulas_and_use_ascii_file_names() {
        let w = scenario();
        w.conn.execute("UPDATE users SET full_name = '=HYPERLINK(\"http://evil\",\"x\")' WHERE email = 'st0@x.com'", []).unwrap();
        w.conn.execute("UPDATE users SET full_name = 'عمر, \"الثاني\"' WHERE email = 'st1@x.com'", []).unwrap();
        let csv = export(&w.conn, &w.admin, &w.exam, "csv", "results", "en", NOW + 99 * 60_000).unwrap();
        assert_eq!((csv.mime, csv.filename.as_str()), ("text/csv; charset=utf-8", format!("exam-{}-results.csv", &w.exam[..8]).as_str()));
        assert!(csv.filename.is_ascii());
        let text = String::from_utf8(csv.bytes[3..].to_vec()).unwrap();
        let mut rdr = csv::ReaderBuilder::new().from_reader(text.as_bytes());
        let header: Vec<String> = rdr.headers().unwrap().iter().map(String::from).collect();
        assert_eq!(header[..4], ["Student", "Email", "Attempt", "Score"]);
        let rows: Vec<Vec<String>> = rdr.records().map(|r| r.unwrap().iter().map(String::from).collect()).collect();
        assert_eq!(rows.len(), 7);
        assert!(rows.iter().any(|r| r[0] == "'=HYPERLINK(\"http://evil\",\"x\")"), "a malicious name is stored as text with a leading quote: {text}");
        assert!(rows.iter().any(|r| r[0] == "عمر, \"الثاني\""), "commas and quotes survive CSV quoting");
        assert!(rows.iter().all(|r| r.len() == header.len()));
        let verdicts: Vec<&str> = rows.iter().map(|r| r[6].as_str()).collect();
        assert!(verdicts.contains(&"Passed") && verdicts.contains(&"Failed") && verdicts.contains(&"Awaiting grading"), "{verdicts:?}");
        // Arabic labels and the question table
        let ar = export(&w.conn, &w.admin, &w.exam, "csv", "questions", "ar", NOW + 99 * 60_000).unwrap();
        let t = String::from_utf8(ar.bytes[3..].to_vec()).unwrap();
        assert!(t.starts_with("#,النوع,السؤال") && t.contains("ضعيف") && t.contains("سهل"), "weak/easy flags exported: {t}");
        assert_eq!(ar.filename, format!("exam-{}-questions.csv", &w.exam[..8]));
        // teachers' exports have no e-mail column
        let teacher = approve(&w, "t@x.com", "s1");
        let tcsv = export(&w.conn, &teacher, &w.exam, "csv", "results", "en", NOW + 99 * 60_000).unwrap();
        assert!(!String::from_utf8(tcsv.bytes).unwrap().contains("st0@x.com") && !String::from_utf8(csv.bytes).unwrap().is_empty());
        // XLSX: a zip with the right type and name
        let x = export(&w.conn, &w.admin, &w.exam, "xlsx", "", "ar", NOW + 99 * 60_000).unwrap();
        assert_eq!((&x.bytes[..2], x.mime), (&b"PK"[..], "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
        assert!(x.filename.ends_with(".xlsx") && x.filename.is_ascii());
        assert_eq!(export(&w.conn, &w.admin, &w.exam, "pdf", "", "ar", NOW).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(export(&w.conn, &w.admin, &w.exam, "csv", "nonsense", "ar", NOW).unwrap_err().0, StatusCode::BAD_REQUEST);
        let log: String = w.conn.query_row("SELECT group_concat(action) FROM audit_log", [], |r| r.get(0)).unwrap();
        assert!(log.contains("results_exported"), "exports are audited");
    }

    // ───── phase 3-3: a question worth 0 points (voided by an answer-key correction) ─────

    fn void(w: &W, qid: &str) {
        let r = crate::platform_exam_key::CorrectReq { mode: Some("void".into()), ..Default::default() };
        crate::platform_exam_key::correct_question(&w.conn, &w.admin, &w.exam, qid, &r, NOW + 99 * 60_000).unwrap();
    }

    #[test]
    fn a_voided_question_has_no_rate_no_flags_and_nothing_left_to_grade() {
        let w = scenario();
        let before = analytics(&w.conn, &w.admin, &w.exam, NOW + 99 * 60_000).unwrap();
        let q = |a: &Analytics, id: &str| a.questions.iter().position(|x| x.id == id).unwrap();
        assert!(before.questions[q(&before, "q5")].weak, "q5 was the weak question before");
        assert!(!before.questions.iter().any(|x| x.voided));
        void(&w, "q5");
        void(&w, "s1");
        let a = analytics(&w.conn, &w.admin, &w.exam, NOW + 99 * 60_000).unwrap();
        for id in ["q5", "s1"] {
            let s = &a.questions[q(&a, id)];
            assert_eq!((s.voided, s.max, s.rate, s.weak, s.easy), (true, 0.0, None, false, false), "{id}");
            assert!(s.avg_points.map_or(true, f64::is_finite), "{id}");
        }
        assert!(!a.questions[q(&a, "q1")].voided && a.questions[q(&a, "q1")].easy, "the others are unaffected");
        assert_eq!(a.info.total_points, 4.0, "10 − 1 (q5) − 5 (s1)");
        for v in [a.average, a.average_pct, a.median, a.highest, a.lowest, a.avg_duration_sec] {
            assert!(v.map_or(true, f64::is_finite), "{v:?}");
        }
        assert_eq!(a.awaiting_grading, 0, "the answer that waited for grading was the voided one");
        // nothing to grade any more: no queue entry, no questions on the grading sheet
        assert!(pending_exams(&w.conn, &w.admin).unwrap().is_empty());
        assert!(grading_sheet(&w.conn, &w.admin, &w.exam, false).unwrap().questions.is_empty());
        // the table: percentages are of the new total, pass flags follow, no division by zero anywhere
        let t = attempts_table(&w.conn, &w.admin, &w.exam, &TableQuery::default(), NOW + 99 * 60_000, false).unwrap();
        assert!(t.items.iter().all(|r| r.percent.map_or(true, f64::is_finite) && r.pending == 0), "{:?}", t.items);
        // exports: the flag column says so, in both languages
        let ar = export(&w.conn, &w.admin, &w.exam, "csv", "questions", "ar", NOW + 99 * 60_000).unwrap();
        let en = export(&w.conn, &w.admin, &w.exam, "csv", "questions", "en", NOW + 99 * 60_000).unwrap();
        assert!(String::from_utf8(ar.bytes).unwrap().contains("ملغى") && String::from_utf8(en.bytes).unwrap().contains("Voided"));
        assert!(export(&w.conn, &w.admin, &w.exam, "xlsx", "", "ar", NOW + 99 * 60_000).is_ok());
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["questions"][q(&a, "q5")]["voided"], serde_json::json!(true), "the wire carries `voided`");
    }

    #[test]
    fn an_exam_whose_every_question_was_voided_still_produces_every_report() {
        let w = scenario();
        for id in ["q1", "q2", "q3", "q4", "q5", "s1"] {
            void(&w, id);
        }
        let now = NOW + 99 * 60_000;
        let a = analytics(&w.conn, &w.admin, &w.exam, now).unwrap();
        assert_eq!(a.info.total_points, 0.0);
        assert!(a.questions.iter().all(|q| q.voided && q.rate.is_none() && !q.weak && !q.easy));
        assert_eq!((a.passed, a.failed, a.highest, a.lowest), (0, 0, Some(0.0), Some(0.0)), "no points to pass or fail on");
        assert_eq!(a.average_pct, Some(0.0));
        assert_eq!(a.distribution.iter().map(|b| b.count).sum::<i64>(), a.graded_students, "every graded student is in exactly one bin");
        let t = attempts_table(&w.conn, &w.admin, &w.exam, &TableQuery::default(), now, false).unwrap();
        assert!(t.items.iter().all(|r| r.percent.is_none() && r.passed.is_none() && r.total == 0.0));
        for result in ["passed", "failed", "pending"] {
            assert!(attempts_table(&w.conn, &w.admin, &w.exam, &TableQuery { result: Some(result.into()), ..Default::default() }, now, false).is_ok(), "{result}");
        }
        for (format, part) in [("csv", "results"), ("csv", "questions"), ("xlsx", "")] {
            assert!(export(&w.conn, &w.admin, &w.exam, format, part, "en", now).is_ok(), "{format}/{part}");
        }
        let r = crate::platform_exams::results_summary(&w.conn, &w.admin, &w.exam, now).unwrap();
        let v = serde_json::to_value(&r).unwrap();
        assert!(v["average"].as_f64().unwrap().is_finite() && v["highest"].as_f64().unwrap() == 0.0);
        assert_eq!(v["passed"], serde_json::json!(0));
    }

    #[test]
    fn utc_timestamps_are_formatted_without_a_date_library() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00");
        assert_eq!(fmt_utc(1_700_000_000_000), "2023-11-14 22:13");
        assert_eq!(fmt_utc(1_709_208_000_000), "2024-02-29 12:00", "leap day");
        assert_eq!(fmt_utc(951_782_399_000), "2000-02-28 23:59");
        assert_eq!(fmt_utc(4_102_444_799_000), "2099-12-31 23:59");
        let _ = new_id();
    }

    // ───── phase 3-4: queue filter, comments in the sheet and the batch, per-student view, absent students ─────

    const MIN: i64 = 60_000;

    fn names(w: &W) {
        for (i, n) in ["أمل", "بسمة", "جميل", "دانة", "هدى", "وليد", "زياد", "ياسر"].iter().enumerate().take(w.students.len()) {
            w.conn.execute("UPDATE users SET full_name = ?2 WHERE id = ?1", params![w.students[i].id, n]).unwrap();
        }
    }

    /// A teacher's own exam in `subject` (same questions as the admin exam), published.
    fn teacher_exam(w: &W, t: &User, subject: &str, title: &str) -> String {
        let treq: crate::platform_exams::AssessmentReq =
            serde_json::from_value(json!({"subject_id": subject, "title": title, "questions": exam_json(json!({}))["questions"], "status": "published", "max_attempts": 5})).unwrap();
        crate::platform_exams::create_assessment(&w.conn, t, &treq).unwrap().id()
    }

    /// Starts and submits an attempt of `exam` (not necessarily `w.exam`): the short answer `s1` is written when given.
    fn sit_exam(w: &W, who: usize, exam: &str, short: Option<&str>, at: i64) -> String {
        let u = &w.students[who];
        let s = start_attempt(&w.conn, u, exam, at).unwrap();
        let mut a: HashMap<String, String> = (1..=5).map(|i| (format!("q{i}"), "B".to_string())).collect();
        if let Some(t) = short {
            a.insert("s1".into(), t.into());
        }
        submit_attempt(&w.conn, u, &s.attempt_id, &a, at + 1).unwrap();
        s.attempt_id
    }

    fn json_of<T: Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    #[test]
    fn the_pending_queue_filters_by_subject_counts_attempts_and_says_who_owns_the_exam() {
        let w = world(4);
        names(&w);
        let owner = approve(&w, "owner@x.com", "s1");
        let other_teacher = approve(&w, "other@x.com", "s1");
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s2','approved',0)", params![owner.id]).unwrap();
        for s in &w.students {
            w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s2',0)", params![s.id]).unwrap();
        }
        let mine_s1 = teacher_exam(&w, &owner, "s1", "امتحاني في البرمجة");
        let mine_s2 = teacher_exam(&w, &owner, "s2", "امتحاني في المادة الأخرى");
        // the admin exam: two waiting attempts + one that nothing waits on (objective only)
        sit_exam(&w, 0, &w.exam, Some("أ"), NOW);
        sit_exam(&w, 1, &w.exam, Some("ب"), NOW + MIN);
        sit_exam(&w, 2, &w.exam, None, NOW + 2 * MIN);
        sit_exam(&w, 0, &mine_s1, Some("ج"), NOW + 3 * MIN);
        sit_exam(&w, 3, &mine_s2, Some("د"), NOW + 4 * MIN);

        let rows = |u: &User, subject: Option<&str>| -> Vec<PendingExam> {
            match subject {
                Some(s) => pending_exams_for_subject(&w.conn, u, s).unwrap(),
                None => pending_exams(&w.conn, u).unwrap(),
            }
        };
        let by_id = |rows: &[PendingExam], id: &str| rows.iter().position(|r| r.assessment_id == id).unwrap_or_else(|| panic!("{id} missing from the queue"));
        // the admin sees every exam; by the one ownership rule (`AssessmentInfo::owned_by`) an admin owns the exams an admin
        // created (no teacher) and only looks at a teacher's
        let all = rows(&w.admin, None);
        assert_eq!(all.len(), 3);
        assert_eq!(
            (all[by_id(&all, &w.exam)].owned, all[by_id(&all, &mine_s1)].owned, all[by_id(&all, &mine_s2)].owned),
            (true, false, false),
            "an admin owns the admin's exam, not the teachers'"
        );
        let admin_row = &all[by_id(&all, &w.exam)];
        assert_eq!((admin_row.subject_id.as_str(), admin_row.pending_attempts, admin_row.pending_answers, admin_row.attempts_total), ("s1", 2, 2, 3), "3 submitted attempts, 2 of them waiting");
        // oldest waiting first, unchanged
        assert_eq!(all.iter().map(|r| r.assessment_id.as_str()).collect::<Vec<_>>(), [w.exam.as_str(), mine_s1.as_str(), mine_s2.as_str()]);
        // the owner: their own exams are theirs, the admin's exam is graded by approval
        let mine = rows(&owner, None);
        assert_eq!(mine.len(), 3);
        assert_eq!((mine[by_id(&mine, &mine_s1)].owned, mine[by_id(&mine, &mine_s2)].owned, mine[by_id(&mine, &w.exam)].owned), (true, true, false));
        // and `owned` is exactly the ownership rule everything else uses (exam builder, lifecycle, answer key)
        for who in [&w.admin, &owner, &other_teacher] {
            for r in rows(who, None) {
                assert_eq!(r.owned, get_info(&w.conn, &who.id, &r.assessment_id).unwrap().owned_by(who), "{} on {}", who.role, r.title);
            }
        }
        // a colleague approved for s1 only: the admin exam, not the owner's exams
        let colleague = rows(&other_teacher, None);
        assert_eq!(colleague.iter().map(|r| r.assessment_id.as_str()).collect::<Vec<_>>(), [w.exam.as_str()]);
        assert!(!colleague[0].owned);
        // the subject filter
        let s2 = rows(&owner, Some("s2"));
        assert_eq!(s2.iter().map(|r| (r.assessment_id.as_str(), r.subject_id.as_str(), r.owned)).collect::<Vec<_>>(), [(mine_s2.as_str(), "s2", true)]);
        assert_eq!(rows(&w.admin, Some("s1")).len(), 2);
        assert_eq!(rows(&owner, Some("s1")).len(), 2, "the owner's s1 exam and the admin's");
        // blank = no filter; an id that matches nothing is an empty list, whatever it contains
        assert_eq!(rows(&w.admin, Some("")).len(), 3);
        assert_eq!(rows(&w.admin, Some("   ")).len(), 3);
        for odd in ["nope", "%", "s%", "_1", "s1' OR '1'='1", "S1", "s1 "] {
            assert!(rows(&w.admin, Some(odd)).is_empty(), "{odd:?}");
        }
        // a filter never widens what the caller may see
        assert!(rows(&other_teacher, Some("s2")).is_empty());
        assert_eq!(pending_exams_for_subject(&w.conn, &w.students[0], "s1").unwrap_err().0, StatusCode::FORBIDDEN);
        // the wire carries the new fields
        let j = json_of(&all[by_id(&all, &mine_s2)]);
        for key in ["assessment_id", "title", "subject_id", "subject_name", "pending_attempts", "pending_answers", "oldest_submitted_at", "attempts_total", "owned"] {
            assert!(j.get(key).is_some(), "{key} in {j}");
        }
    }

    #[test]
    fn the_sheet_shows_each_comment_and_when_a_grade_was_written() {
        let w = world(3);
        let a = attempt(&w, 0, &[], Some("أولى"), 1);
        let b = attempt(&w, 1, &[], Some("ثانية"), 2);
        attempt(&w, 2, &[], Some("ثالثة"), 3);
        grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![
            BatchItem { attempt_id: a.clone(), question_id: "s1".into(), points: Some(4.0), feedback: Some("ممتاز".into()) },
            BatchItem { attempt_id: b.clone(), question_id: "s1".into(), points: None, feedback: Some("تحتاج توضيحاً".into()) },
        ] }, NOW + 77).unwrap();
        let sheet = grading_sheet(&w.conn, &w.admin, &w.exam, false).unwrap();
        let answers = &sheet.questions[0].answers;
        let by = |att: &str| answers.iter().find(|x| x.attempt_id == att).unwrap();
        assert_eq!((by(&a).points, by(&a).feedback.as_deref(), by(&a).graded_at), (Some(4.0), Some("ممتاز"), Some(NOW + 77)));
        assert_eq!((by(&b).points, by(&b).feedback.as_deref(), by(&b).graded_at), (None, Some("تحتاج توضيحاً"), None), "a comment alone does not grade the answer");
        let c = answers.iter().find(|x| x.answer == "ثالثة").unwrap();
        assert_eq!((c.points, c.feedback.clone(), c.graded_at), (None, None, None));
        assert_eq!(sheet.questions[0].pending, 2);
        // "pending only" keeps the commented-but-ungraded answer, with its comment
        let pending = grading_sheet(&w.conn, &w.admin, &w.exam, true).unwrap();
        let ids: Vec<&str> = pending.questions[0].answers.iter().map(|x| x.attempt_id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&b.as_str()) && !ids.contains(&a.as_str()));
        let j = json_of(&sheet.questions[0].answers[0]);
        for key in ["attempt_id", "student_name", "answer", "points", "submitted_at", "feedback", "graded_at"] {
            assert!(j.get(key).is_some(), "{key} in {j}");
        }
    }

    #[test]
    fn a_batch_item_carries_points_a_comment_or_both_and_one_with_neither_is_refused() {
        let w = world(2);
        let a = attempt(&w, 0, &["q1"], Some("إجابة"), 1);
        let b = attempt(&w, 1, &["q1"], Some("إجابة"), 2);
        let item = |att: &str, p: Option<f64>, f: Option<&str>| BatchItem { attempt_id: att.into(), question_id: "s1".into(), points: p, feedback: f.map(String::from) };
        let run = |items: Vec<BatchItem>| grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: items }, NOW + 5);
        let state = |att: &str| -> (f64, i64, Option<String>) {
            let raw: String = w.conn.query_row("SELECT results FROM attempts WHERE id = ?1", params![att], |r| r.get(0)).unwrap();
            let o = serde_json::from_str::<Vec<crate::platform_exams::Outcome>>(&raw).unwrap().into_iter().find(|o| o.id == "s1").unwrap();
            (w.conn.query_row("SELECT score FROM attempts WHERE id = ?1", params![att], |r| r.get(0)).unwrap(), w.conn.query_row("SELECT pending FROM attempts WHERE id = ?1", params![att], |r| r.get(0)).unwrap(), o.feedback)
        };
        // neither points nor comment: the item changes nothing, so it is refused (and nothing else is stored)
        let e = run(vec![item(&a, Some(3.0), None), item(&b, None, None)]).unwrap_err();
        assert_eq!((e.0, e.1.contains("invalid_selection")), (StatusCode::BAD_REQUEST, true));
        assert_eq!((state(&a), state(&b)), ((1.0, 1, None), (1.0, 1, None)));
        // a comment alone: stored, the answer still waits
        assert_eq!(run(vec![item(&a, None, Some("فكرة أولى"))]).unwrap().updated, 1);
        assert_eq!(state(&a), (1.0, 1, Some("فكرة أولى".into())));
        // points and comment together for one answer, points alone for the other
        run(vec![item(&a, Some(3.0), Some("أحسنت")), item(&b, Some(5.0), None)]).unwrap();
        assert_eq!((state(&a), state(&b)), ((4.0, 0, Some("أحسنت".into())), (6.0, 0, None)));
        // a comment cleared through the batch
        run(vec![item(&a, None, Some(""))]).unwrap();
        assert_eq!(state(&a).2, None);
        // the same answer twice is refused whichever fields the two items carry
        for (x, y) in [(item(&a, Some(1.0), None), item(&a, Some(2.0), None)), (item(&a, Some(1.0), None), item(&a, None, Some("x"))), (item(&a, None, Some("x")), item(&a, None, Some("y")))] {
            assert_eq!(run(vec![x, y]).unwrap_err().0, StatusCode::BAD_REQUEST);
        }
        // the JSON of an item: points and feedback are optional on the wire
        let i: BatchItem = serde_json::from_str(r#"{"attempt_id":"a","question_id":"s1","feedback":"x"}"#).unwrap();
        assert!(i.points.is_none() && i.feedback.as_deref() == Some("x"));
        let i: BatchItem = serde_json::from_str(r#"{"attempt_id":"a","question_id":"s1","points":2,"feedback":null}"#).unwrap();
        assert!(i.points == Some(2.0) && i.feedback.is_none());
        // one summary row per call, whatever the items hold (the old 'grades_saved' record is kept)
        let n: i64 = w.conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'grades_saved'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 3, "the three calls that succeeded (the refused ones leave no record)");
    }

    #[test]
    fn a_batch_is_all_or_nothing_for_comments_audit_rows_and_notices_too() {
        let w = world(3);
        let a = attempt(&w, 0, &["q1"], Some("إجابة"), 1);
        let b = attempt(&w, 1, &["q1"], Some("إجابة"), 2);
        let c = attempt(&w, 2, &["q1"], Some("إجابة"), 3);
        let item = |att: &str, p: Option<f64>, f: Option<&str>| BatchItem { attempt_id: att.into(), question_id: "s1".into(), points: p, feedback: f.map(String::from) };
        grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![item(&a, Some(2.0), None)] }, NOW).unwrap();
        let snapshot = |w: &W| -> (String, i64, i64, i64) {
            (
                w.conn.query_row("SELECT group_concat(results || score || pending, '|') FROM (SELECT * FROM attempts ORDER BY id)", [], |r| r.get(0)).unwrap(),
                w.conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'answer_regraded'", [], |r| r.get(0)).unwrap(),
                w.conn.query_row("SELECT count(*) FROM notifications", [], |r| r.get(0)).unwrap(),
                w.conn.query_row("SELECT count(*) FROM audit_log", [], |r| r.get(0)).unwrap(),
            )
        };
        let before = snapshot(&w);
        // a correction + a comment + a first grade, and a last item whose comment is too long: NONE of it may stay
        let too_long = "ع".repeat(501);
        let r = grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![
            item(&a, Some(4.0), Some("تعليق")),
            item(&b, Some(5.0), Some("تعليق")),
            item(&c, None, Some(&too_long)),
        ] }, NOW + 10);
        assert_eq!(r.unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(snapshot(&w), before, "every attempt, audit row and notification is exactly as before");
        // and the same batch without the bad item goes through whole
        grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![item(&a, Some(4.0), Some("تعليق")), item(&b, Some(5.0), Some("تعليق")), item(&c, None, Some("ملاحظة"))] }, NOW + 20).unwrap();
        let after = snapshot(&w);
        assert_eq!(after.1, 1, "the correction of a's grade left its audit row");
        let student_notes = |kind: &str| -> i64 { w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = ?1", params![kind], |r| r.get(0)).unwrap() };
        assert_eq!((student_notes("feedback_added"), student_notes("score_changed"), student_notes("assessment_graded")), (3, 1, 2), "a: comment + correction; b: comment + graded; c: comment");
        // an attempt of another exam inside the batch rolls the lot back, comments included
        let other = {
            let req: crate::platform_exam_admin::ExamReq = serde_json::from_value(exam_json(json!({"title": "آخر"}))).unwrap();
            serde_json::to_value(crate::platform_exam_admin::create_exam(&w.conn, &w.admin, &req, NOW).unwrap()).unwrap()["id"].as_str().unwrap().to_string()
        };
        let before = snapshot(&w);
        let e = grade_batch_at(&w.conn, &w.admin, &other, &BatchReq { grades: vec![item(&a, Some(1.0), Some("لن تُحفظ"))] }, NOW + 30).unwrap_err();
        assert_eq!(e.0, StatusCode::NOT_FOUND);
        assert_eq!(snapshot(&w), before);
        // who may batch: a colleague without the exam cannot, before anything is read
        let stranger = approve(&w, "stranger@x.com", "s2");
        assert_eq!(grade_batch_at(&w.conn, &stranger, &w.exam, &BatchReq { grades: vec![item(&a, None, Some("x"))] }, NOW).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(snapshot(&w), before);
    }

    #[test]
    fn a_batch_that_fails_halfway_undoes_the_attempts_it_already_stored() {
        let w = world(2);
        let a = attempt(&w, 0, &["q1"], Some("إجابة"), 1);
        let b = attempt(&w, 1, &["q1"], Some("إجابة"), 2);
        let item = |att: &str, p: Option<f64>, f: Option<&str>| BatchItem { attempt_id: att.into(), question_id: "s1".into(), points: p, feedback: f.map(String::from) };
        grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![item(&a, Some(2.0), None), item(&b, Some(2.0), None)] }, NOW).unwrap();
        let snapshot = || -> String {
            w.conn.query_row("SELECT (SELECT group_concat(results || score, '|') FROM attempts) || (SELECT count(*) FROM audit_log) || (SELECT count(*) FROM notifications)", [], |r| r.get(0)).unwrap()
        };
        let before = snapshot();
        // attempts are handled in id order: the student of the LAST one is the one whose notice cannot be written, so the
        // first attempt has been stored completely by the time the batch fails
        let last_student = if a > b { &w.students[0] } else { &w.students[1] };
        w.conn.execute_batch(&format!("CREATE TRIGGER no_notice BEFORE INSERT ON notifications WHEN new.user_id = '{}' BEGIN SELECT RAISE(ABORT, 'no notice'); END;", last_student.id)).unwrap();
        let e = grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![item(&a, Some(5.0), Some("أ")), item(&b, Some(5.0), Some("ب"))] }, NOW + 9).unwrap_err();
        assert_eq!(e.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(snapshot(), before, "the first attempt's new grade, its comment, its audit row and its notices were rolled back with the second");
        w.conn.execute_batch("DROP TRIGGER no_notice").unwrap();
        assert_eq!(grade_batch_at(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![item(&a, Some(5.0), Some("أ")), item(&b, Some(5.0), Some("ب"))] }, NOW + 10).unwrap().updated, 2);
        assert_ne!(snapshot(), before);
    }

    // ----- the per-student view -----

    fn q(f: impl FnOnce(&mut TableQuery)) -> TableQuery {
        let mut t = TableQuery { view: Some("students".into()), ..Default::default() };
        f(&mut t);
        t
    }

    fn students(w: &W, who: &User, f: TableQuery) -> StudentsPage {
        students_table(&w.conn, who, &w.exam, &f, NOW + 99 * MIN).unwrap()
    }

    fn name_order(p: &StudentsPage) -> Vec<&str> {
        p.items.iter().map(|s| s.student_name.as_str()).collect()
    }

    /// `scenario()` with names: أمل 10 | بسمة 5 | جميل 2 | دانة 1 | هدى (3 then 9) | وليد 4 + one answer waiting | زياد, ياسر never came.
    fn named_scenario() -> W {
        let w = scenario();
        names(&w);
        w
    }

    #[test]
    fn the_per_student_view_aggregates_best_last_attempts_pending_and_tab_leaves() {
        let w = named_scenario();
        let p = students(&w, &w.admin, q(|_| {}));
        assert_eq!(p.total, 6, "one row per student who has an attempt; the two who never came are not here");
        let by = |n: &str| p.items.iter().find(|s| s.student_name == n).unwrap_or_else(|| panic!("{n} missing"));
        let top = by("أمل");
        assert_eq!((top.attempts, top.best_score, top.best_percent, top.passed, top.pending), (1, Some(10.0), Some(100.0), Some(true), 0));
        assert_eq!(top.best_attempt_id.as_deref(), Some(top.last_attempt_id.as_str()), "a single attempt is both the best and the last");
        assert_eq!((top.last_status.as_str(), top.last_submitted_at.is_some()), ("submitted", true));
        assert_eq!((by("بسمة").best_score, by("بسمة").passed), (Some(5.0), Some(true)), "exactly the pass mark passes");
        assert_eq!((by("جميل").best_score, by("جميل").passed), (Some(2.0), Some(false)));
        assert_eq!((by("دانة").best_percent, by("دانة").passed), (Some(10.0), Some(false)));
        // two attempts: the best is the 9, the last is the later one, which here is the same
        let two = by("هدى");
        assert_eq!((two.attempts, two.best_score, two.passed), (2, Some(9.0), Some(true)));
        let ids: Vec<(String, f64, i64)> = w.conn.prepare("SELECT id, score, started_at FROM attempts WHERE student_id = ?1 ORDER BY started_at").unwrap()
            .query_map(params![w.students[4].id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!((ids.len(), ids[0].1, ids[1].1), (2, 3.0, 9.0));
        assert_eq!((two.best_attempt_id.as_deref(), two.last_attempt_id.as_str()), (Some(ids[1].0.as_str()), ids[1].0.as_str()));
        // an answer still waiting: counted, no verdict
        let waiting = by("وليد");
        assert_eq!((waiting.best_score, waiting.passed, waiting.pending), (Some(4.0), None, 1));
        // tab leaves are summed over all of a student's attempts
        w.conn.execute("UPDATE attempts SET tab_leaves = 2 WHERE id = ?1", params![ids[0].0]).unwrap();
        w.conn.execute("UPDATE attempts SET tab_leaves = 3 WHERE id = ?1", params![ids[1].0]).unwrap();
        assert_eq!(students(&w, &w.admin, q(|_| {})).items.iter().find(|s| s.student_name == "هدى").unwrap().tab_leaves, 5);
        // admin: e-mail; the wire shape
        assert!(top.email.is_some());
        let j = json_of(top);
        for key in ["student_id", "student_name", "email", "attempts", "best_score", "best_percent", "best_attempt_id", "last_attempt_id", "last_status", "last_submitted_at", "passed", "pending", "tab_leaves"] {
            assert!(j.get(key).is_some(), "{key} in {j}");
        }
        assert_eq!(json_of(&p)["total"], 6);
    }

    #[test]
    fn a_fully_graded_attempt_is_the_best_even_when_another_one_waiting_scores_more_on_paper() {
        let w = named_scenario();
        // أمل's second attempt (max 2): 5 objective points + a written answer nobody has graded = 5 on paper, 10 graded
        let second = attempt(&w, 0, &["q1", "q2", "q3", "q4", "q5"], Some("محاولة ثانية"), 120);
        let p = students(&w, &w.admin, q(|_| {}));
        let s = p.items.iter().find(|s| s.student_name == "أمل").unwrap();
        assert_eq!((s.attempts, s.best_score, s.passed, s.pending), (2, Some(10.0), Some(true), 1), "the graded 10 stays the best; the waiting answer is still counted");
        assert_eq!(s.last_attempt_id, second, "the last attempt is the newest one");
        assert_eq!(s.last_status, "submitted");
        // 'pending' keeps every student with an answer waiting in ANY submitted attempt
        let pend = students(&w, &w.admin, q(|f| f.result = Some("pending".into())));
        let mut got = name_order(&pend);
        got.sort();
        assert_eq!(got, ["أمل", "وليد"]);
        // 'passed' follows the best attempt: أمل still passes
        assert!(name_order(&students(&w, &w.admin, q(|f| f.result = Some("passed".into())))).contains(&"أمل"));
    }

    #[test]
    fn the_per_student_view_sorts_filters_searches_and_pages() {
        let w = named_scenario();
        let sorted = |sort: &str, dir: &str| -> Vec<String> {
            students(&w, &w.admin, q(|f| { f.sort = Some(sort.into()); f.dir = Some(dir.into()); })).items.iter().map(|s| s.student_name.clone()).collect()
        };
        assert_eq!(sorted("best", "desc"), ["أمل", "هدى", "بسمة", "وليد", "جميل", "دانة"], "10, 9, 5, 4 (waiting), 2, 1");
        assert_eq!(sorted("best", "asc"), ["دانة", "جميل", "وليد", "بسمة", "هدى", "أمل"]);
        assert_eq!(sorted("attempts", "desc")[0], "هدى", "two attempts");
        assert_eq!(sorted("last", "desc")[0], "وليد", "the newest activity first");
        assert_eq!(sorted("last", "asc")[0], "أمل");
        let by_name = sorted("name", "asc");
        let mut expected = by_name.clone();
        expected.sort_by_key(|n| n.to_lowercase());
        assert_eq!(by_name, expected);
        assert_eq!(sorted("name", "desc"), by_name.iter().rev().cloned().collect::<Vec<_>>());
        assert_eq!(name_order(&students(&w, &w.admin, q(|_| {}))), sorted("last", "desc").iter().map(String::as_str).collect::<Vec<_>>(), "default: newest activity first");
        // filters
        let res = |r: &str| -> Vec<String> { let mut v: Vec<String> = students(&w, &w.admin, q(|f| f.result = Some(r.into()))).items.iter().map(|s| s.student_name.clone()).collect(); v.sort(); v };
        assert_eq!(res("passed"), ["أمل", "بسمة", "هدى"]);
        assert_eq!(res("failed"), ["جميل", "دانة"]);
        assert_eq!(res("pending"), ["وليد"]);
        assert_eq!(res("all").len(), 6);
        assert_eq!(res("").len(), 6);
        assert_eq!(students(&w, &w.admin, q(|f| f.result = Some("passed".into()))).total, 3, "the total follows the filter");
        // search: case-insensitive, and the wildcards are plain characters
        w.conn.execute("UPDATE users SET full_name = 'Alice Smith' WHERE id = ?1", params![w.students[0].id]).unwrap();
        w.conn.execute("UPDATE users SET full_name = '100% عمر_ا' WHERE id = ?1", params![w.students[1].id]).unwrap();
        let find = |s: &str| students(&w, &w.admin, q(|f| f.q = Some(s.into()))).total;
        assert_eq!((find("ALICE"), find("alice smith"), find("  alice "), find("%"), find("_"), find("100%"), find("عمر_ا"), find("zzz")), (1, 1, 1, 1, 1, 1, 1, 0));
        assert_eq!(find("\\"), 0, "a backslash is just a character too");
        // paging: complete, no overlap, stable
        let mut seen: Vec<String> = vec![];
        for off in [0, 2, 4, 6] {
            let p = students(&w, &w.admin, q(|f| { f.sort = Some("name".into()); f.dir = Some("asc".into()); f.limit = Some(2); f.offset = Some(off); }));
            assert_eq!(p.total, 6);
            seen.extend(p.items.iter().map(|s| s.student_id.clone()));
        }
        assert_eq!((seen.len(), seen.iter().collect::<std::collections::HashSet<_>>().len()), (6, 6));
        assert_eq!(students(&w, &w.admin, q(|f| f.limit = Some(0))).items.len(), 1, "limit is at least 1");
        assert_eq!(students(&w, &w.admin, q(|f| f.limit = Some(100_000))).items.len(), 6);
        assert_eq!(students(&w, &w.admin, q(|f| f.offset = Some(-5))).items.len(), 6, "a negative offset is the start");
        assert!(students(&w, &w.admin, q(|f| f.offset = Some(60))).items.is_empty());
    }

    #[test]
    fn students_who_never_submitted_show_up_with_empty_results_and_always_sort_last_by_best() {
        let w = named_scenario();
        // زياد started and left; ياسر's time ran out with nothing saved
        let started = start_attempt(&w.conn, &w.students[6], &w.exam, NOW + 80 * MIN).unwrap();
        let gone = start_attempt(&w.conn, &w.students[7], &w.exam, NOW + 81 * MIN).unwrap();
        w.conn.execute("UPDATE attempts SET status = 'expired' WHERE id = ?1", params![gone.attempt_id]).unwrap();
        let p = students(&w, &w.admin, q(|f| { f.sort = Some("best".into()); f.dir = Some("desc".into()); }));
        assert_eq!(p.total, 8);
        let tail: Vec<&StudentSummary> = p.items.iter().rev().take(2).collect();
        assert!(tail.iter().all(|s| s.best_score.is_none()), "no best attempt: last, whichever way it runs");
        let asc = students(&w, &w.admin, q(|f| { f.sort = Some("best".into()); f.dir = Some("asc".into()); }));
        assert!(asc.items.iter().rev().take(2).all(|s| s.best_score.is_none()));
        let z = p.items.iter().find(|s| s.student_name == "زياد").unwrap();
        assert_eq!((z.attempts, z.best_score, z.best_percent, z.best_attempt_id.clone(), z.passed, z.last_status.as_str(), z.last_submitted_at, z.pending), (1, None, None, None, None, "in_progress", None, 0));
        assert_eq!(z.last_attempt_id, started.attempt_id);
        assert_eq!(p.items.iter().find(|s| s.student_name == "ياسر").unwrap().last_status, "expired");
        assert!(students(&w, &w.admin, q(|f| f.result = Some("passed".into()))).items.iter().all(|s| s.best_score.is_some()), "no verdict without a submitted attempt");
    }

    #[test]
    fn only_graders_see_the_per_student_view_and_only_admins_see_e_mail_addresses() {
        let w = named_scenario();
        let approved = approve(&w, "approved@x.com", "s1");
        let other_subject = approve(&w, "other@x.com", "s2");
        let unapproved = insert_test_user(&w.conn, "un@x.com", "teacher", "active");
        let teacher_exam_id = teacher_exam(&w, &approved, "s1", "امتحان المعلم");
        let colleague = approve(&w, "colleague@x.com", "s1");
        let go = |who: &User, exam: &str| results_view(&w.conn, who, exam, &q(|_| {}), NOW + 99 * MIN);
        // an approved teacher grades an admin's exam; admins grade everything
        assert!(go(&w.admin, &w.exam).is_ok() && go(&approved, &w.exam).is_ok() && go(&w.admin, &teacher_exam_id).is_ok());
        // everyone else gets a 403 - and for every view/parameter alike, so nothing about the exam leaks
        for (who, exam, label) in [
            (&other_subject, &w.exam, "a teacher of another subject"),
            (&unapproved, &w.exam, "a teacher with no approval"),
            (&w.students[0], &w.exam, "a student"),
            (&colleague, &teacher_exam_id, "a colleague on someone else's exam"),
        ] {
            for view in ["students", "attempts", "garbage"] {
                let t = TableQuery { view: Some(view.into()), ..Default::default() };
                assert_eq!(results_view(&w.conn, who, exam, &t, NOW).unwrap_err().0, StatusCode::FORBIDDEN, "{label} / {view}");
            }
            assert_eq!(students_table(&w.conn, who, exam, &q(|_| {}), NOW).unwrap_err().0, StatusCode::FORBIDDEN, "{label}");
        }
        assert_eq!(results_view(&w.conn, &w.admin, "no-such-exam", &q(|_| {}), NOW).unwrap_err().0, StatusCode::NOT_FOUND);
        // e-mail addresses: admins only
        let admin_json = json_of(&go(&w.admin, &w.exam).unwrap());
        assert!(admin_json["items"].as_array().unwrap().iter().all(|s| s["email"].as_str().map_or(false, |e| e.ends_with("@x.com"))));
        for teacher_view in [go(&approved, &w.exam).unwrap(), go(&w.admin, &teacher_exam_id).map(|_| json_of(&students(&w, &approved, q(|_| {})))).unwrap()] {
            let text = teacher_view.to_string();
            assert!(!text.contains("@x.com") && !text.contains("st0") && teacher_view["items"].as_array().unwrap().iter().all(|s| s["email"].is_null()), "{text}");
        }
        // views and sorts are whitelists
        let bad = |t: TableQuery| results_view(&w.conn, &w.admin, &w.exam, &t, NOW).unwrap_err();
        for t in [
            TableQuery { view: Some("everything".into()), ..Default::default() },
            q(|f| f.sort = Some("score".into())), // a sort of the attempts view, not of this one
            q(|f| f.sort = Some("password".into())),
            q(|f| f.sort = Some("name; DROP TABLE users".into())),
            q(|f| f.dir = Some("sideways".into())),
            q(|f| f.result = Some("expired".into())),
            q(|f| f.result = Some("in_progress".into())),
            q(|f| f.result = Some("nonsense".into())),
        ] {
            let e = bad(t);
            assert_eq!((e.0, e.1.contains("invalid_filter")), (StatusCode::BAD_REQUEST, true));
        }
        // the default view is still the attempts table
        let default = results_view(&w.conn, &w.admin, &w.exam, &TableQuery::default(), NOW + 99 * MIN).unwrap();
        assert!(default["items"][0].get("attempt_id").is_some() && default["items"][0].get("student_id").is_none());
    }

    // ----- who has not taken it -----

    fn absent(w: &W, who: &User, f: AbsentQuery) -> AbsentPage {
        absent_students(&w.conn, who, &w.exam, &f).unwrap()
    }

    fn absent_names(p: &AbsentPage) -> Vec<&str> {
        p.items.iter().map(|s| s.student_name.as_str()).collect()
    }

    #[test]
    fn the_absent_list_is_the_active_class_minus_anyone_with_an_attempt_of_any_status() {
        let w = world(5);
        names(&w);
        attempt(&w, 0, &["q1"], None, 1); // submitted
        start_attempt(&w.conn, &w.students[1], &w.exam, NOW + MIN).unwrap(); // started, never finished
        let gone = start_attempt(&w.conn, &w.students[2], &w.exam, NOW + 2 * MIN).unwrap();
        w.conn.execute("UPDATE attempts SET status = 'expired' WHERE id = ?1", params![gone.attempt_id]).unwrap();
        // a suspended student, and students of other classes, are not part of this class
        let suspended = insert_test_user(&w.conn, "susp@x.com", "student", "suspended");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![suspended.id]).unwrap();
        let elsewhere = insert_test_user(&w.conn, "else@x.com", "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s2',0)", params![elsewhere.id]).unwrap();
        insert_test_user(&w.conn, "nobody@x.com", "student", "active"); // not enrolled anywhere
        let p = absent(&w, &w.admin, AbsentQuery::default());
        assert_eq!(absent_names(&p), ["دانة", "هدى"], "the two who never touched it, by name");
        assert_eq!((p.total, p.enrolled), (2, 5), "the class is the 5 active enrolled students");
        assert!(p.items.iter().all(|s| s.student_id == w.students[3].id || s.student_id == w.students[4].id));
        // as soon as one of them starts, they are no longer absent
        start_attempt(&w.conn, &w.students[3], &w.exam, NOW + 9 * MIN).unwrap();
        assert_eq!(absent_names(&absent(&w, &w.admin, AbsentQuery::default())), ["هدى"]);
        // a student who leaves the subject is not part of the class any more
        w.conn.execute("DELETE FROM subject_enrollments WHERE student_id = ?1", params![w.students[4].id]).unwrap();
        let p = absent(&w, &w.admin, AbsentQuery::default());
        assert_eq!((p.items.len(), p.total, p.enrolled), (0, 0, 4));
    }

    #[test]
    fn the_absent_list_has_names_only_never_an_e_mail_not_even_for_an_admin() {
        let w = world(3);
        names(&w);
        let approved = approve(&w, "approved@x.com", "s1");
        for who in [&w.admin, &approved] {
            let p = absent(&w, who, AbsentQuery::default());
            let j = json_of(&p);
            assert_eq!(p.items.len(), 3);
            for item in j["items"].as_array().unwrap() {
                assert_eq!(item.as_object().unwrap().keys().collect::<Vec<_>>(), ["student_id", "student_name"], "{}", who.role);
            }
            assert!(!j.to_string().contains("@") && !j.to_string().contains("st0"), "{j}");
            assert_eq!(j.as_object().unwrap().keys().collect::<Vec<_>>(), ["enrolled", "items", "total"]);
        }
    }

    #[test]
    fn the_absent_list_is_for_graders_only_and_survives_a_switched_off_subject() {
        let w = world(2);
        let approved = approve(&w, "approved@x.com", "s1");
        let other_subject = approve(&w, "other@x.com", "s2");
        let unapproved = insert_test_user(&w.conn, "un@x.com", "teacher", "active");
        let colleague = approve(&w, "colleague@x.com", "s1");
        let own = teacher_exam(&w, &approved, "s1", "امتحان المعلم");
        for (who, label) in [(&other_subject, "teacher of another subject"), (&unapproved, "no approval"), (&w.students[0], "student")] {
            assert_eq!(absent_students(&w.conn, who, &w.exam, &AbsentQuery::default()).unwrap_err().0, StatusCode::FORBIDDEN, "{label}");
        }
        assert_eq!(absent_students(&w.conn, &colleague, &own, &AbsentQuery::default()).unwrap_err().0, StatusCode::FORBIDDEN, "a colleague on a teacher's own exam");
        assert!(absent_students(&w.conn, &approved, &own, &AbsentQuery::default()).is_ok() && absent_students(&w.conn, &approved, &w.exam, &AbsentQuery::default()).is_ok());
        assert_eq!(absent_students(&w.conn, &w.admin, "nope", &AbsentQuery::default()).unwrap_err().0, StatusCode::NOT_FOUND);
        // the class is still listed to the graders when the subject, or its institution, is switched off
        w.conn.execute("UPDATE subjects SET is_active = 0", []).unwrap();
        assert_eq!(absent(&w, &w.admin, AbsentQuery::default()).total, 2);
        w.conn.execute("UPDATE subjects SET is_active = 1", []).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        assert_eq!(absent(&w, &approved, AbsentQuery::default()).total, 2);
    }

    #[test]
    fn the_sheet_tells_the_grader_whether_students_will_see_the_comments() {
        let w = world(1);
        let sheet = |w: &W| json_of(&grading_sheet(&w.conn, &w.admin, &w.exam, false).unwrap());
        assert_eq!(sheet(&w)["show_answers"], json!(true), "the default: students review their answers, comments included");
        w.conn.execute("UPDATE assessments SET show_answers = 0 WHERE id = ?1", params![w.exam]).unwrap();
        assert_eq!(sheet(&w)["show_answers"], json!(false), "an exam that hides its answers hides the comments with them");
    }

    #[test]
    fn a_teacher_whose_assignment_was_taken_away_keeps_the_results_but_not_the_class_list() {
        let w = world(4);
        names(&w);
        let owner = approve(&w, "owner@x.com", "s1");
        let own = teacher_exam(&w, &owner, "s1", "امتحان قديم");
        sit_exam(&w, 0, &own, Some("كتبت"), NOW);
        let list = |who: &User| absent_students(&w.conn, who, &own, &AbsentQuery::default());
        assert_eq!(list(&owner).unwrap().total, 3, "approved: the three who never sat it");
        // the admin takes the subject away (approved -> rejected; a request that is pending again is the same)
        for status in ["rejected", "pending"] {
            w.conn.execute("UPDATE teacher_subjects SET status = ?1 WHERE teacher_id = ?2", params![status, owner.id]).unwrap();
            let e = list(&owner).unwrap_err();
            assert_eq!((e.0, e.1.contains("forbidden")), (StatusCode::FORBIDDEN, true), "{status}: the roster is the class as it is today");
            // what the teacher already owns stays readable
            assert!(results_view(&w.conn, &owner, &own, &TableQuery::default(), NOW + 99 * MIN).is_ok(), "{status}: their results");
            assert!(analytics(&w.conn, &owner, &own, NOW + 99 * MIN).is_ok(), "{status}: their analytics");
            // and nobody else gains anything: the admin still sees the class
            assert_eq!(list(&w.admin).unwrap().total, 3);
        }
        // approved again -> the list is back; a switched-off subject still works for an approved teacher (checked elsewhere)
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1", params![owner.id]).unwrap();
        assert_eq!(list(&owner).unwrap().total, 3);
        // the check is about the exam's subject: an approval for another subject does not open this class
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected' WHERE teacher_id = ?1 AND subject_id = 's1'", params![owner.id]).unwrap();
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s2','approved',0)", params![owner.id]).unwrap();
        assert_eq!(list(&owner).unwrap_err().0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn the_absent_list_pages_and_searches_without_treating_wildcards_as_patterns() {
        let w = world(8);
        for (i, s) in w.students.iter().enumerate() {
            let n = match i {
                0 => "100% صفر".to_string(),
                1 => "عمر_ا".to_string(),
                2 => "Alice".to_string(),
                _ => format!("طالب {i}"),
            };
            w.conn.execute("UPDATE users SET full_name = ?2 WHERE id = ?1", params![s.id, n]).unwrap();
        }
        let f = |q: &str| absent(&w, &w.admin, AbsentQuery { q: Some(q.into()), ..Default::default() });
        assert_eq!((f("%").total, f("_").total, f("ALICE").total, f("alice").total, f("  alice  ").total, f("طالب").total, f("zzz").total), (1, 1, 1, 1, 1, 5, 0));
        assert_eq!(f("100%").items[0].student_name, "100% صفر");
        assert_eq!(f("%").enrolled, 8, "the class size ignores the search");
        assert_eq!(f("").total, 8, "a blank search is no search");
        // paging
        let page = |limit: i64, offset: i64| absent(&w, &w.admin, AbsentQuery { q: None, limit: Some(limit), offset: Some(offset) });
        let mut ids: Vec<String> = vec![];
        for off in [0, 3, 6] {
            let p = page(3, off);
            assert_eq!(p.total, 8);
            ids.extend(p.items.iter().map(|s| s.student_id.clone()));
        }
        assert_eq!((ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len()), (8, 8));
        assert_eq!((page(0, 0).items.len(), page(1000, 0).items.len(), page(5, -3).items.len(), page(5, 100).items.len()), (1, 8, 5, 0));
        assert_eq!(absent(&w, &w.admin, AbsentQuery::default()).items.len(), 8, "default limit 25 holds the whole class here");
        // more than a default page
        let big = world(30);
        let p = absent(&big, &big.admin, AbsentQuery::default());
        assert_eq!((p.items.len(), p.total, p.enrolled), (25, 30, 30));
        assert_eq!(absent(&big, &big.admin, AbsentQuery { offset: Some(25), ..Default::default() }).items.len(), 5);
    }
}
