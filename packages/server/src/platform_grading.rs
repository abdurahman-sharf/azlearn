//! Grading and results (phase 1-7): the pending-grading queue and quick grading sheet, per-exam
//! analytics, the students table and XLSX/CSV export.
//!
//! Who may use it: any admin; the owning teacher of a teacher's exam; for an admin-created exam any
//! active teacher approved for its subject (`platform_exams::can_grade`). Students never reach any of it.

use crate::platform::{audit, bad, db_err, lock, require_active, Res, User};
use crate::platform_exams::{can_grade, get_info, load_questions, passed_flag, points_of, round2, settle_assessment, AssessmentInfo, GradeReq, Outcome};
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
    subject_name: String,
    pending_attempts: i64,
    pending_answers: i64,
    oldest_submitted_at: Option<i64>,
}

/// Exams with manual grading still to do that this user may grade, oldest waiting first.
pub fn pending_exams(conn: &Connection, user: &User) -> Res<Vec<PendingExam>> {
    if user.role != "admin" && user.role != "teacher" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    conn.prepare(
        "SELECT a.id, a.title, s.name_ar, count(*), sum(t.pending), min(t.submitted_at)
         FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN subjects s ON s.id = a.subject_id
         WHERE t.status = 'submitted' AND t.pending > 0 AND (
               ?1 = 'admin' OR (?1 = 'teacher' AND (a.teacher_id = ?2 OR (a.teacher_id IS NULL AND EXISTS(
                 SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = ?2 AND ts.subject_id = a.subject_id AND ts.status = 'approved')))))
         GROUP BY a.id ORDER BY min(t.submitted_at), a.id LIMIT 200",
    )
    .map_err(db_err)?
    .query_map(params![user.role, user.id], |r| {
        Ok(PendingExam { assessment_id: r.get(0)?, title: r.get(1)?, subject_name: r.get(2)?, pending_attempts: r.get(3)?, pending_answers: r.get(4)?, oldest_submitted_at: r.get(5)? })
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
            let graded = outcomes.iter().find(|o| o.id == q.id).filter(|o| o.correct.is_some()).map(|o| o.points);
            if graded.is_none() {
                q.pending += 1;
            }
            if only_pending && graded.is_some() {
                continue;
            }
            q.answers.push(GradingAnswer { attempt_id: attempt_id.clone(), student_name: name.clone(), answer: text.clone(), points: graded, submitted_at });
        }
    }
    Ok(GradingSheet { assessment_id: id.to_string(), title: info.title, subject_name: info.subject_name, questions: out })
}

#[derive(Deserialize)]
pub struct BatchItem {
    attempt_id: String,
    question_id: String,
    points: f64,
}

#[derive(Deserialize)]
pub struct BatchReq {
    grades: Vec<BatchItem>,
}

#[derive(Serialize, Debug)]
pub struct BatchResult {
    updated: usize,
}

/// Grades many answers in one transaction: either every grade is stored or none is.
pub fn grade_batch(conn: &Connection, user: &User, id: &str, r: &BatchReq) -> Res<BatchResult> {
    require_grader(conn, user, id)?;
    if r.grades.is_empty() || r.grades.len() > MAX_BATCH {
        return Err(bad("invalid_selection"));
    }
    let mut by_attempt: HashMap<&str, HashMap<String, f64>> = HashMap::new();
    for g in &r.grades {
        if by_attempt.entry(g.attempt_id.as_str()).or_default().insert(g.question_id.clone(), g.points).is_some() {
            return Err(bad("invalid_selection")); // the same answer twice in one request
        }
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
    let result = (|| -> Res<()> {
        for (attempt_id, grades) in &by_attempt {
            let belongs: bool = conn
                .query_row("SELECT EXISTS(SELECT 1 FROM attempts WHERE id = ?1 AND assessment_id = ?2)", params![attempt_id, id], |x| x.get(0))
                .map_err(db_err)?;
            if !belongs {
                return Err(err(StatusCode::NOT_FOUND, "not_found"));
            }
            crate::platform_exams::grade_attempt(conn, user, attempt_id, &GradeReq { grades: grades.clone() })?;
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

pub async fn pending_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Vec<PendingExam>>> {
    let u = require_active(&s, &h)?;
    pending_exams(&*lock(&s)?, &u).map(Json)
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

pub async fn table_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Query(q): Query<TableQuery>) -> Res<Json<TablePage>> {
    let u = require_active(&s, &h)?;
    attempts_table(&*lock(&s)?, &u, &id, &q, now_ms(), false).map(Json)
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
        crate::platform_exams::grade_attempt(&w.conn, &w.admin, attempt, &GradeReq { grades: [("s1".to_string(), pts)].into() }).unwrap();
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
            grade_batch(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![BatchItem { attempt_id: a.attempt_id.clone(), question_id: "s1".into(), points: 2.0 }] }).unwrap();
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
        let item = |att: &str, q: &str, p: f64| BatchItem { attempt_id: att.into(), question_id: q.into(), points: p };
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
        let r = grade_batch(&w.conn, &w.admin, &w.exam, &BatchReq { grades: vec![BatchItem { attempt_id: a, question_id: "s1".into(), points: 5.0 }] });
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
}
