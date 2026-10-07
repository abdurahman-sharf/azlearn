//! Graded assessments inside the platform: a teacher publishes a question bank (generated with
//! Exameow's generator) for a subject; enrolled students take it with a server-enforced timer and
//! attempt limit; objective questions are auto-graded (same rules as the exam relay, plus Arabic
//! normalization) and short answers are graded by the teacher. Answers never leave the server
//! before submission.

use crate::platform::{audit, bad, db_err, lock, new_id, opt_text, require_active, require_role, text, Res, User};
use crate::relay::{err, grade, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use exameow_core::exam::{Question, QuestionType};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS attempts (
  id TEXT PRIMARY KEY,
  assessment_id TEXT NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  started_at INTEGER NOT NULL,
  submitted_at INTEGER,
  status TEXT NOT NULL CHECK (status IN ('in_progress','submitted','expired')),
  answers TEXT NOT NULL DEFAULT '{}',
  results TEXT NOT NULL DEFAULT '[]',
  score REAL NOT NULL DEFAULT 0,
  pending INTEGER NOT NULL DEFAULT 0,
  saved_at INTEGER,
  tab_leaves INTEGER NOT NULL DEFAULT 0,
  order_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_attempts_assess ON attempts(assessment_id, student_id);
CREATE INDEX IF NOT EXISTS idx_attempts_student ON attempts(student_id, started_at);
";


/// Final `assessments` definition (phase 1-0). `teacher_id` is nullable (admin-created exams, phase 1-5),
/// `created_by` records the creator, and `status` gained the `closed`/`archived` lifecycle states.
/// SQLite cannot alter NOT NULL or CHECK constraints in place, so older databases are rebuilt (see `rebuild`).
fn assessments_ddl(table: &str) -> String {
    format!(
        "CREATE TABLE {table} (
  id TEXT PRIMARY KEY,
  teacher_id TEXT REFERENCES users(id) ON DELETE CASCADE,
  created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT,
  questions TEXT NOT NULL,
  question_count INTEGER NOT NULL,
  total_points REAL NOT NULL,
  duration_min INTEGER,
  opens_at INTEGER,
  closes_at INTEGER,
  max_attempts INTEGER NOT NULL DEFAULT 1,
  show_answers INTEGER NOT NULL DEFAULT 1,
  shuffle_questions INTEGER NOT NULL DEFAULT 0,
  shuffle_options INTEGER NOT NULL DEFAULT 0,
  pass_mark REAL,
  release_mode TEXT NOT NULL DEFAULT 'immediate' CHECK (release_mode IN ('immediate','after_close')),
  status TEXT NOT NULL CHECK (status IN ('draft','published','closed','archived')),
  closed_at INTEGER,
  archived_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
)"
    )
}

/// Columns shared by the old and new tables, copied verbatim on rebuild.
const CARRIED_COLUMNS: &str = "id, teacher_id, subject_id, title, description, questions, question_count, total_points, \
     duration_min, opens_at, closes_at, max_attempts, show_answers, status, created_at, updated_at";

/// Rebuilds `assessments` atomically: new table -> copy -> drop old -> rename.
/// Foreign keys are switched off for the swap (otherwise DROP would cascade-delete every attempt) and
/// verified with `foreign_key_check` before commit; any failure rolls everything back untouched.
fn rebuild(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA foreign_keys = OFF;").map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let e = |x: rusqlite::Error| x.to_string();
        tx.execute_batch("DROP TABLE IF EXISTS assessments_new;").map_err(e)?;
        tx.execute_batch(&assessments_ddl("assessments_new")).map_err(e)?;
        tx.execute_batch(&format!(
            "INSERT INTO assessments_new ({CARRIED_COLUMNS}, created_by)
             SELECT {CARRIED_COLUMNS}, teacher_id FROM assessments;
             DROP TABLE assessments;
             ALTER TABLE assessments_new RENAME TO assessments;"
        ))
        .map_err(e)?;
        let violations: i64 = tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).map_err(e)?;
        if violations > 0 {
            return Err(format!("assessments migration aborted: {violations} foreign key violation(s)"));
        }
        tx.commit().map_err(e)
    })();
    conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(|e| e.to_string())?;
    result
}

/// Creates or upgrades the exam tables. Idempotent: safe to run on every start.
pub fn migrate(conn: &Connection) -> Result<(), String> {
    let exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'assessments')", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !exists {
        conn.execute_batch(&assessments_ddl("assessments")).map_err(|e| e.to_string())?;
    } else if !crate::platform::column_exists(conn, "assessments", "created_by")? {
        rebuild(conn)?;
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_assess_subject ON assessments(subject_id, status);")
        .map_err(|e| e.to_string())?;
    // Older `attempts` tables predate autosave / integrity tracking.
    crate::platform::add_column_if_missing(conn, "attempts", "saved_at", "INTEGER")?;
    crate::platform::add_column_if_missing(conn, "attempts", "tab_leaves", "INTEGER NOT NULL DEFAULT 0")?;
    crate::platform::add_column_if_missing(conn, "attempts", "order_json", "TEXT")?;
    Ok(())
}

const MAX_QUESTIONS: usize = 200;
const GRACE_MS: i64 = 60_000;
const OPEN_ENDED_LIMIT_MS: i64 = 24 * 3_600_000;

// ───────── grading ─────────

/// Arabic-aware comparison form: strips diacritics/tatweel, unifies alef/yeh variants, lowercases.
fn norm_text(s: &str) -> String {
    s.trim()
        .chars()
        .filter(|c| !matches!(*c as u32, 0x064B..=0x065F | 0x0670 | 0x0640))
        .map(|c| match c {
            'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
            'ى' => 'ي',
            c => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// Maps Arabic true/false words onto the relay's A/B convention.
fn canon_tf(s: &str) -> String {
    match norm_text(s).as_str() {
        "صح" | "صحيح" | "نعم" => "A".into(),
        "خطا" | "خاطئ" | "لا" | "غير صحيح" | "خطاء" => "B".into(),
        _ => s.to_string(),
    }
}

/// Some(true/false) for objective questions, None for short answers (teacher grades those).
fn auto_grade(q: &Question, user: Option<&str>) -> Option<bool> {
    match q.qtype {
        QuestionType::ShortAnswer => None,
        QuestionType::TrueFalse => {
            let mut c = q.clone();
            c.answer = canon_tf(&q.answer);
            grade(&c, user.map(canon_tf).as_deref())
        }
        QuestionType::FillBlank => {
            let u = norm_text(user?);
            if u.is_empty() {
                return Some(false);
            }
            Some(q.answer.split('|').any(|alt| norm_text(alt) == u))
        }
        _ => grade(q, user),
    }
}

fn points_of(q: &Question) -> f64 {
    q.score.filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(1.0)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Outcome {
    id: String,
    correct: Option<bool>,
    points: f64,
    max: f64,
}

fn grade_all(questions: &[Question], answers: &HashMap<String, String>) -> (Vec<Outcome>, f64, i64) {
    let mut outcomes = Vec::with_capacity(questions.len());
    let (mut score, mut pending) = (0.0, 0);
    for q in questions {
        let max = points_of(q);
        let user = answers.get(&q.id).map(String::as_str);
        let (correct, points) = match auto_grade(q, user) {
            Some(true) => (Some(true), max),
            Some(false) => (Some(false), 0.0),
            None => {
                // short answer: pending only if the student wrote something
                if user.map_or(false, |u| !u.trim().is_empty()) {
                    pending += 1;
                    (None, 0.0)
                } else {
                    (Some(false), 0.0)
                }
            }
        };
        score += points;
        outcomes.push(Outcome { id: q.id.clone(), correct, points, max });
    }
    (outcomes, round2(score), pending)
}

// ───────── validation ─────────

fn validate_questions(qs: &[Question]) -> Res<()> {
    if qs.is_empty() || qs.len() > MAX_QUESTIONS {
        return Err(bad("invalid_question_count"));
    }
    let mut seen = HashSet::new();
    for q in qs {
        let id_ok = !q.id.trim().is_empty() && q.id.chars().count() <= 100;
        if !id_ok || !seen.insert(q.id.clone()) {
            return Err(bad("invalid_question_id"));
        }
        if q.stem.trim().is_empty() || q.stem.chars().count() > 3000 || q.answer.trim().is_empty() || q.answer.chars().count() > 2000 {
            return Err(bad("invalid_question"));
        }
        if q.analysis.chars().count() > 6000 || q.options.len() > 10 || q.options.iter().any(|o| o.chars().count() > 1000) {
            return Err(bad("invalid_question"));
        }
        let needs_options = matches!(q.qtype, QuestionType::SingleChoice | QuestionType::MultiChoice);
        if needs_options && q.options.len() < 2 {
            return Err(bad("invalid_question"));
        }
        if q.score.map_or(false, |s| !s.is_finite() || !(0.0..=100.0).contains(&s)) {
            return Err(bad("invalid_question"));
        }
    }
    Ok(())
}

fn check_window(opens: Option<i64>, closes: Option<i64>) -> Res<()> {
    if let (Some(o), Some(c)) = (opens, closes) {
        if c <= o {
            return Err(bad("invalid_time"));
        }
    }
    Ok(())
}

// ───────── data types ─────────

#[derive(Serialize, Debug, Clone)]
pub struct AssessmentInfo {
    id: String,
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    title: String,
    description: Option<String>,
    question_count: i64,
    total_points: f64,
    duration_min: Option<i64>,
    opens_at: Option<i64>,
    closes_at: Option<i64>,
    max_attempts: i64,
    show_answers: bool,
    status: String,
    attempts_used: i64,
    attempt_count: i64,
}

const INFO_SELECT: &str = "SELECT a.id, a.teacher_id, u.full_name, a.subject_id, s.name_ar, a.title, a.description, a.question_count,
        a.total_points, a.duration_min, a.opens_at, a.closes_at, a.max_attempts, a.show_answers, a.status,
        (SELECT count(*) FROM attempts t WHERE t.assessment_id = a.id AND t.student_id = ?1),
        (SELECT count(*) FROM attempts t WHERE t.assessment_id = a.id)
     FROM assessments a JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id";

fn map_info(r: &rusqlite::Row) -> rusqlite::Result<AssessmentInfo> {
    Ok(AssessmentInfo {
        id: r.get(0)?,
        teacher_id: r.get(1)?,
        teacher_name: r.get(2)?,
        subject_id: r.get(3)?,
        subject_name: r.get(4)?,
        title: r.get(5)?,
        description: r.get(6)?,
        question_count: r.get(7)?,
        total_points: r.get(8)?,
        duration_min: r.get(9)?,
        opens_at: r.get(10)?,
        closes_at: r.get(11)?,
        max_attempts: r.get(12)?,
        show_answers: r.get(13)?,
        status: r.get(14)?,
        attempts_used: r.get(15)?,
        attempt_count: r.get(16)?,
    })
}

fn get_info(conn: &Connection, viewer_id: &str, id: &str) -> Res<AssessmentInfo> {
    conn.query_row(&format!("{INFO_SELECT} WHERE a.id = ?2"), params![viewer_id, id], map_info)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

/// Published, teacher active and approved for the subject, subject active.
fn publicly_visible(conn: &Connection, id: &str) -> Res<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM assessments a JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
          WHERE a.id = ?1 AND a.status = 'published' AND u.status = 'active' AND s.is_active = 1
            AND EXISTS(SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = a.teacher_id AND ts.subject_id = a.subject_id AND ts.status = 'approved'))",
        params![id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

fn is_enrolled(conn: &Connection, student_id: &str, subject_id: &str) -> Res<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM subject_enrollments WHERE student_id = ?1 AND subject_id = ?2)",
        params![student_id, subject_id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

fn load_questions(conn: &Connection, id: &str) -> Res<Vec<Question>> {
    let raw: String = conn.query_row("SELECT questions FROM assessments WHERE id = ?1", params![id], |r| r.get(0)).map_err(db_err)?;
    serde_json::from_str(&raw).map_err(db_err)
}

// ───────── teacher: create / update / delete ─────────

#[derive(Deserialize)]
pub struct AssessmentReq {
    subject_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    questions: Option<Vec<Question>>,
    duration_min: Option<i64>,
    opens_at: Option<i64>,
    closes_at: Option<i64>,
    max_attempts: Option<i64>,
    show_answers: Option<bool>,
    status: Option<String>,
    /// Set to true to clear a previously set duration/window.
    clear_duration: Option<bool>,
    clear_window: Option<bool>,
}

fn check_duration(d: Option<i64>) -> Res<()> {
    match d {
        Some(m) if !(1..=480).contains(&m) => Err(bad("invalid_time")),
        _ => Ok(()),
    }
}

fn create_assessment(conn: &Connection, teacher: &User, r: &AssessmentReq) -> Res<AssessmentInfo> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    let assigned: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM teacher_subjects ts JOIN subjects s ON s.id = ts.subject_id
             WHERE ts.teacher_id = ?1 AND ts.subject_id = ?2 AND ts.status = 'approved' AND s.is_active = 1)",
            params![teacher.id, subject_id],
            |x| x.get(0),
        )
        .map_err(db_err)?;
    if !assigned {
        return Err(err(StatusCode::FORBIDDEN, "not_assigned"));
    }
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let questions = r.questions.as_ref().ok_or_else(|| bad("invalid_question_count"))?;
    validate_questions(questions)?;
    check_duration(r.duration_min)?;
    check_window(r.opens_at, r.closes_at)?;
    let max_attempts = r.max_attempts.unwrap_or(1);
    if !(1..=10).contains(&max_attempts) {
        return Err(bad("invalid_attempts"));
    }
    let status = r.status.as_deref().unwrap_or("draft");
    if !["draft", "published"].contains(&status) {
        return Err(bad("invalid_status"));
    }
    let total: f64 = round2(questions.iter().map(points_of).sum());
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO assessments(id, teacher_id, created_by, subject_id, title, description, questions, question_count, total_points, duration_min,
           opens_at, closes_at, max_attempts, show_answers, status, created_at, updated_at)
         VALUES (?1,?2,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15)",
        params![
            id, teacher.id, subject_id, title, description, serde_json::to_string(questions).map_err(db_err)?,
            questions.len() as i64, total, r.duration_min, r.opens_at, r.closes_at, max_attempts, r.show_answers.unwrap_or(true), status, now
        ],
    )
    .map_err(db_err)?;
    if status == "published" {
        announce(conn, teacher, subject_id, &id, &title);
    }
    get_info(conn, &teacher.id, &id)
}

fn announce(conn: &Connection, teacher: &User, subject_id: &str, id: &str, title: &str) {
    crate::platform_engage::notify_audience(
        conn,
        &teacher.id,
        subject_id,
        "new_assessment",
        json!({ "title": title, "teacher": teacher.full_name }),
        &format!("/platform/assessments/{id}"),
    );
}

fn owner_of(conn: &Connection, id: &str) -> Res<String> {
    conn.query_row("SELECT teacher_id FROM assessments WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn update_assessment(conn: &Connection, user: &User, id: &str, r: &AssessmentReq) -> Res<AssessmentInfo> {
    let owner = owner_of(conn, id)?;
    let is_owner = owner == user.id;
    if !is_owner && user.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let only_status = r.title.is_none() && r.description.is_none() && r.questions.is_none() && r.duration_min.is_none()
        && r.opens_at.is_none() && r.closes_at.is_none() && r.max_attempts.is_none() && r.show_answers.is_none()
        && r.clear_duration.is_none() && r.clear_window.is_none();
    if !is_owner && !only_status {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let cur = get_info(conn, "", id)?;
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title.clone(),
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { cur.description.clone() };
    let duration = if r.clear_duration == Some(true) { None } else { r.duration_min.or(cur.duration_min) };
    let (opens, closes) = if r.clear_window == Some(true) { (None, None) } else { (r.opens_at.or(cur.opens_at), r.closes_at.or(cur.closes_at)) };
    check_duration(duration)?;
    check_window(opens, closes)?;
    let max_attempts = r.max_attempts.unwrap_or(cur.max_attempts);
    if !(1..=10).contains(&max_attempts) {
        return Err(bad("invalid_attempts"));
    }
    let status = r.status.clone().unwrap_or_else(|| cur.status.clone());
    if !["draft", "published"].contains(&status.as_str()) {
        return Err(bad("invalid_status"));
    }
    if status == "published" && is_owner {
        let ok: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2 AND status = 'approved')",
                params![owner, cur.subject_id],
                |x| x.get(0),
            )
            .map_err(db_err)?;
        if !ok {
            return Err(err(StatusCode::FORBIDDEN, "not_assigned"));
        }
    }
    if let Some(qs) = &r.questions {
        if cur.attempt_count > 0 {
            return Err(err(StatusCode::CONFLICT, "has_attempts")); // never change questions under existing attempts
        }
        validate_questions(qs)?;
        let total: f64 = round2(qs.iter().map(points_of).sum());
        conn.execute(
            "UPDATE assessments SET questions = ?1, question_count = ?2, total_points = ?3 WHERE id = ?4",
            params![serde_json::to_string(qs).map_err(db_err)?, qs.len() as i64, total, id],
        )
        .map_err(db_err)?;
    }
    conn.execute(
        "UPDATE assessments SET title=?1, description=?2, duration_min=?3, opens_at=?4, closes_at=?5, max_attempts=?6, show_answers=?7, status=?8, updated_at=?9 WHERE id=?10",
        params![title, description, duration, opens, closes, max_attempts, r.show_answers.unwrap_or(cur.show_answers), status, now_ms(), id],
    )
    .map_err(db_err)?;
    if !is_owner {
        audit(conn, &user.id, id, "assessment_moderated", &status);
        crate::platform_engage::notify(conn, &owner, "content_unpublished", json!({ "title": title }), &format!("/platform/assessments/{id}"));
    } else if status == "published" && cur.status != "published" {
        announce(conn, user, &cur.subject_id, id, &title);
    }
    get_info(conn, &user.id, id)
}

fn delete_assessment(conn: &Connection, user: &User, id: &str) -> Res<()> {
    let owner = owner_of(conn, id)?;
    if owner != user.id && user.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    conn.execute("DELETE FROM assessments WHERE id = ?1", params![id]).map_err(db_err)?;
    audit(conn, &user.id, id, "assessment_deleted", "");
    Ok(())
}

// ───────── student: start / submit ─────────

#[derive(Serialize, Debug)]
pub struct PublicQuestion {
    id: String,
    #[serde(rename = "type")]
    qtype: QuestionType,
    stem: String,
    options: Vec<String>,
    points: f64,
}

#[derive(Serialize, Debug)]
pub struct StartRes {
    attempt_id: String,
    started_at: i64,
    /// Server-enforced deadline (ms epoch).
    ends_at: i64,
    questions: Vec<PublicQuestion>,
    resumed: bool,
}

fn deadline(started_at: i64, duration_min: Option<i64>) -> i64 {
    started_at + duration_min.map_or(OPEN_ENDED_LIMIT_MS, |m| m * 60_000)
}

/// Marks in-progress attempts past their deadline (+grace) as expired with score 0.
fn expire_stale(conn: &Connection, assessment_id: &str, student_id: &str, duration_min: Option<i64>, now: i64) -> Res<()> {
    let stale: Vec<(String, i64)> = conn
        .prepare("SELECT id, started_at FROM attempts WHERE assessment_id = ?1 AND student_id = ?2 AND status = 'in_progress'")
        .map_err(db_err)?
        .query_map(params![assessment_id, student_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    for (id, started) in stale {
        if now > deadline(started, duration_min) + GRACE_MS {
            conn.execute("UPDATE attempts SET status = 'expired' WHERE id = ?1", params![id]).map_err(db_err)?;
        }
    }
    Ok(())
}

fn start_attempt(conn: &Connection, student: &User, assessment_id: &str, now: i64) -> Res<StartRes> {
    if !publicly_visible(conn, assessment_id)? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let info = get_info(conn, &student.id, assessment_id)?;
    if !is_enrolled(conn, &student.id, &info.subject_id)? {
        return Err(err(StatusCode::FORBIDDEN, "not_enrolled"));
    }
    expire_stale(conn, assessment_id, &student.id, info.duration_min, now)?;
    let questions = load_questions(conn, assessment_id)?;
    let public = |qs: &[Question]| -> Vec<PublicQuestion> {
        qs.iter()
            .map(|q| PublicQuestion { id: q.id.clone(), qtype: q.qtype.clone(), stem: q.stem.clone(), options: q.options.clone(), points: points_of(q) })
            .collect()
    };
    // Resume an in-progress attempt instead of burning another one.
    let open: Option<(String, i64)> = conn
        .query_row(
            "SELECT id, started_at FROM attempts WHERE assessment_id = ?1 AND student_id = ?2 AND status = 'in_progress'",
            params![assessment_id, student.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;
    if let Some((id, started)) = open {
        return Ok(StartRes { attempt_id: id, started_at: started, ends_at: deadline(started, info.duration_min), questions: public(&questions), resumed: true });
    }
    if info.opens_at.map_or(false, |o| now < o) {
        return Err(err(StatusCode::FORBIDDEN, "not_open_yet"));
    }
    if info.closes_at.map_or(false, |c| now > c) {
        return Err(err(StatusCode::FORBIDDEN, "closed"));
    }
    let used: i64 = conn
        .query_row("SELECT count(*) FROM attempts WHERE assessment_id = ?1 AND student_id = ?2", params![assessment_id, student.id], |r| r.get(0))
        .map_err(db_err)?;
    if used >= info.max_attempts {
        return Err(err(StatusCode::FORBIDDEN, "no_attempts_left"));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO attempts(id, assessment_id, student_id, started_at, status) VALUES (?1,?2,?3,?4,'in_progress')",
        params![id, assessment_id, student.id, now],
    )
    .map_err(db_err)?;
    Ok(StartRes { attempt_id: id, started_at: now, ends_at: deadline(now, info.duration_min), questions: public(&questions), resumed: false })
}

#[derive(Serialize, Debug)]
pub struct ItemResult {
    id: String,
    #[serde(rename = "type")]
    qtype: QuestionType,
    stem: String,
    options: Vec<String>,
    your_answer: Option<String>,
    correct: Option<bool>,
    points: f64,
    max: f64,
    correct_answer: Option<String>,
    analysis: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct AttemptResult {
    attempt_id: String,
    assessment_id: String,
    title: String,
    student_name: String,
    status: String,
    score: f64,
    total: f64,
    pending: i64,
    started_at: i64,
    submitted_at: Option<i64>,
    show_answers: bool,
    items: Option<Vec<ItemResult>>,
}

fn clean_answers(raw: &HashMap<String, String>, questions: &[Question]) -> HashMap<String, String> {
    let ids: HashSet<&str> = questions.iter().map(|q| q.id.as_str()).collect();
    raw.iter()
        .filter(|(k, _)| ids.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.chars().take(2000).collect()))
        .collect()
}

fn submit_attempt(conn: &Connection, student: &User, attempt_id: &str, raw: &HashMap<String, String>, now: i64) -> Res<AttemptResult> {
    let (assessment_id, started, status): (String, i64, String) = conn
        .query_row(
            "SELECT assessment_id, started_at, status FROM attempts WHERE id = ?1 AND student_id = ?2",
            params![attempt_id, student.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if status != "in_progress" {
        return Err(err(StatusCode::CONFLICT, if status == "expired" { "time_expired" } else { "already_submitted" }));
    }
    let info = get_info(conn, &student.id, &assessment_id)?;
    if now > deadline(started, info.duration_min) + GRACE_MS {
        conn.execute("UPDATE attempts SET status = 'expired' WHERE id = ?1", params![attempt_id]).map_err(db_err)?;
        return Err(err(StatusCode::CONFLICT, "time_expired"));
    }
    let questions = load_questions(conn, &assessment_id)?;
    let answers = clean_answers(raw, &questions);
    let (outcomes, score, pending) = grade_all(&questions, &answers);
    conn.execute(
        "UPDATE attempts SET status = 'submitted', submitted_at = ?1, answers = ?2, results = ?3, score = ?4, pending = ?5 WHERE id = ?6",
        params![now, serde_json::to_string(&answers).map_err(db_err)?, serde_json::to_string(&outcomes).map_err(db_err)?, score, pending, attempt_id],
    )
    .map_err(db_err)?;
    attempt_result(conn, student, attempt_id)
}

/// Builds the result view. Students only see per-question detail when the assessment allows it;
/// the owner/admin always see everything.
fn attempt_result(conn: &Connection, viewer: &User, attempt_id: &str) -> Res<AttemptResult> {
    let row = conn
        .query_row(
            "SELECT t.assessment_id, t.student_id, t.status, t.score, t.pending, t.started_at, t.submitted_at, t.answers, t.results,
                    a.title, a.total_points, a.show_answers, a.teacher_id, u.full_name
             FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN users u ON u.id = t.student_id WHERE t.id = ?1",
            params![attempt_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?, r.get::<_, Option<i64>>(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?, r.get::<_, f64>(10)?, r.get::<_, bool>(11)?, r.get::<_, String>(12)?, r.get::<_, String>(13)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let (assessment_id, student_id, status, score, pending, started_at, submitted_at, answers_raw, results_raw, title, total, show, teacher_id, student_name) = row;
    let is_owner = teacher_id == viewer.id || viewer.role == "admin";
    if student_id != viewer.id && !is_owner {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    if status == "in_progress" {
        return Err(err(StatusCode::CONFLICT, "in_progress"));
    }
    let detail = is_owner || show;
    let items = if detail && status == "submitted" {
        let questions = load_questions(conn, &assessment_id)?;
        let answers: HashMap<String, String> = serde_json::from_str(&answers_raw).unwrap_or_default();
        let outcomes: Vec<Outcome> = serde_json::from_str(&results_raw).unwrap_or_default();
        let by_id: HashMap<&str, &Outcome> = outcomes.iter().map(|o| (o.id.as_str(), o)).collect();
        Some(
            questions
                .iter()
                .map(|q| {
                    let o = by_id.get(q.id.as_str());
                    ItemResult {
                        id: q.id.clone(),
                        qtype: q.qtype.clone(),
                        stem: q.stem.clone(),
                        options: q.options.clone(),
                        your_answer: answers.get(&q.id).cloned(),
                        correct: o.and_then(|o| o.correct),
                        points: o.map_or(0.0, |o| o.points),
                        max: points_of(q),
                        correct_answer: Some(q.answer.clone()),
                        analysis: (!q.analysis.is_empty()).then(|| q.analysis.clone()),
                    }
                })
                .collect(),
        )
    } else {
        None
    };
    Ok(AttemptResult {
        attempt_id: attempt_id.to_string(),
        assessment_id,
        title,
        student_name: if is_owner { student_name } else { String::new() },
        status,
        score,
        total,
        pending,
        started_at,
        submitted_at,
        show_answers: show,
        items,
    })
}

// ───────── teacher: grading & results ─────────

#[derive(Deserialize)]
pub struct GradeReq {
    grades: HashMap<String, f64>,
}

fn grade_attempt(conn: &Connection, teacher: &User, attempt_id: &str, g: &GradeReq) -> Res<AttemptResult> {
    let (assessment_id, student_id, status, results_raw): (String, String, String, String) = conn
        .query_row("SELECT assessment_id, student_id, status, results FROM attempts WHERE id = ?1", params![attempt_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if owner_of(conn, &assessment_id)? != teacher.id {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    if status != "submitted" {
        return Err(err(StatusCode::CONFLICT, "not_submitted"));
    }
    let mut outcomes: Vec<Outcome> = serde_json::from_str(&results_raw).map_err(db_err)?;
    for (qid, pts) in &g.grades {
        let o = outcomes.iter_mut().find(|o| &o.id == qid).ok_or_else(|| bad("invalid_question_id"))?;
        if o.correct.is_some() {
            return Err(bad("not_gradable")); // only pending short answers can be graded by hand
        }
        if !pts.is_finite() || *pts < 0.0 || *pts > o.max {
            return Err(bad("invalid_points"));
        }
        o.points = round2(*pts);
        o.correct = Some(*pts >= o.max);
    }
    let score = round2(outcomes.iter().map(|o| o.points).sum());
    let pending = outcomes.iter().filter(|o| o.correct.is_none()).count() as i64;
    conn.execute(
        "UPDATE attempts SET results = ?1, score = ?2, pending = ?3 WHERE id = ?4",
        params![serde_json::to_string(&outcomes).map_err(db_err)?, score, pending, attempt_id],
    )
    .map_err(db_err)?;
    if pending == 0 && !g.grades.is_empty() {
        let title: String = conn.query_row("SELECT title FROM assessments WHERE id = ?1", params![assessment_id], |r| r.get(0)).map_err(db_err)?;
        crate::platform_engage::notify(conn, &student_id, "assessment_graded", json!({ "title": title }), &format!("/platform/attempts/{attempt_id}"));
    }
    attempt_result(conn, teacher, attempt_id)
}

#[derive(Serialize, Debug)]
pub struct AttemptRow {
    attempt_id: String,
    student_name: String,
    status: String,
    score: f64,
    pending: i64,
    submitted_at: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct ResultsSummary {
    info: AssessmentInfo,
    submitted: i64,
    average: f64,
    highest: f64,
    lowest: f64,
    attempts: Vec<AttemptRow>,
}

fn results_summary(conn: &Connection, user: &User, id: &str) -> Res<ResultsSummary> {
    let owner = owner_of(conn, id)?;
    if owner != user.id && user.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let info = get_info(conn, &user.id, id)?;
    let attempts = conn
        .prepare(
            "SELECT t.id, u.full_name, t.status, t.score, t.pending, t.submitted_at FROM attempts t JOIN users u ON u.id = t.student_id
             WHERE t.assessment_id = ?1 AND t.status <> 'in_progress' ORDER BY t.submitted_at DESC, t.started_at DESC LIMIT 500",
        )
        .map_err(db_err)?
        .query_map(params![id], |r| {
            Ok(AttemptRow { attempt_id: r.get(0)?, student_name: r.get(1)?, status: r.get(2)?, score: r.get(3)?, pending: r.get(4)?, submitted_at: r.get(5)? })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let scores: Vec<f64> = attempts.iter().filter(|a| a.status == "submitted").map(|a| a.score).collect();
    let n = scores.len();
    Ok(ResultsSummary {
        info,
        submitted: n as i64,
        average: if n == 0 { 0.0 } else { round2(scores.iter().sum::<f64>() / n as f64) },
        highest: scores.iter().cloned().fold(0.0, f64::max),
        lowest: if n == 0 { 0.0 } else { scores.iter().cloned().fold(f64::MAX, f64::min) },
        attempts,
    })
}

// ───────── lists ─────────

fn list_for_subject(conn: &Connection, student_id: &str, subject_id: &str) -> Res<Vec<AssessmentInfo>> {
    conn.prepare(&format!(
        "{INFO_SELECT} WHERE a.subject_id = ?2 AND a.status = 'published' AND u.status = 'active' AND s.is_active = 1
           AND EXISTS(SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = a.teacher_id AND ts.subject_id = a.subject_id AND ts.status = 'approved')
         ORDER BY a.updated_at DESC LIMIT 50"
    ))
    .map_err(db_err)?
    .query_map(params![student_id, subject_id], map_info)
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

fn list_mine(conn: &Connection, teacher_id: &str) -> Res<Vec<AssessmentInfo>> {
    conn.prepare(&format!("{INFO_SELECT} WHERE a.teacher_id = ?1 ORDER BY a.updated_at DESC LIMIT 100"))
        .map_err(db_err)?
        .query_map(params![teacher_id], map_info)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

/// Published assessments of the student's enrolled subjects.
fn list_available(conn: &Connection, student_id: &str) -> Res<Vec<AssessmentInfo>> {
    conn.prepare(&format!(
        "{INFO_SELECT} WHERE a.status = 'published' AND u.status = 'active' AND s.is_active = 1
           AND EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?1 AND e.subject_id = a.subject_id)
           AND EXISTS(SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = a.teacher_id AND ts.subject_id = a.subject_id AND ts.status = 'approved')
         ORDER BY a.updated_at DESC LIMIT 30"
    ))
    .map_err(db_err)?
    .query_map(params![student_id], map_info)
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

#[derive(Serialize, Debug)]
pub struct MyAttempt {
    attempt_id: String,
    assessment_id: String,
    title: String,
    status: String,
    score: f64,
    total: f64,
    pending: i64,
    started_at: i64,
}

fn my_attempts(conn: &Connection, student_id: &str, assessment_id: Option<&str>) -> Res<Vec<MyAttempt>> {
    conn.prepare(
        "SELECT t.id, a.id, a.title, t.status, t.score, a.total_points, t.pending, t.started_at FROM attempts t
         JOIN assessments a ON a.id = t.assessment_id WHERE t.student_id = ?1 AND (?2 IS NULL OR a.id = ?2) ORDER BY t.started_at DESC LIMIT 50",
    )
    .map_err(db_err)?
    .query_map(params![student_id, assessment_id], |r| {
        Ok(MyAttempt { attempt_id: r.get(0)?, assessment_id: r.get(1)?, title: r.get(2)?, status: r.get(3)?, score: r.get(4)?, total: r.get(5)?, pending: r.get(6)?, started_at: r.get(7)? })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

// ───────── handlers ─────────

pub async fn create_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<AssessmentReq>) -> Res<(StatusCode, Json<AssessmentInfo>)> {
    let u = require_role(&s, &h, "teacher")?;
    create_assessment(&*lock(&s)?, &u, &r).map(|i| (StatusCode::CREATED, Json(i)))
}

pub async fn update_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<AssessmentReq>) -> Res<Json<AssessmentInfo>> {
    let u = require_active(&s, &h)?;
    update_assessment(&*lock(&s)?, &u, &id, &r).map(Json)
}

pub async fn delete_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let u = require_active(&s, &h)?;
    delete_assessment(&*lock(&s)?, &u, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct AssessmentDetail {
    #[serde(flatten)]
    info: AssessmentInfo,
    can_start: bool,
    in_progress_attempt: Option<String>,
    attempts: Vec<MyAttempt>,
}

pub async fn detail_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<AssessmentDetail>> {
    let u = require_active(&s, &h)?;
    let conn = lock(&s)?;
    let info = get_info(&conn, &u.id, &id)?;
    let is_owner = info.teacher_id == u.id || u.role == "admin";
    if !is_owner && !publicly_visible(&conn, &id)? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let now = now_ms();
    let mut can_start = false;
    let mut open_attempt = None;
    let mut attempts = Vec::new();
    if u.role == "student" {
        expire_stale(&conn, &id, &u.id, info.duration_min, now)?;
        attempts = my_attempts(&conn, &u.id, Some(&id))?;
        open_attempt = attempts.iter().find(|a| a.status == "in_progress").map(|a| a.attempt_id.clone());
        let used = attempts.len() as i64;
        can_start = is_enrolled(&conn, &u.id, &info.subject_id)?
            && (open_attempt.is_some()
                || (used < info.max_attempts && info.opens_at.map_or(true, |o| now >= o) && info.closes_at.map_or(true, |c| now <= c)));
    }
    Ok(Json(AssessmentDetail { info, can_start, in_progress_attempt: open_attempt, attempts }))
}

pub async fn start_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<StartRes>> {
    let u = require_role(&s, &h, "student")?;
    start_attempt(&*lock(&s)?, &u, &id, now_ms()).map(Json)
}

#[derive(Deserialize)]
pub struct SubmitReq {
    answers: HashMap<String, String>,
}

pub async fn submit_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<SubmitReq>) -> Res<Json<AttemptResult>> {
    let u = require_role(&s, &h, "student")?;
    submit_attempt(&*lock(&s)?, &u, &id, &r.answers, now_ms()).map(Json)
}

pub async fn attempt_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<AttemptResult>> {
    let u = require_active(&s, &h)?;
    attempt_result(&*lock(&s)?, &u, &id).map(Json)
}

pub async fn grade_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(g): Json<GradeReq>) -> Res<Json<AttemptResult>> {
    let u = require_role(&s, &h, "teacher")?;
    grade_attempt(&*lock(&s)?, &u, &id, &g).map(Json)
}

pub async fn results_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<ResultsSummary>> {
    let u = require_active(&s, &h)?;
    results_summary(&*lock(&s)?, &u, &id).map(Json)
}

pub async fn subject_list_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<Vec<AssessmentInfo>>> {
    let u = require_active(&s, &h)?;
    list_for_subject(&*lock(&s)?, &u.id, &id).map(Json)
}

pub async fn mine_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Vec<AssessmentInfo>>> {
    let u = require_role(&s, &h, "teacher")?;
    list_mine(&*lock(&s)?, &u.id).map(Json)
}

pub async fn available_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Vec<AssessmentInfo>>> {
    let u = require_role(&s, &h, "student")?;
    list_available(&*lock(&s)?, &u.id).map(Json)
}

pub async fn my_attempts_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Vec<MyAttempt>>> {
    let u = require_role(&s, &h, "student")?;
    my_attempts(&*lock(&s)?, &u.id, None).map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    fn q(id: &str, t: QuestionType, answer: &str, opts: &[&str], score: Option<f64>) -> Question {
        Question {
            id: id.into(), qtype: t, stem: format!("سؤال {id}"), options: opts.iter().map(|s| s.to_string()).collect(),
            answer: answer.into(), analysis: "شرح".into(), ai_analysis: None, score, subject: None, chapter: None, difficulty: None,
        }
    }

    fn questions() -> Vec<Question> {
        vec![
            q("q1", QuestionType::SingleChoice, "B", &["أ", "ب", "ج"], None),
            q("q2", QuestionType::MultiChoice, "A,C", &["أ", "ب", "ج"], Some(2.0)),
            q("q3", QuestionType::TrueFalse, "صح", &[], None),
            q("q4", QuestionType::FillBlank, "القاهرة|مصر", &[], None),
            q("q5", QuestionType::ShortAnswer, "نموذج", &[], Some(4.0)),
        ]
    }

    struct W {
        conn: Connection,
        teacher: User,
        student: User,
        admin: User,
        id: String,
    }

    fn world(show: bool, duration: Option<i64>, max_attempts: i64) -> W {
        let conn = create_test_db();
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','ج',1,0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0)", []).unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO teacher_subjects VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO subject_enrollments VALUES (?1,'s1',0)", params![student.id]).unwrap();
        let req = AssessmentReq {
            subject_id: Some("s1".into()), title: Some("اختبار".into()), description: None, questions: Some(questions()),
            duration_min: duration, opens_at: None, closes_at: None, max_attempts: Some(max_attempts), show_answers: Some(show),
            status: Some("published".into()), clear_duration: None, clear_window: None,
        };
        let id = create_assessment(&conn, &teacher, &req).unwrap().id;
        W { conn, teacher, student, admin, id }
    }

    fn ans(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn arabic_aware_grading() {
        let tf = q("t", QuestionType::TrueFalse, "صح", &[], None);
        assert_eq!(auto_grade(&tf, Some("صحيح")), Some(true));
        assert_eq!(auto_grade(&tf, Some("A")), Some(true));
        assert_eq!(auto_grade(&tf, Some("خطأ")), Some(false));
        assert_eq!(auto_grade(&tf, Some("")), Some(false));
        let fb = q("f", QuestionType::FillBlank, "القاهرة|مصر", &[], None);
        assert_eq!(auto_grade(&fb, Some("مِصْر")), Some(true), "diacritics ignored, alternatives accepted");
        assert_eq!(auto_grade(&fb, Some("  القاهره ")), Some(false));
        assert_eq!(auto_grade(&q("f2", QuestionType::FillBlank, "إسلام", &[], None), Some("اسلام")), Some(true), "alef variants");
        let mc = q("m", QuestionType::MultiChoice, "A,C", &["x", "y", "z"], None);
        assert_eq!(auto_grade(&mc, Some("C, A")), Some(true));
        assert_eq!(auto_grade(&mc, Some("A")), Some(false));
        assert_eq!(auto_grade(&q("s", QuestionType::ShortAnswer, "x", &[], None), Some("y")), None);
    }

    #[test]
    fn full_flow_grades_and_hides_answers_until_submit() {
        let w = world(true, Some(30), 2);
        let now = now_ms();
        let start = start_attempt(&w.conn, &w.student, &w.id, now).unwrap();
        let json = serde_json::to_string(&start.questions).unwrap();
        assert!(!json.contains("\"answer\"") && !json.contains("analysis") && !json.contains("القاهرة"), "no answers leak: {json}");
        assert_eq!(attempt_result(&w.conn, &w.student, &start.attempt_id).unwrap_err().0, StatusCode::CONFLICT, "no result while in progress");
        let again = start_attempt(&w.conn, &w.student, &w.id, now + 1000).unwrap();
        assert!(again.resumed && again.attempt_id == start.attempt_id, "resumes instead of burning an attempt");
        let r = submit_attempt(&w.conn, &w.student, &start.attempt_id, &ans(&[("q1", "B"), ("q2", "A,C"), ("q3", "صحيح"), ("q4", "مصر"), ("q5", "إجابتي"), ("ghost", "x")]), now + 5000).unwrap();
        assert_eq!((r.score, r.total, r.pending), (5.0, 9.0, 1), "1+2+1+1 auto; 4 pts short answer pending");
        assert_eq!(r.items.as_ref().unwrap().len(), 5);
        assert_eq!(submit_attempt(&w.conn, &w.student, &start.attempt_id, &ans(&[]), now + 6000).unwrap_err().0, StatusCode::CONFLICT, "no double submit");
        // teacher grades the short answer
        assert_eq!(grade_attempt(&w.conn, &w.admin, &start.attempt_id, &GradeReq { grades: ans_f(&[("q5", 3.0)]) }).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(grade_attempt(&w.conn, &w.teacher, &start.attempt_id, &GradeReq { grades: ans_f(&[("q5", 9.0)]) }).unwrap_err().0, StatusCode::BAD_REQUEST, "above max");
        assert_eq!(grade_attempt(&w.conn, &w.teacher, &start.attempt_id, &GradeReq { grades: ans_f(&[("q1", 0.0)]) }).unwrap_err().0, StatusCode::BAD_REQUEST, "auto-graded cannot be overridden");
        let g = grade_attempt(&w.conn, &w.teacher, &start.attempt_id, &GradeReq { grades: ans_f(&[("q5", 3.0)]) }).unwrap();
        assert_eq!((g.score, g.pending), (8.0, 0));
        let n: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'assessment_graded'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let s = results_summary(&w.conn, &w.teacher, &w.id).unwrap();
        assert_eq!((s.submitted, s.average, s.highest), (1, 8.0, 8.0));
    }

    fn ans_f(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn answers_hidden_from_students_when_disabled_but_teacher_sees_all() {
        let w = world(false, None, 1);
        let st = start_attempt(&w.conn, &w.student, &w.id, now_ms()).unwrap();
        let r = submit_attempt(&w.conn, &w.student, &st.attempt_id, &ans(&[("q1", "B")]), now_ms()).unwrap();
        assert!(r.items.is_none() && r.score == 1.0);
        assert!(attempt_result(&w.conn, &w.student, &st.attempt_id).unwrap().items.is_none());
        assert!(attempt_result(&w.conn, &w.teacher, &st.attempt_id).unwrap().items.is_some());
        let other = insert_test_user(&w.conn, "o@x.com", "student", "active");
        assert_eq!(attempt_result(&w.conn, &other, &st.attempt_id).unwrap_err().0, StatusCode::NOT_FOUND, "other students cannot read it");
    }

    #[test]
    fn timer_attempt_limit_and_window_are_enforced_server_side() {
        let w = world(true, Some(10), 1);
        let now = now_ms();
        let st = start_attempt(&w.conn, &w.student, &w.id, now).unwrap();
        assert_eq!(st.ends_at, now + 600_000);
        // submitting after the deadline + grace is rejected and the attempt expires
        let late = now + 600_000 + GRACE_MS + 1;
        assert_eq!(submit_attempt(&w.conn, &w.student, &st.attempt_id, &ans(&[("q1", "B")]), late).unwrap_err().1.contains("time_expired"), true);
        assert_eq!(start_attempt(&w.conn, &w.student, &w.id, late).unwrap_err().0, StatusCode::FORBIDDEN, "expired attempt still counts (max 1)");
        // within grace is accepted
        let w2 = world(true, Some(10), 1);
        let st2 = start_attempt(&w2.conn, &w2.student, &w2.id, now).unwrap();
        assert!(submit_attempt(&w2.conn, &w2.student, &st2.attempt_id, &ans(&[("q1", "B")]), now + 600_000 + GRACE_MS - 1).is_ok());
        // availability window
        let w3 = world(true, None, 3);
        w3.conn.execute("UPDATE assessments SET opens_at = ?1, closes_at = ?2", params![now + 1000, now + 5000]).unwrap();
        assert_eq!(start_attempt(&w3.conn, &w3.student, &w3.id, now).unwrap_err().0, StatusCode::FORBIDDEN);
        assert!(start_attempt(&w3.conn, &w3.student, &w3.id, now + 2000).is_ok());
        let w4 = world(true, None, 3);
        w4.conn.execute("UPDATE assessments SET closes_at = ?1", params![now - 1]).unwrap();
        assert!(start_attempt(&w4.conn, &w4.student, &w4.id, now).unwrap_err().1.contains("closed"));
    }

    #[test]
    fn access_rules() {
        let w = world(true, None, 1);
        let outsider = insert_test_user(&w.conn, "o@x.com", "student", "active");
        assert!(start_attempt(&w.conn, &outsider, &w.id, now_ms()).unwrap_err().1.contains("not_enrolled"));
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected'", []).unwrap();
        assert_eq!(start_attempt(&w.conn, &w.student, &w.id, now_ms()).unwrap_err().0, StatusCode::NOT_FOUND, "assignment revoked hides it");
        assert!(list_for_subject(&w.conn, &w.student.id, "s1").unwrap().is_empty());
        w.conn.execute("UPDATE teacher_subjects SET status = 'approved'", []).unwrap();
        w.conn.execute("UPDATE assessments SET status = 'draft'", []).unwrap();
        assert_eq!(start_attempt(&w.conn, &w.student, &w.id, now_ms()).unwrap_err().0, StatusCode::NOT_FOUND, "draft hidden");
        assert!(list_available(&w.conn, &w.student.id).unwrap().is_empty());
    }

    #[test]
    fn validation_and_edit_rules() {
        let w = world(true, None, 1);
        let base = |qs: Vec<Question>| AssessmentReq {
            subject_id: Some("s1".into()), title: Some("x".into()), description: None, questions: Some(qs), duration_min: None, opens_at: None,
            closes_at: None, max_attempts: None, show_answers: None, status: None, clear_duration: None, clear_window: None,
        };
        let mut dup = questions(); dup[1].id = "q1".into();
        for (label, qs) in [("empty", vec![]), ("dup ids", dup), ("choice without options", vec![q("x", QuestionType::SingleChoice, "A", &["only"], None)]),
                            ("empty answer", vec![q("x", QuestionType::FillBlank, " ", &[], None)]), ("negative score", vec![q("x", QuestionType::FillBlank, "a", &[], Some(-1.0))])] {
            assert_eq!(create_assessment(&w.conn, &w.teacher, &base(qs)).unwrap_err().0, StatusCode::BAD_REQUEST, "{label}");
        }
        let other = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        assert_eq!(create_assessment(&w.conn, &other, &base(questions())).unwrap_err().0, StatusCode::FORBIDDEN, "unassigned teacher");
        let patch = |t: &str| AssessmentReq { title: Some(t.into()), subject_id: None, description: None, questions: None, duration_min: None, opens_at: None, closes_at: None, max_attempts: None, show_answers: None, status: None, clear_duration: None, clear_window: None };
        assert_eq!(update_assessment(&w.conn, &other, &w.id, &patch("hack")).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(update_assessment(&w.conn, &w.admin, &w.id, &patch("rewrite")).unwrap_err().0, StatusCode::FORBIDDEN, "admin moderates status only");
        let unpublish = AssessmentReq { status: Some("draft".into()), ..patch("") };
        let unpublish = AssessmentReq { title: None, ..unpublish };
        assert_eq!(update_assessment(&w.conn, &w.admin, &w.id, &unpublish).unwrap().status, "draft");
        // questions are frozen once attempts exist
        w.conn.execute("UPDATE assessments SET status = 'published'", []).unwrap();
        start_attempt(&w.conn, &w.student, &w.id, now_ms()).unwrap();
        let replace = AssessmentReq { questions: Some(questions()), title: None, ..patch("") };
        assert_eq!(update_assessment(&w.conn, &w.teacher, &w.id, &replace).unwrap_err().0, StatusCode::CONFLICT);
        // deleting cascades attempts
        delete_assessment(&w.conn, &w.teacher, &w.id).unwrap();
        let n: i64 = w.conn.query_row("SELECT count(*) FROM attempts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }
}

/// Phase 1-0: upgrading a database produced by the previous server version must never lose data.
#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::platform::{apply_schema, create_test_db};

    const FIXTURE: &str = include_str!("../tests/fixtures/platform_v1.sql");

    /// The database exactly as the pre-1-0 server wrote it (loaded with FKs off, like a file on disk).
    fn old_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        c.execute_batch(FIXTURE).unwrap();
        c
    }

    fn tables(c: &Connection) -> Vec<String> {
        c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn counts(c: &Connection) -> Vec<(String, i64)> {
        tables(c).into_iter().map(|t| { let n = c.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0)).unwrap(); (t, n) }).collect()
    }

    /// (name, type, notnull, default) per column, order-independent.
    fn columns(c: &Connection, table: &str) -> Vec<(String, String, i64, Option<String>)> {
        let mut v: Vec<_> = c
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| Ok((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        v.sort();
        v
    }

    fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn fixture_really_is_the_old_schema() {
        let c = old_db();
        assert!(!crate::platform::column_exists(&c, "assessments", "created_by").unwrap());
        assert!(!crate::platform::column_exists(&c, "attempts", "tab_leaves").unwrap());
        assert!(!crate::platform::column_exists(&c, "users", "consented_at").unwrap());
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM assessments"), 2);
    }

    #[test]
    fn upgrade_preserves_every_row_and_value() {
        let c = old_db();
        let before = counts(&c);
        let assess_before: Vec<(String, String, f64, String)> = c
            .prepare("SELECT id, title, total_points, status FROM assessments ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap();
        let attempts_before: Vec<(String, String, f64, i64, String, String)> = c
            .prepare("SELECT id, status, score, pending, answers, results FROM attempts ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().collect::<Result<_, _>>().unwrap();
        let questions_before: Vec<String> = c.prepare("SELECT questions FROM assessments ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();

        apply_schema(&c).unwrap();

        assert_eq!(counts(&c).into_iter().filter(|(t, _)| before.iter().any(|(b, _)| b == t)).collect::<Vec<_>>(), before, "row counts unchanged for every pre-existing table");
        let assess_after: Vec<(String, String, f64, String)> = c
            .prepare("SELECT id, title, total_points, status FROM assessments ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(assess_after, assess_before);
        let attempts_after: Vec<(String, String, f64, i64, String, String)> = c
            .prepare("SELECT id, status, score, pending, answers, results FROM attempts ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(attempts_after, attempts_before, "answers/results/scores byte-identical");
        let questions_after: Vec<String> = c.prepare("SELECT questions FROM assessments ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(questions_after, questions_before, "question JSON untouched");
        assert!(attempts_after.iter().any(|a| a.2 == 8.0), "the graded attempt (8.0) survived");
    }

    #[test]
    fn upgrade_sets_safe_defaults_and_keeps_integrity() {
        let c = old_db();
        apply_schema(&c).unwrap();
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM assessments WHERE created_by IS NOT teacher_id"), 0, "creator = old owner");
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM assessments WHERE shuffle_questions <> 0 OR shuffle_options <> 0 OR pass_mark IS NOT NULL OR closed_at IS NOT NULL OR archived_at IS NOT NULL OR release_mode <> 'immediate'"), 0, "behaviour unchanged by default");
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM attempts WHERE tab_leaves <> 0 OR saved_at IS NOT NULL OR order_json IS NOT NULL"), 0);
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM users WHERE consented_at IS NOT NULL"), 0);
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM pragma_foreign_key_check"), 0, "no dangling references");
        assert_eq!(one::<String>(&c, "PRAGMA integrity_check"), "ok");
        assert_eq!(one::<i64>(&c, "PRAGMA foreign_keys"), 1, "foreign keys are back ON after the swap");
        assert!(tables(&c).contains(&"settings".to_string()));
        assert!(!tables(&c).contains(&"assessments_new".to_string()), "no leftover temp table");
    }

    #[test]
    fn upgrade_is_idempotent() {
        let c = old_db();
        apply_schema(&c).unwrap();
        let snapshot = |c: &Connection| -> (Vec<(String, i64)>, String) {
            (counts(c), one::<String>(c, "SELECT group_concat(id || title || status || created_by, '|') FROM (SELECT * FROM assessments ORDER BY id)"))
        };
        let first = snapshot(&c);
        apply_schema(&c).unwrap();
        apply_schema(&c).unwrap();
        assert_eq!(snapshot(&c), first);
    }

    #[test]
    fn upgraded_schema_equals_a_fresh_install() {
        let migrated = old_db();
        apply_schema(&migrated).unwrap();
        let fresh = create_test_db();
        assert_eq!(tables(&migrated), tables(&fresh), "same set of tables");
        for t in tables(&fresh) {
            assert_eq!(columns(&migrated, &t), columns(&fresh, &t), "column definitions differ for `{t}`");
        }
    }

    #[test]
    fn constraints_survive_the_rebuild() {
        let c = old_db();
        apply_schema(&c).unwrap();
        // new lifecycle states are allowed, junk is not
        let id: String = one(&c, "SELECT assessment_id FROM attempts GROUP BY assessment_id ORDER BY count(*) DESC LIMIT 1");
        for ok in ["closed", "archived", "draft", "published"] {
            c.execute("UPDATE assessments SET status = ?1 WHERE id = ?2", params![ok, id]).unwrap();
        }
        assert!(c.execute("UPDATE assessments SET status = 'bogus' WHERE id = ?1", params![id]).is_err());
        assert!(c.execute("UPDATE assessments SET release_mode = 'sometimes' WHERE id = ?1", params![id]).is_err());
        // FK cascades still work: deleting an assessment removes its attempts
        let attempts_of: i64 = c.query_row("SELECT count(*) FROM attempts WHERE assessment_id = ?1", params![id], |r| r.get(0)).unwrap();
        assert!(attempts_of > 0);
        c.execute("DELETE FROM assessments WHERE id = ?1", params![id]).unwrap();
        assert_eq!(c.query_row::<i64, _, _>("SELECT count(*) FROM attempts WHERE assessment_id = ?1", params![id], |r| r.get(0)).unwrap(), 0);
        // legacy owner semantics: deleting the teacher still removes their exams
        let teacher: String = one(&c, "SELECT teacher_id FROM assessments LIMIT 1");
        c.execute("DELETE FROM users WHERE id = ?1", params![teacher]).unwrap();
        assert_eq!(c.query_row::<i64, _, _>("SELECT count(*) FROM assessments WHERE teacher_id = ?1", params![teacher], |r| r.get(0)).unwrap(), 0);
    }

    #[test]
    fn admin_created_exams_have_no_teacher_and_survive_their_creator() {
        let c = old_db();
        apply_schema(&c).unwrap();
        let admin: String = one(&c, "SELECT id FROM users WHERE role = 'admin' LIMIT 1");
        let subject: String = one(&c, "SELECT id FROM subjects LIMIT 1");
        c.execute(
            "INSERT INTO assessments(id, teacher_id, created_by, subject_id, title, questions, question_count, total_points, status, created_at, updated_at)
             VALUES ('admin-exam', NULL, ?1, ?2, 'امتحان المدير', '[]', 0, 0, 'draft', 1, 1)",
            params![admin, subject],
        )
        .unwrap();
        c.execute("DELETE FROM users WHERE id = ?1", params![admin]).unwrap();
        let (teacher, creator): (Option<String>, Option<String>) =
            c.query_row("SELECT teacher_id, created_by FROM assessments WHERE id = 'admin-exam'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((teacher, creator), (None, None), "exam survives, creator reference cleared");
    }

    #[test]
    fn existing_features_work_on_the_upgraded_database() {
        let c = old_db();
        apply_schema(&c).unwrap();
        let user = |email: &str| -> User {
            c.query_row(&format!("SELECT {} FROM users WHERE email = ?1", crate::platform::USER_COLS), params![email], crate::platform::row_to_user).unwrap()
        };
        let (teacher, s1, s2) = (user("t1@x.com"), user("s1@x.com"), user("s2@x.com"));
        let published: String = one(&c, "SELECT id FROM assessments WHERE status = 'published'");
        // teacher dashboard numbers come out as they did before the upgrade
        let r = results_summary(&c, &teacher, &published).unwrap();
        assert_eq!((r.submitted, r.highest), (2, 8.0));
        // student review of the graded attempt still shows the graded items
        let graded: String = one(&c, "SELECT id FROM attempts WHERE score = 8.0");
        let view = attempt_result(&c, &s1, &graded).unwrap();
        assert_eq!((view.score, view.total, view.pending), (8.0, 9.0, 0));
        assert_eq!(view.items.as_ref().map(|i| i.len()), Some(5));
        // the pending one is still gradable by the teacher, and a new attempt can start/resume
        let pending: String = one(&c, "SELECT id FROM attempts WHERE pending = 1");
        let g = grade_attempt(&c, &teacher, &pending, &GradeReq { grades: [("q5".to_string(), 2.0)].into() }).unwrap();
        assert_eq!((g.score, g.pending), (2.0, 0));
        let resumed = start_attempt(&c, &s1, &published, now_ms()).unwrap();
        assert!(resumed.resumed, "the in-progress attempt from before the upgrade is resumed");
        assert!(start_attempt(&c, &s2, &published, now_ms()).is_ok());
        // creating new exams records the creator
        let subject: String = one(&c, "SELECT subject_id FROM assessments LIMIT 1");
        let new = create_assessment(&c, &teacher, &AssessmentReq {
            subject_id: Some(subject), title: Some("جديد".into()), description: None,
            questions: Some(vec![q("n1", QuestionType::FillBlank, "x", &[], None)]), duration_min: None, opens_at: None, closes_at: None,
            max_attempts: None, show_answers: None, status: Some("draft".into()), clear_duration: None, clear_window: None,
        })
        .unwrap();
        assert_eq!(c.query_row::<String, _, _>("SELECT created_by FROM assessments WHERE id = ?1", params![new.id], |r| r.get(0)).unwrap(), teacher.id);
    }

    fn q(id: &str, t: QuestionType, answer: &str, opts: &[&str], score: Option<f64>) -> Question {
        Question { id: id.into(), qtype: t, stem: format!("سؤال {id}"), options: opts.iter().map(|s| s.to_string()).collect(), answer: answer.into(),
                   analysis: String::new(), ai_analysis: None, score, subject: None, chapter: None, difficulty: None }
    }

    #[test]
    fn a_failed_upgrade_rolls_back_completely() {
        let c = old_db();
        let before = counts(&c);
        // a view squatting on the temp table name makes the swap fail midway
        c.execute_batch("CREATE VIEW assessments_new AS SELECT 1;").unwrap();
        let err = apply_schema(&c).unwrap_err();
        assert!(!err.is_empty());
        assert!(!crate::platform::column_exists(&c, "assessments", "created_by").unwrap(), "old table left exactly as it was");
        assert_eq!(counts(&c).into_iter().filter(|(t, _)| before.iter().any(|(b, _)| b == t)).collect::<Vec<_>>(), before, "no rows lost");
        assert_eq!(one::<i64>(&c, "PRAGMA foreign_keys"), 1, "foreign keys restored even on failure");
        // fix the obstacle and the same database upgrades cleanly
        c.execute_batch("DROP VIEW assessments_new;").unwrap();
        apply_schema(&c).unwrap();
        assert!(crate::platform::column_exists(&c, "assessments", "created_by").unwrap());
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM assessments"), 2);
    }

    #[test]
    fn dangling_references_abort_the_upgrade_without_changes() {
        let c = old_db();
        c.execute("INSERT INTO attempts(id, assessment_id, student_id, started_at, status) VALUES ('orphan', 'no-such-exam', (SELECT id FROM users LIMIT 1), 1, 'in_progress')", []).unwrap();
        let err = apply_schema(&c).unwrap_err();
        assert!(err.contains("foreign key"), "{err}");
        assert!(!crate::platform::column_exists(&c, "assessments", "created_by").unwrap());
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM assessments"), 2);
        assert_eq!(one::<i64>(&c, "PRAGMA foreign_keys"), 1);
    }
}
