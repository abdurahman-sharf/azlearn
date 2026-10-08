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
-- statistics count submitted attempts per exam (platform_stats); a partial index keeps that an index-only scan
CREATE INDEX IF NOT EXISTS idx_attempts_submitted ON attempts(assessment_id) WHERE status = 'submitted';
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
    // Question -> bank item it was copied from (statistics only); the exam itself is a fixed snapshot.
    crate::platform::add_column_if_missing(conn, "assessments", "source_map", "TEXT")?;
    // When students were last told about the exam (phase 1-8): the closing reminder never lands on top of it.
    crate::platform::add_column_if_missing(conn, "assessments", "published_at", "INTEGER")?;
    Ok(())
}

pub(crate) const MAX_QUESTIONS: usize = 200;
const GRACE_MS: i64 = 60_000;
const OPEN_ENDED_LIMIT_MS: i64 = 24 * 3_600_000;

// ───────── grading ─────────

/// Arabic-aware comparison form: strips diacritics/tatweel, unifies alef/yeh variants, lowercases.
pub(crate) fn norm_text(s: &str) -> String {
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

pub(crate) fn points_of(q: &Question) -> f64 {
    q.score.filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(1.0)
}

pub(crate) fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct Outcome {
    pub(crate) id: String,
    pub(crate) correct: Option<bool>,
    pub(crate) points: f64,
    pub(crate) max: f64,
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

/// Shape checks shared by exams and the question bank (ids and uniqueness are the caller's business).
pub(crate) fn validate_question(q: &Question) -> Res<()> {
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
    Ok(())
}

pub(crate) fn validate_questions(qs: &[Question]) -> Res<()> {
    if qs.is_empty() || qs.len() > MAX_QUESTIONS {
        return Err(bad("invalid_question_count"));
    }
    let mut seen = HashSet::new();
    for q in qs {
        let id_ok = !q.id.trim().is_empty() && q.id.chars().count() <= 100;
        if !id_ok || !seen.insert(q.id.clone()) {
            return Err(bad("invalid_question_id"));
        }
        validate_question(q)?;
    }
    Ok(())
}

pub(crate) fn check_window(opens: Option<i64>, closes: Option<i64>) -> Res<()> {
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
    pub(crate) id: String,
    /// `None` for exams created by an admin (they have no owning teacher).
    pub(crate) teacher_id: Option<String>,
    /// The owning teacher's name, or the creating admin's for admin exams.
    pub(crate) teacher_name: String,
    pub(crate) subject_id: String,
    pub(crate) subject_name: String,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) question_count: i64,
    pub(crate) total_points: f64,
    pub(crate) duration_min: Option<i64>,
    pub(crate) opens_at: Option<i64>,
    pub(crate) closes_at: Option<i64>,
    pub(crate) max_attempts: i64,
    pub(crate) show_answers: bool,
    pub(crate) status: String,
    pub(crate) attempts_used: i64,
    pub(crate) attempt_count: i64,
    pub(crate) shuffle_questions: bool,
    pub(crate) shuffle_options: bool,
    /// Pass mark as a percentage of the total points.
    pub(crate) pass_mark: Option<f64>,
    /// `immediate` or `after_close`.
    pub(crate) release_mode: String,
    pub(crate) closed_at: Option<i64>,
    pub(crate) archived_at: Option<i64>,
    pub(crate) created_by: Option<String>,
}

impl AssessmentInfo {
    #[cfg(test)]
    pub(crate) fn id(&self) -> String {
        self.id.clone()
    }
}

pub(crate) const INFO_SELECT: &str = "SELECT a.id, a.teacher_id, COALESCE(u.full_name, cb.full_name, ''), a.subject_id, s.name_ar, a.title, a.description, a.question_count,
        a.total_points, a.duration_min, a.opens_at, a.closes_at, a.max_attempts, a.show_answers, a.status,
        (SELECT count(*) FROM attempts t WHERE t.assessment_id = a.id AND t.student_id = ?1),
        (SELECT count(*) FROM attempts t WHERE t.assessment_id = a.id),
        a.shuffle_questions, a.shuffle_options, a.pass_mark, a.release_mode, a.closed_at, a.archived_at, a.created_by
     FROM assessments a LEFT JOIN users u ON u.id = a.teacher_id LEFT JOIN users cb ON cb.id = a.created_by JOIN subjects s ON s.id = a.subject_id";

pub(crate) fn map_info(r: &rusqlite::Row) -> rusqlite::Result<AssessmentInfo> {
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
        shuffle_questions: r.get(17)?,
        shuffle_options: r.get(18)?,
        pass_mark: r.get(19)?,
        release_mode: r.get(20)?,
        closed_at: r.get(21)?,
        archived_at: r.get(22)?,
        created_by: r.get(23)?,
    })
}

pub(crate) fn get_info(conn: &Connection, viewer_id: &str, id: &str) -> Res<AssessmentInfo> {
    conn.query_row(&format!("{INFO_SELECT} WHERE a.id = ?2"), params![viewer_id, id], map_info)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

/// Shared SQL predicate: an admin-created exam (no teacher) needs nothing more; a teacher's exam needs the
/// teacher to be active and approved for the subject. Expects aliases `a` (assessments) and `u` (its teacher).
pub(crate) const TEACHER_OK: &str = "(a.teacher_id IS NULL OR (u.status = 'active'
            AND EXISTS(SELECT 1 FROM teacher_subjects ts WHERE ts.teacher_id = a.teacher_id AND ts.subject_id = a.subject_id AND ts.status = 'approved')))";

/// Published (or closed, so students can still see it and their results), subject active, teacher in good standing.
fn publicly_visible(conn: &Connection, id: &str) -> Res<bool> {
    conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM assessments a LEFT JOIN users u ON u.id = a.teacher_id JOIN subjects s ON s.id = a.subject_id
          WHERE a.id = ?1 AND a.status IN ('published','closed') AND s.is_active = 1 AND {TEACHER_OK})"),
        params![id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

pub(crate) fn is_enrolled(conn: &Connection, student_id: &str, subject_id: &str) -> Res<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM subject_enrollments WHERE student_id = ?1 AND subject_id = ?2)",
        params![student_id, subject_id],
        |r| r.get(0),
    )
    .map_err(db_err)
}

pub(crate) fn load_questions(conn: &Connection, id: &str) -> Res<Vec<Question>> {
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

pub(crate) fn check_duration(d: Option<i64>) -> Res<()> {
    match d {
        Some(m) if !(1..=480).contains(&m) => Err(bad("invalid_time")),
        _ => Ok(()),
    }
}

pub(crate) fn create_assessment(conn: &Connection, teacher: &User, r: &AssessmentReq) -> Res<AssessmentInfo> {
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
        announce(conn, teacher, subject_id, &id, &title, now);
    }
    get_info(conn, &teacher.id, &id)
}

/// Tells the audience about a newly published exam and records when (`published_at`).
/// Only the **first** publication announces: `published_at` is claimed with a conditional update and never
/// overwritten, so unpublishing and publishing again (or flipping draft/published while editing) does not
/// notify every follower and enrolled student a second time.
pub(crate) fn announce(conn: &Connection, teacher: &User, subject_id: &str, id: &str, title: &str, now: i64) {
    let first = conn.execute("UPDATE assessments SET published_at = ?2 WHERE id = ?1 AND published_at IS NULL", params![id, now]).map_or(false, |n| n == 1);
    if !first {
        return;
    }
    crate::platform_engage::notify_audience(
        conn,
        &teacher.id,
        subject_id,
        "new_assessment",
        json!({ "title": title, "teacher": teacher.full_name }),
        &format!("/platform/assessments/{id}"),
    );
}

/// The owning teacher, or `None` for an admin-created exam.
pub(crate) fn owner_of(conn: &Connection, id: &str) -> Res<Option<String>> {
    conn.query_row("SELECT teacher_id FROM assessments WHERE id = ?1", params![id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

/// Who may grade and see the results of an exam: any admin; the owning teacher of a teacher's exam;
/// for an admin-created exam (no owner) any active teacher approved for its subject (PRD G2).
pub(crate) fn can_grade(conn: &Connection, user: &User, assessment_id: &str) -> Res<bool> {
    if user.role == "admin" {
        return Ok(true);
    }
    if user.role != "teacher" {
        return Ok(false);
    }
    match owner_of(conn, assessment_id)? {
        Some(owner) => Ok(owner == user.id),
        None => conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM assessments a JOIN teacher_subjects ts ON ts.subject_id = a.subject_id
                  WHERE a.id = ?1 AND ts.teacher_id = ?2 AND ts.status = 'approved')",
                params![assessment_id, user.id],
                |r| r.get(0),
            )
            .map_err(db_err),
    }
}

fn update_assessment(conn: &Connection, user: &User, id: &str, r: &AssessmentReq) -> Res<AssessmentInfo> {
    let owner = owner_of(conn, id)?;
    let is_owner = owner.as_deref() == Some(user.id.as_str());
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
    // A closed/archived exam is read-only for its teacher (an admin closed or archived it).
    if is_owner && !["draft", "published"].contains(&cur.status.as_str()) {
        return Err(err(StatusCode::CONFLICT, "closed"));
    }
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
    // Students already sat (or are sitting) this exam: pulling it back to a draft would hide it from them mid-way.
    // Closing it is the way to stop new attempts (admins have their own lifecycle actions for that).
    if status == "draft" && cur.status != "draft" && cur.attempt_count > 0 {
        return Err(err(StatusCode::CONFLICT, "has_attempts"));
    }
    if status == "published" && is_owner {
        let ok: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2 AND status = 'approved')",
                params![user.id, cur.subject_id],
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
        if let Some(owner) = &owner {
            crate::platform_engage::notify(conn, owner, "content_unpublished", json!({ "title": title }), &format!("/platform/assessments/{id}"));
        }
    } else if status == "published" && cur.status != "published" {
        announce(conn, user, &cur.subject_id, id, &title, now_ms());
    }
    get_info(conn, &user.id, id)
}

/// Deleting an exam takes every attempt and grade with it (`ON DELETE CASCADE`), so an exam that students have
/// already sat can never be deleted here — not by its teacher, and not by an admin through this legacy route
/// (an admin removes it from the admin screen, which asks for the exam's title first). Closing or archiving
/// is the way to retire such an exam.
fn delete_assessment(conn: &Connection, user: &User, id: &str) -> Res<()> {
    let owner = owner_of(conn, id)?;
    if owner.as_deref() != Some(user.id.as_str()) && user.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let attempts: i64 = conn.query_row("SELECT count(*) FROM attempts WHERE assessment_id = ?1", params![id], |r| r.get(0)).map_err(db_err)?;
    if attempts > 0 {
        return Err(err(StatusCode::CONFLICT, "has_attempts"));
    }
    conn.execute("DELETE FROM assessments WHERE id = ?1", params![id]).map_err(db_err)?;
    audit(conn, &user.id, id, "assessment_deleted", &format!("attempts={attempts}"));
    Ok(())
}

// ───────── "may teachers create exams?" (teachers.can_create_exams) ─────────

/// 403 `exams_disabled` while the admin has explicitly switched teacher exams off (unset = on).
/// Enforced by the HTTP entry points below, not inside `create_assessment`/`update_assessment`, which
/// stay pure so other modules and tests can call them directly.
pub(crate) fn ensure_exams_enabled(conn: &Connection, crypto: &crate::platform_settings::Crypto) -> Res<()> {
    if crate::platform_ai::exams_enabled(conn, crypto)? {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "exams_disabled"))
    }
}

/// Creating an exam (draft or published) needs the switch to be on.
pub(crate) fn create_checked(conn: &Connection, crypto: &crate::platform_settings::Crypto, teacher: &User, r: &AssessmentReq) -> Res<AssessmentInfo> {
    ensure_exams_enabled(conn, crypto)?;
    create_assessment(conn, teacher, r)
}

/// Moving an exam to `published` needs the switch to be on. Everything else stays possible with it off:
/// editing, closing, and managing an exam that is already published (switching off stops *new* exams only).
pub(crate) fn update_checked(conn: &Connection, crypto: &crate::platform_settings::Crypto, user: &User, id: &str, r: &AssessmentReq) -> Res<AssessmentInfo> {
    if r.status.as_deref() == Some("published") && get_info(conn, "", id)?.status != "published" {
        ensure_exams_enabled(conn, crypto)?;
    }
    update_assessment(conn, user, id, r)
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
    pub(crate) attempt_id: String,
    pub(crate) started_at: i64,
    /// Server-enforced deadline (ms epoch).
    pub(crate) ends_at: i64,
    /// Questions in this attempt's display order, with options already permuted when the exam shuffles them.
    pub(crate) questions: Vec<PublicQuestion>,
    pub(crate) resumed: bool,
    /// Answers autosaved earlier (keyed like `questions`, i.e. in the *displayed* option letters), so an
    /// attempt resumes on any device.
    pub(crate) saved_answers: HashMap<String, String>,
    pub(crate) saved_at: Option<i64>,
}

fn deadline(started_at: i64, duration_min: Option<i64>) -> i64 {
    started_at + duration_min.map_or(OPEN_ENDED_LIMIT_MS, |m| m * 60_000)
}

// ───────── per-attempt shuffling (PRD F2) ─────────

/// The order one attempt was dealt, stored in `attempts.order_json` so reopening it shows the same
/// layout and answers can be mapped back to the original options at grading time.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub(crate) struct Order {
    /// Question ids in display order.
    q: Vec<String>,
    /// Choice questions only: displayed option position -> original option index.
    o: HashMap<String, Vec<usize>>,
}

fn is_choice(t: &QuestionType) -> bool {
    matches!(t, QuestionType::SingleChoice | QuestionType::MultiChoice)
}

/// Deals a fresh order, or `None` when the exam shuffles nothing (the stored layout is then the original).
pub(crate) fn make_order(questions: &[Question], shuffle_questions: bool, shuffle_options: bool, rng: &mut impl rand::Rng) -> Option<Order> {
    use rand::seq::SliceRandom;
    if !shuffle_questions && !shuffle_options {
        return None;
    }
    let mut q: Vec<String> = questions.iter().map(|x| x.id.clone()).collect();
    if shuffle_questions {
        q.shuffle(rng);
    }
    let mut o = HashMap::new();
    if shuffle_options {
        for x in questions.iter().filter(|x| is_choice(&x.qtype) && x.options.len() > 1) {
            let mut perm: Vec<usize> = (0..x.options.len()).collect();
            perm.shuffle(rng);
            o.insert(x.id.clone(), perm);
        }
    }
    Some(Order { q, o })
}

/// The question list as one student sees it (reordered, options permuted).
fn displayed(questions: &[Question], order: Option<&Order>) -> Vec<Question> {
    let Some(order) = order else { return questions.to_vec() };
    let by_id: HashMap<&str, &Question> = questions.iter().map(|q| (q.id.as_str(), q)).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<Question> = vec![];
    for id in &order.q {
        if let Some(q) = by_id.get(id.as_str()) {
            seen.insert(q.id.as_str());
            out.push(permuted(q, order));
        }
    }
    // anything the stored order does not mention keeps its place at the end
    out.extend(questions.iter().filter(|q| !seen.contains(q.id.as_str())).map(|q| permuted(q, order)));
    out
}

fn permuted(q: &Question, order: &Order) -> Question {
    let mut c = q.clone();
    if let Some(perm) = order.o.get(&q.id) {
        if perm.len() == q.options.len() && perm.iter().all(|&i| i < q.options.len()) {
            c.options = perm.iter().map(|&i| q.options[i].clone()).collect();
        }
    }
    c
}

/// Converts answers given against the displayed options back to the original option letters.
pub(crate) fn to_original(questions: &[Question], order: Option<&Order>, answers: &HashMap<String, String>) -> HashMap<String, String> {
    let Some(order) = order else { return answers.clone() };
    let mut out = answers.clone();
    for q in questions.iter().filter(|q| is_choice(&q.qtype)) {
        let (Some(perm), Some(given)) = (order.o.get(&q.id), answers.get(&q.id)) else { continue };
        let mut letters: Vec<char> = given
            .to_uppercase()
            .chars()
            .filter(|c| c.is_ascii_uppercase())
            .filter_map(|c| perm.get((c as u8 - b'A') as usize).map(|&orig| (b'A' + orig as u8) as char))
            .collect();
        letters.sort();
        letters.dedup();
        out.insert(q.id.clone(), letters.into_iter().collect());
    }
    out
}

fn parse_order(raw: Option<String>) -> Option<Order> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

fn public_questions(qs: &[Question]) -> Vec<PublicQuestion> {
    qs.iter().map(|q| PublicQuestion { id: q.id.clone(), qtype: q.qtype.clone(), stem: q.stem.clone(), options: q.options.clone(), points: points_of(q) }).collect()
}

// ───────── finishing attempts ─────────

/// Grades `shown` answers (displayed letters) and stores the attempt as submitted at `submitted_at`.
fn finalize_attempt(conn: &Connection, attempt_id: &str, assessment_id: &str, shown: &HashMap<String, String>, order_raw: Option<String>, submitted_at: i64) -> Res<()> {
    let questions = load_questions(conn, assessment_id)?;
    let order = parse_order(order_raw);
    let answers = to_original(&questions, order.as_ref(), &clean_answers(shown, &questions));
    let (outcomes, score, pending) = grade_all(&questions, &answers);
    conn.execute(
        "UPDATE attempts SET status = 'submitted', submitted_at = ?1, answers = ?2, results = ?3, score = ?4, pending = ?5 WHERE id = ?6",
        params![submitted_at, serde_json::to_string(&answers).map_err(db_err)?, serde_json::to_string(&outcomes).map_err(db_err)?, score, pending, attempt_id],
    )
    .map_err(db_err)?;
    Ok(())
}

/// If an in-progress attempt is past its deadline (+grace): submit whatever was autosaved, or mark it
/// expired (score 0) when nothing was ever saved. Returns whether it was due.
fn settle_if_due(conn: &Connection, attempt_id: &str, assessment_id: &str, duration_min: Option<i64>, now: i64) -> Res<bool> {
    let row: Option<(i64, Option<i64>, String, Option<String>)> = conn
        .query_row("SELECT started_at, saved_at, answers, order_json FROM attempts WHERE id = ?1 AND status = 'in_progress'", params![attempt_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()
        .map_err(db_err)?;
    let Some((started, saved_at, answers_raw, order_raw)) = row else { return Ok(false) };
    let due = deadline(started, duration_min);
    if now <= due + GRACE_MS {
        return Ok(false);
    }
    if saved_at.is_some() {
        // the browser went away (or lost connection) before the timer ran out: the autosaved answers count
        let shown: HashMap<String, String> = serde_json::from_str(&answers_raw).unwrap_or_default();
        finalize_attempt(conn, attempt_id, assessment_id, &shown, order_raw, due)?;
    } else {
        conn.execute("UPDATE attempts SET status = 'expired' WHERE id = ?1", params![attempt_id]).map_err(db_err)?;
    }
    Ok(true)
}

/// Settles this student's overdue attempts on one assessment.
fn expire_stale(conn: &Connection, assessment_id: &str, student_id: &str, duration_min: Option<i64>, now: i64) -> Res<()> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM attempts WHERE assessment_id = ?1 AND student_id = ?2 AND status = 'in_progress'")
        .map_err(db_err)?
        .query_map(params![assessment_id, student_id], |r| r.get(0))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    for id in ids {
        settle_if_due(conn, &id, assessment_id, duration_min, now)?;
    }
    Ok(())
}

/// Settles every overdue attempt of an assessment (graders open its results).
pub(crate) fn settle_assessment(conn: &Connection, assessment_id: &str, duration_min: Option<i64>, now: i64) -> Res<()> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM attempts WHERE assessment_id = ?1 AND status = 'in_progress'")
        .map_err(db_err)?
        .query_map(params![assessment_id], |r| r.get(0))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    for id in ids {
        settle_if_due(conn, &id, assessment_id, duration_min, now)?;
    }
    Ok(())
}

/// Hourly task entry point: settles overdue attempts nobody came back to.
pub fn settle_overdue(state: &crate::platform::PlatformState) {
    if let Ok(conn) = state.conn.lock() {
        let _ = settle_all(&conn, now_ms());
    }
}

/// Periodic sweep: settles every overdue attempt on the platform. Returns how many were settled.
pub(crate) fn settle_all(conn: &Connection, now: i64) -> Res<usize> {
    let rows: Vec<(String, String, Option<i64>)> = conn
        .prepare("SELECT t.id, t.assessment_id, a.duration_min FROM attempts t JOIN assessments a ON a.id = t.assessment_id WHERE t.status = 'in_progress'")
        .map_err(db_err)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut n = 0;
    for (id, aid, dur) in rows {
        if settle_if_due(conn, &id, &aid, dur, now)? {
            n += 1;
        }
    }
    Ok(n)
}

pub(crate) fn start_attempt(conn: &Connection, student: &User, assessment_id: &str, now: i64) -> Res<StartRes> {
    start_attempt_with(conn, student, assessment_id, now, &mut rand::thread_rng())
}

pub(crate) fn start_attempt_with(conn: &Connection, student: &User, assessment_id: &str, now: i64, rng: &mut impl rand::Rng) -> Res<StartRes> {
    if !publicly_visible(conn, assessment_id)? {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let info = get_info(conn, &student.id, assessment_id)?;
    if !is_enrolled(conn, &student.id, &info.subject_id)? {
        return Err(err(StatusCode::FORBIDDEN, "not_enrolled"));
    }
    expire_stale(conn, assessment_id, &student.id, info.duration_min, now)?;
    let questions = load_questions(conn, assessment_id)?;
    // Resume an in-progress attempt instead of burning another one: same order, same saved answers.
    let open: Option<(String, i64, Option<String>, String, Option<i64>)> = conn
        .query_row(
            "SELECT id, started_at, order_json, answers, saved_at FROM attempts WHERE assessment_id = ?1 AND student_id = ?2 AND status = 'in_progress'",
            params![assessment_id, student.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(db_err)?;
    if let Some((id, started, order_raw, answers_raw, saved_at)) = open {
        let order = parse_order(order_raw);
        return Ok(StartRes {
            attempt_id: id,
            started_at: started,
            ends_at: deadline(started, info.duration_min),
            questions: public_questions(&displayed(&questions, order.as_ref())),
            resumed: true,
            saved_answers: serde_json::from_str(&answers_raw).unwrap_or_default(),
            saved_at,
        });
    }
    if info.status != "published" {
        return Err(err(StatusCode::FORBIDDEN, "closed"));
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
    let order = make_order(&questions, info.shuffle_questions, info.shuffle_options, rng);
    let id = new_id();
    conn.execute(
        "INSERT INTO attempts(id, assessment_id, student_id, started_at, status, order_json) VALUES (?1,?2,?3,?4,'in_progress',?5)",
        params![id, assessment_id, student.id, now, order.as_ref().map(|o| serde_json::to_string(o).unwrap_or_default())],
    )
    .map_err(db_err)?;
    Ok(StartRes {
        attempt_id: id,
        started_at: now,
        ends_at: deadline(now, info.duration_min),
        questions: public_questions(&displayed(&questions, order.as_ref())),
        resumed: false,
        saved_answers: HashMap::new(),
        saved_at: None,
    })
}

// ───────── autosave & integrity events (PRD F3, F5) ─────────

/// Stores the in-progress answers (displayed letters) so the attempt can resume anywhere.
pub(crate) fn save_answers(conn: &Connection, student: &User, attempt_id: &str, raw: &HashMap<String, String>, now: i64) -> Res<i64> {
    let (assessment_id, started, status): (String, i64, String) = conn
        .query_row("SELECT assessment_id, started_at, status FROM attempts WHERE id = ?1 AND student_id = ?2", params![attempt_id, student.id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if status != "in_progress" {
        return Err(err(StatusCode::CONFLICT, if status == "expired" { "time_expired" } else { "already_submitted" }));
    }
    let info = get_info(conn, &student.id, &assessment_id)?;
    if now > deadline(started, info.duration_min) + GRACE_MS {
        settle_if_due(conn, attempt_id, &assessment_id, info.duration_min, now)?;
        return Err(err(StatusCode::CONFLICT, "time_expired"));
    }
    let questions = load_questions(conn, &assessment_id)?;
    let answers = clean_answers(raw, &questions);
    conn.execute("UPDATE attempts SET answers = ?1, saved_at = ?2 WHERE id = ?3", params![serde_json::to_string(&answers).map_err(db_err)?, now, attempt_id]).map_err(db_err)?;
    Ok(now)
}

const MAX_TAB_LEAVES: i64 = 1000;

/// Counts a tab/focus loss. Informational only: nothing is blocked or ended (shown to graders/admins).
pub(crate) fn record_event(conn: &Connection, student: &User, attempt_id: &str, kind: &str) -> Res<()> {
    if kind != "tab_leave" {
        return Err(bad("invalid_event"));
    }
    let n = conn
        .execute(
            "UPDATE attempts SET tab_leaves = tab_leaves + 1 WHERE id = ?1 AND student_id = ?2 AND status = 'in_progress' AND tab_leaves < ?3",
            params![attempt_id, student.id, MAX_TAB_LEAVES],
        )
        .map_err(db_err)?;
    if n == 0 {
        // distinguish "not yours / finished" from "cap reached" without leaking other students' attempts
        let owned: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM attempts WHERE id = ?1 AND student_id = ?2)", params![attempt_id, student.id], |r| r.get(0)).map_err(db_err)?;
        if !owned {
            return Err(err(StatusCode::NOT_FOUND, "not_found"));
        }
    }
    Ok(())
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
    pub(crate) score: f64,
    total: f64,
    pending: i64,
    started_at: i64,
    submitted_at: Option<i64>,
    show_answers: bool,
    items: Option<Vec<ItemResult>>,
    /// False while a student must wait for the exam to close (release mode `after_close`): score and
    /// details are withheld by the server until then.
    released: bool,
    release_at: Option<i64>,
    pass_mark: Option<f64>,
    /// Pass/fail once fully graded and released; `None` when there is no pass mark or grading is pending.
    passed: Option<bool>,
    /// Tab/focus losses during the attempt; only graders and admins see it.
    tab_leaves: Option<i64>,
}

/// Results are visible at once for `immediate` exams; for `after_close` only once the exam is closed
/// (manually, or its closing time has passed).
pub(crate) fn is_released(release_mode: &str, status: &str, closes_at: Option<i64>, now: i64) -> bool {
    release_mode != "after_close" || matches!(status, "closed" | "archived") || closes_at.map_or(false, |c| c <= now)
}

pub(crate) fn passed_flag(score: f64, total: f64, pending: i64, pass_mark: Option<f64>) -> Option<bool> {
    let pm = pass_mark?;
    if pending > 0 || total <= 0.0 {
        return None;
    }
    Some(score / total * 100.0 + 1e-9 >= pm)
}

fn clean_answers(raw: &HashMap<String, String>, questions: &[Question]) -> HashMap<String, String> {
    let ids: HashSet<&str> = questions.iter().map(|q| q.id.as_str()).collect();
    raw.iter()
        .filter(|(k, _)| ids.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.chars().take(2000).collect()))
        .collect()
}

pub(crate) fn submit_attempt(conn: &Connection, student: &User, attempt_id: &str, raw: &HashMap<String, String>, now: i64) -> Res<AttemptResult> {
    let (assessment_id, started, status, order_raw): (String, i64, String, Option<String>) = conn
        .query_row(
            "SELECT assessment_id, started_at, status, order_json FROM attempts WHERE id = ?1 AND student_id = ?2",
            params![attempt_id, student.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if status != "in_progress" {
        return Err(err(StatusCode::CONFLICT, if status == "expired" { "time_expired" } else { "already_submitted" }));
    }
    let info = get_info(conn, &student.id, &assessment_id)?;
    if now > deadline(started, info.duration_min) + GRACE_MS {
        // too late for a manual submit; whatever was autosaved still counts
        settle_if_due(conn, attempt_id, &assessment_id, info.duration_min, now)?;
        return Err(err(StatusCode::CONFLICT, "time_expired"));
    }
    finalize_attempt(conn, attempt_id, &assessment_id, raw, order_raw, now)?;
    attempt_result(conn, student, attempt_id)
}

/// Builds the result view. Students only see per-question detail when the assessment allows it;
/// the owner/admin always see everything.
fn attempt_result(conn: &Connection, viewer: &User, attempt_id: &str) -> Res<AttemptResult> {
    let row = conn
        .query_row(
            "SELECT t.assessment_id, t.student_id, t.status, t.score, t.pending, t.started_at, t.submitted_at, t.answers, t.results,
                    a.title, a.total_points, a.show_answers, a.teacher_id, u.full_name, a.release_mode, a.status, a.closes_at, a.pass_mark, t.tab_leaves
             FROM attempts t JOIN assessments a ON a.id = t.assessment_id JOIN users u ON u.id = t.student_id WHERE t.id = ?1",
            params![attempt_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?, r.get::<_, Option<i64>>(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?, r.get::<_, f64>(10)?, r.get::<_, bool>(11)?, r.get::<_, Option<String>>(12)?, r.get::<_, String>(13)?,
                    r.get::<_, String>(14)?, r.get::<_, String>(15)?, r.get::<_, Option<i64>>(16)?, r.get::<_, Option<f64>>(17)?, r.get::<_, i64>(18)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let (assessment_id, student_id, status, score, pending, started_at, submitted_at, answers_raw, results_raw, title, total, show, teacher_id, student_name, release_mode, exam_status, closes_at, pass_mark, tab_leaves) = row;
    let is_owner = teacher_id.as_deref() == Some(viewer.id.as_str()) || viewer.role == "admin" || (viewer.role == "teacher" && teacher_id.is_none() && can_grade(conn, viewer, &assessment_id)?);
    if student_id != viewer.id && !is_owner {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    if status == "in_progress" {
        return Err(err(StatusCode::CONFLICT, "in_progress"));
    }
    let released = is_owner || is_released(&release_mode, &exam_status, closes_at, now_ms());
    let detail = (is_owner || show) && released;
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
        status: status.clone(),
        // a student who must wait for the exam to close learns nothing about the outcome yet
        score: if released { score } else { 0.0 },
        total,
        pending: if released { pending } else { 0 },
        started_at,
        submitted_at,
        show_answers: show,
        items,
        released,
        release_at: if released { None } else { closes_at },
        pass_mark,
        passed: if released && status == "submitted" { passed_flag(score, total, pending, pass_mark) } else { None },
        tab_leaves: is_owner.then_some(tab_leaves),
    })
}

// ───────── teacher: grading & results ─────────

#[derive(Deserialize)]
pub struct GradeReq {
    pub(crate) grades: HashMap<String, f64>,
}

pub(crate) fn grade_attempt(conn: &Connection, grader: &User, attempt_id: &str, g: &GradeReq) -> Res<AttemptResult> {
    let (assessment_id, student_id, status, results_raw, answers_raw): (String, String, String, String, String) = conn
        .query_row("SELECT assessment_id, student_id, status, results, answers FROM attempts WHERE id = ?1", params![attempt_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if !can_grade(conn, grader, &assessment_id)? {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    if status != "submitted" {
        return Err(err(StatusCode::CONFLICT, "not_submitted"));
    }
    let questions = load_questions(conn, &assessment_id)?;
    let answers: HashMap<String, String> = serde_json::from_str(&answers_raw).unwrap_or_default();
    let mut outcomes: Vec<Outcome> = serde_json::from_str(&results_raw).map_err(db_err)?;
    let pending_before = outcomes.iter().filter(|o| o.correct.is_none()).count();
    for (qid, pts) in &g.grades {
        let q = questions.iter().find(|q| &q.id == qid).ok_or_else(|| bad("invalid_question_id"))?;
        let o = outcomes.iter_mut().find(|o| &o.id == qid).ok_or_else(|| bad("invalid_question_id"))?;
        // Only written (short) answers are graded by hand; a grade can be corrected later, but a blank
        // answer stays 0 and objective questions are never overridden.
        let written = answers.get(qid).map_or(false, |a| !a.trim().is_empty());
        if q.qtype != QuestionType::ShortAnswer || !written {
            return Err(bad("not_gradable"));
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
    // tell the student once, when the last pending answer gets its grade (not on later corrections)
    if pending == 0 && pending_before > 0 {
        let (title, mode, exam_status, closes_at): (String, String, String, Option<i64>) = conn
            .query_row("SELECT title, release_mode, status, closes_at FROM assessments WHERE id = ?1", params![assessment_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(db_err)?;
        // While the result is withheld (`after_close`) the student is told when it is released instead (phase 1-8).
        if is_released(&mode, &exam_status, closes_at, now_ms()) {
            crate::platform_engage::notify(conn, &student_id, "assessment_graded", json!({ "title": title }), &format!("/platform/attempts/{attempt_id}"));
        }
    }
    attempt_result(conn, grader, attempt_id)
}

#[derive(Serialize, Debug)]
pub struct AttemptRow {
    attempt_id: String,
    student_name: String,
    status: String,
    score: f64,
    pending: i64,
    submitted_at: Option<i64>,
    tab_leaves: i64,
    /// Pass/fail against the exam's pass mark (`None` when there is none or grading is pending).
    passed: Option<bool>,
}

#[derive(Serialize, Debug)]
pub struct ResultsSummary {
    info: AssessmentInfo,
    submitted: i64,
    average: f64,
    highest: f64,
    lowest: f64,
    /// Number of submitted attempts that reached the pass mark (0 when the exam has none).
    passed: i64,
    attempts: Vec<AttemptRow>,
}

fn results_summary(conn: &Connection, user: &User, id: &str, now: i64) -> Res<ResultsSummary> {
    if !can_grade(conn, user, id)? {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let info = get_info(conn, &user.id, id)?;
    // attempts whose time ran out while the browser was away are settled before the numbers are shown
    settle_assessment(conn, id, info.duration_min, now)?;
    let total = info.total_points;
    let pass_mark = info.pass_mark;
    let attempts = conn
        .prepare(
            "SELECT t.id, u.full_name, t.status, t.score, t.pending, t.submitted_at, t.tab_leaves FROM attempts t JOIN users u ON u.id = t.student_id
             WHERE t.assessment_id = ?1 AND t.status <> 'in_progress' ORDER BY t.submitted_at DESC, t.started_at DESC LIMIT 500",
        )
        .map_err(db_err)?
        .query_map(params![id], |r| {
            let (status, score, pending): (String, f64, i64) = (r.get(2)?, r.get(3)?, r.get(4)?);
            Ok(AttemptRow {
                attempt_id: r.get(0)?,
                student_name: r.get(1)?,
                passed: if status == "submitted" { passed_flag(score, total, pending, pass_mark) } else { None },
                status, score, pending, submitted_at: r.get(5)?, tab_leaves: r.get(6)?,
            })
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
        passed: attempts.iter().filter(|a| a.passed == Some(true)).count() as i64,
        attempts,
    })
}

// ───────── lists ─────────

fn list_for_subject(conn: &Connection, student_id: &str, subject_id: &str) -> Res<Vec<AssessmentInfo>> {
    conn.prepare(&format!(
        "{INFO_SELECT} WHERE a.subject_id = ?2 AND a.status = 'published' AND s.is_active = 1 AND {TEACHER_OK}
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
pub(crate) fn list_available(conn: &Connection, student_id: &str) -> Res<Vec<AssessmentInfo>> {
    conn.prepare(&format!(
        "{INFO_SELECT} WHERE a.status = 'published' AND s.is_active = 1 AND {TEACHER_OK}
           AND EXISTS(SELECT 1 FROM subject_enrollments e WHERE e.student_id = ?1 AND e.subject_id = a.subject_id)
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
    /// False while the score is withheld until the exam closes.
    released: bool,
    passed: Option<bool>,
}

fn my_attempts(conn: &Connection, student_id: &str, assessment_id: Option<&str>) -> Res<Vec<MyAttempt>> {
    let now = now_ms();
    conn.prepare(
        "SELECT t.id, a.id, a.title, t.status, t.score, a.total_points, t.pending, t.started_at, a.release_mode, a.status, a.closes_at, a.pass_mark FROM attempts t
         JOIN assessments a ON a.id = t.assessment_id WHERE t.student_id = ?1 AND (?2 IS NULL OR a.id = ?2) ORDER BY t.started_at DESC LIMIT 50",
    )
    .map_err(db_err)?
    .query_map(params![student_id, assessment_id], |r| {
        let (status, score, total, pending): (String, f64, f64, i64) = (r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?);
        let released = is_released(&r.get::<_, String>(8)?, &r.get::<_, String>(9)?, r.get(10)?, now);
        let pass_mark: Option<f64> = r.get(11)?;
        Ok(MyAttempt {
            attempt_id: r.get(0)?,
            assessment_id: r.get(1)?,
            title: r.get(2)?,
            passed: if released && status == "submitted" { passed_flag(score, total, pending, pass_mark) } else { None },
            status,
            score: if released { score } else { 0.0 },
            total,
            pending: if released { pending } else { 0 },
            started_at: r.get(7)?,
            released,
        })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

// ───────── handlers ─────────

pub async fn create_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<AssessmentReq>) -> Res<(StatusCode, Json<AssessmentInfo>)> {
    let u = require_role(&s, &h, "teacher")?;
    create_checked(&*lock(&s)?, &s.platform.crypto, &u, &r).map(|i| (StatusCode::CREATED, Json(i)))
}

pub async fn update_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<AssessmentReq>) -> Res<Json<AssessmentInfo>> {
    let u = require_active(&s, &h)?;
    update_checked(&*lock(&s)?, &s.platform.crypto, &u, &id, &r).map(Json)
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
    let is_owner = info.teacher_id.as_deref() == Some(u.id.as_str()) || u.role == "admin" || (u.role == "teacher" && info.teacher_id.is_none() && can_grade(&conn, &u, &id)?);
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
                || (info.status == "published" && used < info.max_attempts && info.opens_at.map_or(true, |o| now >= o) && info.closes_at.map_or(true, |c| now <= c)));
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

#[derive(Deserialize)]
pub struct SaveReq {
    answers: HashMap<String, String>,
}

pub async fn save_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<SaveReq>) -> Res<Json<serde_json::Value>> {
    let u = require_role(&s, &h, "student")?;
    let conn = lock(&s)?;
    // The browser saves about every 30 s and after edits; far more than that is a runaway client.
    crate::platform::rate_limit(&conn, &format!("save:{id}"), 60_000, 30)?;
    let at = save_answers(&conn, &u, &id, &r.answers, now_ms())?;
    Ok(Json(json!({ "saved_at": at })))
}

#[derive(Deserialize)]
pub struct EventReq {
    #[serde(rename = "type")]
    kind: String,
}

pub async fn event_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<EventReq>) -> Res<StatusCode> {
    let u = require_role(&s, &h, "student")?;
    let conn = lock(&s)?;
    crate::platform::rate_limit(&conn, &format!("evt:{id}"), 60_000, 30)?;
    record_event(&conn, &u, &id, &r.kind)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn attempt_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<AttemptResult>> {
    let u = require_active(&s, &h)?;
    attempt_result(&*lock(&s)?, &u, &id).map(Json)
}

pub async fn grade_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(g): Json<GradeReq>) -> Res<Json<AttemptResult>> {
    let u = require_active(&s, &h)?;
    if u.role != "teacher" && u.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    grade_attempt(&*lock(&s)?, &u, &id, &g).map(Json)
}

pub async fn results_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<ResultsSummary>> {
    let u = require_active(&s, &h)?;
    results_summary(&*lock(&s)?, &u, &id, now_ms()).map(Json)
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
        let other_teacher = crate::platform::insert_test_user(&w.conn, "t2@x.com", "teacher", "active");
        assert_eq!(grade_attempt(&w.conn, &other_teacher, &start.attempt_id, &GradeReq { grades: ans_f(&[("q5", 3.0)]) }).unwrap_err().0, StatusCode::FORBIDDEN, "another teacher cannot grade");
        assert_eq!(grade_attempt(&w.conn, &w.teacher, &start.attempt_id, &GradeReq { grades: ans_f(&[("q5", 9.0)]) }).unwrap_err().0, StatusCode::BAD_REQUEST, "above max");
        assert_eq!(grade_attempt(&w.conn, &w.teacher, &start.attempt_id, &GradeReq { grades: ans_f(&[("q1", 0.0)]) }).unwrap_err().0, StatusCode::BAD_REQUEST, "auto-graded cannot be overridden");
        let g = grade_attempt(&w.conn, &w.teacher, &start.attempt_id, &GradeReq { grades: ans_f(&[("q5", 3.0)]) }).unwrap();
        assert_eq!((g.score, g.pending), (8.0, 0));
        let n: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'assessment_graded'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let s = results_summary(&w.conn, &w.teacher, &w.id, now_ms()).unwrap();
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
        // phase 3-1: deleting would cascade every attempt and grade away, so it is refused while attempts exist
        // (this used to delete them; the dedicated rules are covered in `safety_tests`)
        assert_eq!(delete_assessment(&w.conn, &w.teacher, &w.id).unwrap_err().0, StatusCode::CONFLICT);
        let n: i64 = w.conn.query_row("SELECT count(*) FROM attempts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "the attempt survived the refused delete");
    }

    // ───── phase 3-1: the teachers.can_create_exams switch and exam safety ─────

    fn crypto() -> crate::platform_settings::Crypto {
        crate::platform_settings::Crypto::for_tests()
    }

    fn switch(w: &W, on: bool) {
        let req: crate::platform_ai::SettingsReq = serde_json::from_value(json!({ "teachers_can_create_exams": on })).unwrap();
        crate::platform_ai::apply_settings(&w.conn, &crypto(), &w.admin.id, &req).unwrap();
    }

    fn exam_req(status: &str) -> AssessmentReq {
        AssessmentReq {
            subject_id: Some("s1".into()), title: Some("امتحان جديد".into()), description: None, questions: Some(questions()), duration_min: None,
            opens_at: None, closes_at: None, max_attempts: Some(1), show_answers: None, status: Some(status.into()), clear_duration: None, clear_window: None,
        }
    }

    fn status_patch(status: &str) -> AssessmentReq {
        AssessmentReq {
            subject_id: None, title: None, description: None, questions: None, duration_min: None, opens_at: None, closes_at: None,
            max_attempts: None, show_answers: None, status: Some(status.into()), clear_duration: None, clear_window: None,
        }
    }

    fn code(e: &crate::relay::Err) -> String {
        serde_json::from_str::<serde_json::Value>(&e.1).unwrap()["error"].as_str().unwrap_or("").to_string()
    }

    fn status_of(w: &W, id: &str) -> String {
        w.conn.query_row("SELECT status FROM assessments WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
    }

    fn announcements(w: &W) -> i64 {
        w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = 'new_assessment'", params![w.student.id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn an_unset_switch_means_teachers_may_create_and_publish() {
        let w = world(true, None, 1);
        assert!(crate::platform_ai::exams_enabled(&w.conn, &crypto()).unwrap(), "nothing saved = enabled");
        let made = create_checked(&w.conn, &crypto(), &w.teacher, &exam_req("draft")).unwrap();
        assert_eq!(update_checked(&w.conn, &crypto(), &w.teacher, &made.id, &status_patch("published")).unwrap().status, "published");
        switch(&w, true);
        create_checked(&w.conn, &crypto(), &w.teacher, &exam_req("published")).expect("an explicit on works too");
    }

    #[test]
    fn switching_exams_off_blocks_creating_and_publishing_with_exams_disabled() {
        let w = world(true, None, 1);
        let draft = create_checked(&w.conn, &crypto(), &w.teacher, &exam_req("draft")).unwrap().id;
        switch(&w, false);
        let before: i64 = w.conn.query_row("SELECT count(*) FROM assessments", [], |r| r.get(0)).unwrap();
        for status in ["draft", "published"] {
            let e = create_checked(&w.conn, &crypto(), &w.teacher, &exam_req(status)).unwrap_err();
            assert_eq!((e.0, code(&e).as_str()), (StatusCode::FORBIDDEN, "exams_disabled"), "creating as {status}");
        }
        let after: i64 = w.conn.query_row("SELECT count(*) FROM assessments", [], |r| r.get(0)).unwrap();
        assert_eq!(after, before, "nothing was created");
        // moving a draft to published is refused too — including by an admin using the legacy route
        for who in [&w.teacher, &w.admin] {
            let e = update_checked(&w.conn, &crypto(), who, &draft, &status_patch("published")).unwrap_err();
            assert_eq!((e.0, code(&e).as_str()), (StatusCode::FORBIDDEN, "exams_disabled"), "publishing as {}", who.role);
        }
        assert_eq!(status_of(&w, &draft), "draft");
        // back on: both work again
        switch(&w, true);
        create_checked(&w.conn, &crypto(), &w.teacher, &exam_req("published")).unwrap();
        assert_eq!(update_checked(&w.conn, &crypto(), &w.teacher, &draft, &status_patch("published")).unwrap().status, "published");
    }

    #[test]
    fn switching_exams_off_leaves_existing_exams_manageable() {
        let w = world(true, None, 1);
        let draft = create_checked(&w.conn, &crypto(), &w.teacher, &exam_req("draft")).unwrap().id;
        let second = create_checked(&w.conn, &crypto(), &w.teacher, &exam_req("published")).unwrap().id;
        switch(&w, false);
        // a published exam keeps working: edits (even when the editor re-sends status "published") and students taking it
        let edit = AssessmentReq { title: Some("عنوان جديد".into()), ..status_patch("published") };
        assert_eq!(update_checked(&w.conn, &crypto(), &w.teacher, &w.id, &edit).unwrap().title, "عنوان جديد");
        assert!(start_attempt(&w.conn, &w.student, &w.id, now_ms()).is_ok(), "students can still take a published exam");
        // drafts stay editable and deletable
        let edit_draft = AssessmentReq { title: Some("مسودة معدلة".into()), ..status_patch("draft") };
        assert_eq!(update_checked(&w.conn, &crypto(), &w.teacher, &draft, &edit_draft).unwrap().title, "مسودة معدلة");
        delete_assessment(&w.conn, &w.teacher, &draft).unwrap();
        // and an admin can still moderate (unpublish) a teacher's exam that has no attempts
        assert_eq!(update_checked(&w.conn, &crypto(), &w.admin, &second, &status_patch("draft")).unwrap().status, "draft");
    }

    #[test]
    fn an_exam_that_students_have_sat_can_never_be_deleted() {
        let w = world(true, None, 3);
        let now = now_ms();
        // without attempts: the owner can delete (audited with the attempt count) and so can an admin
        let spare = create_assessment(&w.conn, &w.teacher, &exam_req("draft")).unwrap().id;
        delete_assessment(&w.conn, &w.teacher, &spare).unwrap();
        let detail: String = w.conn.query_row("SELECT detail FROM audit_log WHERE action = 'assessment_deleted' AND target_id = ?1", params![spare], |r| r.get(0)).unwrap();
        assert_eq!(detail, "attempts=0");
        let spare2 = create_assessment(&w.conn, &w.teacher, &exam_req("draft")).unwrap().id;
        delete_assessment(&w.conn, &w.admin, &spare2).unwrap();
        // an attempt in progress already counts
        let st = start_attempt(&w.conn, &w.student, &w.id, now).unwrap();
        for who in [&w.teacher, &w.admin] {
            let e = delete_assessment(&w.conn, who, &w.id).unwrap_err();
            assert_eq!((e.0, code(&e).as_str()), (StatusCode::CONFLICT, "has_attempts"), "{} / in progress", who.role);
        }
        // and so does a submitted one
        submit_attempt(&w.conn, &w.student, &st.attempt_id, &ans(&[("q1", "B")]), now + 1000).unwrap();
        for who in [&w.teacher, &w.admin] {
            let e = delete_assessment(&w.conn, who, &w.id).unwrap_err();
            assert_eq!((e.0, code(&e).as_str()), (StatusCode::CONFLICT, "has_attempts"), "{} / submitted", who.role);
        }
        let (exams, attempts): (i64, i64) = w.conn.query_row("SELECT (SELECT count(*) FROM assessments WHERE id = ?1), (SELECT count(*) FROM attempts)", params![w.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((exams, attempts), (1, 1), "the exam, its attempt and the grade are all still there");
        let refused: i64 = w.conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'assessment_deleted' AND target_id = ?1", params![w.id], |r| r.get(0)).unwrap();
        assert_eq!(refused, 0, "a refused delete is not recorded as a deletion");
        // someone who owns nothing still gets 403, not a hint about attempts
        let outsider = insert_test_user(&w.conn, "o@x.com", "teacher", "active");
        assert_eq!(delete_assessment(&w.conn, &outsider, &w.id).unwrap_err().0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn only_the_first_publication_announces_and_published_at_is_never_overwritten() {
        let w = world(true, None, 1);
        assert_eq!(announcements(&w), 1, "created as published: announced once");
        let stamp = |w: &W| -> Option<i64> { w.conn.query_row("SELECT published_at FROM assessments WHERE id = ?1", params![w.id], |r| r.get(0)).unwrap() };
        let first = stamp(&w).expect("published_at is set by the announcement");
        for round in 0..3 {
            assert_eq!(update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("draft")).unwrap().status, "draft");
            assert_eq!(update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("published")).unwrap().status, "published");
            assert_eq!(announcements(&w), 1, "round {round}: publishing again does not notify again");
            assert_eq!(stamp(&w), Some(first), "round {round}: the original publication time is kept");
        }
        // an exam from before published_at existed (NULL) is announced on its next publication — once
        w.conn.execute("UPDATE assessments SET published_at = NULL WHERE id = ?1", params![w.id]).unwrap();
        update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("draft")).unwrap();
        update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("published")).unwrap();
        assert_eq!((announcements(&w), stamp(&w).is_some()), (2, true));
        update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("draft")).unwrap();
        update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("published")).unwrap();
        assert_eq!(announcements(&w), 2);
        // a draft that is published later announces then, once
        let later = create_assessment(&w.conn, &w.teacher, &exam_req("draft")).unwrap().id;
        assert_eq!(announcements(&w), 2, "a draft announces nothing");
        update_assessment(&w.conn, &w.teacher, &later, &status_patch("published")).unwrap();
        assert_eq!(announcements(&w), 3);
    }

    #[test]
    fn an_exam_students_have_started_cannot_be_pulled_back_to_a_draft() {
        let w = world(true, None, 2);
        // no attempts yet: back to draft is fine (and back again)
        update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("draft")).unwrap();
        update_assessment(&w.conn, &w.teacher, &w.id, &status_patch("published")).unwrap();
        start_attempt(&w.conn, &w.student, &w.id, now_ms()).unwrap();
        for who in [&w.teacher, &w.admin] {
            let e = update_assessment(&w.conn, who, &w.id, &status_patch("draft")).unwrap_err();
            assert_eq!((e.0, code(&e).as_str()), (StatusCode::CONFLICT, "has_attempts"), "{}", who.role);
        }
        assert_eq!(status_of(&w, &w.id), "published", "still published");
        // other edits keep working, including a PATCH that re-sends status "published"
        let edit = AssessmentReq { title: Some("تعديل".into()), ..status_patch("published") };
        assert_eq!(update_assessment(&w.conn, &w.teacher, &w.id, &edit).unwrap().title, "تعديل");
        // a draft that already has attempts (older data) can still be edited without 409: nothing changes status
        w.conn.execute("UPDATE assessments SET status = 'draft' WHERE id = ?1", params![w.id]).unwrap();
        let edit = AssessmentReq { title: Some("تعديل آخر".into()), ..status_patch("draft") };
        assert_eq!(update_assessment(&w.conn, &w.teacher, &w.id, &edit).unwrap().status, "draft");
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
        assert!(tables(&c).contains(&"backups".to_string()) && tables(&c).contains(&"exam_reminders".to_string()), "phase 1-8/1-9 tables are created on upgrade");
        assert!(!tables(&c).contains(&"assessments_new".to_string()), "no leftover temp table");
        // phase 1-8: old exams count as announced long ago, the reminder table starts empty, and a sweep over
        // the migrated data neither fails nor invents notifications for finished exams
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM assessments WHERE published_at IS NOT NULL"), 0);
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM exam_reminders"), 0);
        let before = one::<i64>(&c, "SELECT count(*) FROM notifications");
        let swept = crate::platform_reminders::sweep(&c, now_ms());
        assert_eq!(swept, crate::platform_reminders::Sweep::default(), "the fixture's exams are long over");
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM notifications"), before);
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
        // anchored to the fixture's own clock so the old in-progress attempt is not settled as overdue
        let fixture_now: i64 = c.query_row("SELECT max(started_at) FROM attempts", [], |r| r.get(0)).unwrap();
        let r = results_summary(&c, &teacher, &published, fixture_now + 1000).unwrap();
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
        // "now" is anchored to the fixture's own clock (the dump was taken at a fixed moment), so the
        // exam's time limit does not expire the in-progress attempt as real time passes.
        let at: i64 = c.query_row("SELECT started_at FROM attempts WHERE status = 'in_progress'", [], |r| r.get(0)).unwrap();
        let resumed = start_attempt(&c, &s1, &published, at + 1000).unwrap();
        assert!(resumed.resumed, "the in-progress attempt from before the upgrade is resumed");
        assert!(start_attempt(&c, &s2, &published, at + 1000).is_ok());
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

/// Exam-taking behaviour (phase 1-6): per-attempt shuffling, autosave, overdue settling, tab events,
/// result release timing and the pass mark.
#[cfg(test)]
mod taking_tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use rand::{rngs::StdRng, SeedableRng};
    use serde_json::json;

    const NOW: i64 = 1_000_000_000_000;

    struct W {
        conn: Connection,
        admin: User,
        student: User,
    }

    fn world() -> W {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![student.id]).unwrap();
        W { conn, admin, student }
    }

    fn enroll(w: &W, email: &str) -> User {
        let u = insert_test_user(&w.conn, email, "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1,'s1',0)", params![u.id]).unwrap();
        u
    }

    /// 6 questions: 3 single, 1 multi, 1 true/false, 1 short answer. Option texts are "o<q>-<index>"; the
    /// correct original answers are B / AC / A.
    fn qs() -> Vec<serde_json::Value> {
        let opts = |q: usize, n: usize| (0..n).map(|i| format!("o{q}-{i}")).collect::<Vec<_>>();
        vec![
            json!({"id":"q1","type":"single_choice","stem":"س1","options":opts(1,4),"answer":"B","score":1}),
            json!({"id":"q2","type":"single_choice","stem":"س2","options":opts(2,4),"answer":"D","score":1}),
            json!({"id":"q3","type":"single_choice","stem":"س3","options":opts(3,5),"answer":"A","score":1}),
            json!({"id":"q4","type":"multi_choice","stem":"س4","options":opts(4,5),"answer":"AC","score":2}),
            json!({"id":"q5","type":"true_false","stem":"س5","options":["صحيح","خطأ"],"answer":"A","score":1}),
            json!({"id":"q6","type":"short_answer","stem":"س6","answer":"مرجع","score":4}),
        ]
    }

    fn exam(w: &W, extra: serde_json::Value) -> String {
        let mut v = json!({"subject_id":"s1","title":"امتحان","questions":qs(),"status":"published"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        let req: crate::platform_exam_admin::ExamReq = serde_json::from_value(v).unwrap();
        crate::platform_exam_admin::create_exam(&w.conn, &w.admin, &req, NOW).unwrap().info_id()
    }

    fn originals() -> Vec<Question> {
        crate::platform_bank::tests_questions(&qs())
    }

    // ── ordering primitives

    #[test]
    fn no_shuffle_flags_means_no_stored_order_and_identity_mapping() {
        let mut rng = StdRng::seed_from_u64(1);
        assert_eq!(make_order(&originals(), false, false, &mut rng), None);
        let a = ans(&[("q1", "B"), ("q4", "AC")]);
        assert_eq!(to_original(&originals(), None, &a), a);
        assert_eq!(displayed(&originals(), None).iter().map(|q| q.id.clone()).collect::<Vec<_>>(), vec!["q1", "q2", "q3", "q4", "q5", "q6"]);
    }

    fn ans(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn every_dealt_order_is_a_real_permutation_and_leaves_non_choice_questions_alone() {
        let qs = originals();
        for seed in 0..50 {
            let mut rng = StdRng::seed_from_u64(seed);
            let o = make_order(&qs, true, true, &mut rng).unwrap();
            let mut ids = o.q.clone();
            ids.sort();
            assert_eq!(ids, vec!["q1", "q2", "q3", "q4", "q5", "q6"], "each question exactly once");
            assert_eq!(o.o.keys().cloned().collect::<HashSet<_>>(), HashSet::from(["q1".to_string(), "q2".into(), "q3".into(), "q4".into()]), "only choice questions get option permutations");
            for (id, perm) in &o.o {
                let n = qs.iter().find(|q| &q.id == id).unwrap().options.len();
                let mut sorted = perm.clone();
                sorted.sort();
                assert_eq!(sorted, (0..n).collect::<Vec<_>>(), "{id}: a permutation of the option indexes");
            }
            let shown = displayed(&qs, Some(&o));
            assert_eq!(shown.iter().map(|q| q.id.clone()).collect::<Vec<_>>(), o.q);
            let tf = shown.iter().find(|q| q.id == "q5").unwrap();
            assert_eq!(tf.options, vec!["صحيح", "خطأ"], "true/false options keep their order so A/B stay true/false");
        }
        let seeds_differ: HashSet<Vec<String>> = (0..20).map(|s| make_order(&qs, true, false, &mut StdRng::seed_from_u64(s)).unwrap().q).collect();
        assert!(seeds_differ.len() > 10, "different attempts really get different question orders");
        let only_q = make_order(&qs, true, false, &mut StdRng::seed_from_u64(3)).unwrap();
        assert!(only_q.o.is_empty(), "options untouched when only questions are shuffled");
        let only_o = make_order(&qs, false, true, &mut StdRng::seed_from_u64(3)).unwrap();
        assert_eq!(only_o.q, vec!["q1", "q2", "q3", "q4", "q5", "q6"], "questions untouched when only options are shuffled");
    }

    /// The property that makes shuffling safe: whatever a student ticks on the shuffled screen, the
    /// answer mapped back to the original letters selects exactly the same option *texts*.
    #[test]
    fn displayed_answers_map_back_to_the_same_option_texts_for_every_possible_selection() {
        let qs = originals();
        for seed in 0..30 {
            let order = make_order(&qs, true, true, &mut StdRng::seed_from_u64(seed)).unwrap();
            let shown = displayed(&qs, Some(&order));
            for q in qs.iter().filter(|q| is_choice(&q.qtype)) {
                let sq = shown.iter().find(|x| x.id == q.id).unwrap();
                let n = q.options.len();
                for mask in 1u32..(1 << n) {
                    let picked: String = (0..n).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i as u8) as char).collect();
                    if q.qtype == QuestionType::SingleChoice && picked.len() != 1 {
                        continue;
                    }
                    let mapped = to_original(&qs, Some(&order), &ans(&[(&q.id, &picked)]))[&q.id].clone();
                    let shown_texts: HashSet<&String> = picked.chars().map(|c| &sq.options[(c as u8 - b'A') as usize]).collect();
                    let orig_texts: HashSet<&String> = mapped.chars().map(|c| &q.options[(c as u8 - b'A') as usize]).collect();
                    assert_eq!(shown_texts, orig_texts, "seed {seed} {} picked {picked} mapped {mapped}", q.id);
                }
            }
        }
        // out-of-range letters and junk are dropped, duplicates collapsed, lower case accepted
        let order = make_order(&qs, false, true, &mut StdRng::seed_from_u64(9)).unwrap();
        let m = to_original(&qs, Some(&order), &ans(&[("q1", "Zb9b"), ("q2", ""), ("q5", "B")]));
        assert_eq!(m["q1"].len(), 1, "junk and duplicates dropped");
        assert_eq!((m["q2"].as_str(), m["q5"].as_str()), ("", "B"), "empty stays empty, true/false untouched");
    }

    // ── full attempt flow

    /// The displayed letter(s) whose option text equals the original correct option(s).
    fn solve(shown: &[PublicQuestion], original: &[Question], right: bool) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for o in original.iter().filter(|q| is_choice(&q.qtype)) {
            let sq = shown.iter().find(|x| x.id == o.id).unwrap();
            let want: Vec<&String> = o.answer.chars().map(|c| &o.options[(c as u8 - b'A') as usize]).collect();
            let pick: String = sq.options.iter().enumerate().filter(|(_, t)| want.contains(t) == right).map(|(i, _)| (b'A' + i as u8) as char).take(if o.qtype == QuestionType::SingleChoice { 1 } else { 9 }).collect();
            out.insert(o.id.clone(), pick);
        }
        out.insert("q5".into(), if right { "A" } else { "B" }.into());
        out
    }

    #[test]
    fn shuffled_attempts_grade_by_option_text_not_by_screen_position_and_never_leak() {
        let w = world();
        let id = exam(&w, json!({"shuffle_questions": true, "shuffle_options": true}));
        let s2 = enroll(&w, "s2@x.com");
        let a = start_attempt_with(&w.conn, &w.student, &id, NOW, &mut StdRng::seed_from_u64(1)).unwrap();
        let b = start_attempt_with(&w.conn, &s2, &id, NOW, &mut StdRng::seed_from_u64(2)).unwrap();
        let ids = |s: &StartRes| s.questions.iter().map(|q| q.id.clone()).collect::<Vec<_>>();
        assert_ne!(ids(&a), ids(&b), "two students are dealt different question orders");
        assert!(a.questions.iter().zip(&b.questions).any(|(x, y)| x.id == y.id && x.options != y.options) || ids(&a) != ids(&b));
        for s in [&a, &b] {
            let json = serde_json::to_string(&s.questions).unwrap();
            assert!(!json.contains("\"answer\"") && !json.contains("analysis") && !json.contains("مرجع"), "no answers leak: {json}");
        }
        // the attempt reopens in exactly the same layout
        let again = start_attempt_with(&w.conn, &w.student, &id, NOW + 5, &mut StdRng::seed_from_u64(99)).unwrap();
        assert!(again.resumed && ids(&again) == ids(&a) && again.questions.iter().zip(&a.questions).all(|(x, y)| x.options == y.options), "resuming restores the stored order, not a new deal");
        // student A answers everything right (by option text), student B everything wrong
        let right = solve(&a.questions, &originals(), true);
        let mut right_all = right.clone();
        right_all.insert("q6".into(), "إجابتي".into());
        let ra = submit_attempt(&w.conn, &w.student, &a.attempt_id, &right_all, NOW + 10).unwrap();
        assert_eq!((ra.score, ra.pending), (1.0 + 1.0 + 1.0 + 2.0 + 1.0, 1), "every objective question is right regardless of the shuffle");
        let wrong = submit_attempt(&w.conn, &s2, &b.attempt_id, &solve(&b.questions, &originals(), false), NOW + 10).unwrap();
        assert_eq!(wrong.score, 0.0, "the complement of the right options scores nothing");
        // the stored answers are in the original letters, so the review shows them against the original options
        let items = ra.items.unwrap();
        let q4 = items.iter().find(|i| i.id == "q4").unwrap();
        assert_eq!((q4.your_answer.as_deref(), q4.correct_answer.as_deref(), q4.correct), (Some("AC"), Some("AC"), Some(true)));
    }

    #[test]
    fn autosave_resumes_on_any_device_and_is_validated() {
        let w = world();
        let id = exam(&w, json!({"shuffle_options": true, "duration_min": 10}));
        let s = start_attempt_with(&w.conn, &w.student, &id, NOW, &mut StdRng::seed_from_u64(4)).unwrap();
        assert!(s.saved_answers.is_empty() && s.saved_at.is_none());
        let long = "ك".repeat(3000);
        let at = save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "C"), ("q6", &long), ("ghost", "x")]), NOW + 1000).unwrap();
        assert_eq!(at, NOW + 1000);
        let r = start_attempt_with(&w.conn, &w.student, &id, NOW + 2000, &mut StdRng::seed_from_u64(5)).unwrap();
        assert!(r.resumed && r.saved_at == Some(NOW + 1000));
        assert_eq!(r.saved_answers.get("q1").map(String::as_str), Some("C"), "answers are given back in displayed letters");
        assert_eq!(r.saved_answers["q6"].chars().count(), 2000, "over-long text is cut like on submit");
        assert!(!r.saved_answers.contains_key("ghost"), "answers for unknown questions are dropped");
        // latest save wins
        save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "A")]), NOW + 3000).unwrap();
        assert_eq!(start_attempt_with(&w.conn, &w.student, &id, NOW + 4000, &mut StdRng::seed_from_u64(5)).unwrap().saved_answers.len(), 1);
        // not yours / already submitted / unknown
        let other = enroll(&w, "o@x.com");
        assert_eq!(save_answers(&w.conn, &other, &s.attempt_id, &ans(&[]), NOW + 5000).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(save_answers(&w.conn, &w.student, "nope", &ans(&[]), NOW + 5000).unwrap_err().0, StatusCode::NOT_FOUND);
        submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "A")]), NOW + 6000).unwrap();
        assert_eq!(save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[]), NOW + 7000).unwrap_err().0, StatusCode::CONFLICT, "no saving after the attempt ended");
    }

    #[test]
    fn overdue_attempts_are_submitted_from_the_autosave_or_expired_when_nothing_was_saved() {
        let w = world();
        let id = exam(&w, json!({"duration_min": 10, "max_attempts": 3}));
        let late = NOW + 10 * 60_000 + GRACE_MS + 1;
        // 1. saved something, then the browser vanished: the saved answers are graded at the deadline
        let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
        save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B"), ("q2", "A")]), NOW + 60_000).unwrap();
        let next = start_attempt(&w.conn, &w.student, &id, late).unwrap();
        assert!(!next.resumed, "a new attempt starts once the old one is settled");
        let (status, score, at): (String, f64, i64) = w.conn.query_row("SELECT status, score, submitted_at FROM attempts WHERE id = ?1", params![s.attempt_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((status.as_str(), score, at), ("submitted", 1.0, NOW + 10 * 60_000), "q1 right, q2 wrong; stamped at the deadline, not at discovery time");
        // 2. never saved anything: expired with 0, as before
        let gone = start_attempt(&w.conn, &w.student, &id, late + 1).unwrap();
        assert!(gone.resumed);
        let very_late = late + 1 + 10 * 60_000 + GRACE_MS + 1;
        let third = start_attempt(&w.conn, &w.student, &id, very_late).unwrap();
        assert!(!third.resumed);
        let st: String = w.conn.query_row("SELECT status FROM attempts WHERE id = ?1", params![next.attempt_id], |r| r.get(0)).unwrap();
        assert_eq!(st, "expired");
        // 3. a manual submit that is too late fails, but the autosave still counts
        save_answers(&w.conn, &w.student, &third.attempt_id, &ans(&[("q1", "B")]), very_late + 1000).unwrap();
        let too_late = very_late + 10 * 60_000 + GRACE_MS + 1;
        assert_eq!(submit_attempt(&w.conn, &w.student, &third.attempt_id, &ans(&[("q1", "B"), ("q2", "D")]), too_late).unwrap_err().0, StatusCode::CONFLICT);
        let (status, score): (String, f64) = w.conn.query_row("SELECT status, score FROM attempts WHERE id = ?1", params![third.attempt_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((status.as_str(), score), ("submitted", 1.0), "only what was autosaved is graded");
        // saving after the deadline is refused (and settles the attempt)
        let fourth = enroll(&w, "f@x.com");
        let f = start_attempt(&w.conn, &fourth, &id, NOW).unwrap();
        save_answers(&w.conn, &fourth, &f.attempt_id, &ans(&[("q1", "B")]), NOW + 1000).unwrap();
        assert_eq!(save_answers(&w.conn, &fourth, &f.attempt_id, &ans(&[("q1", "B")]), late).unwrap_err().0, StatusCode::CONFLICT);
        assert_eq!(w.conn.query_row::<String, _, _>("SELECT status FROM attempts WHERE id = ?1", params![f.attempt_id], |r| r.get(0)).unwrap(), "submitted");
    }

    #[test]
    fn the_periodic_sweep_and_the_results_page_settle_attempts_nobody_returned_to() {
        let w = world();
        let id = exam(&w, json!({"duration_min": 5}));
        let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
        save_answers(&w.conn, &w.student, &s.attempt_id, &ans(&[("q1", "B")]), NOW + 1).unwrap();
        let s2 = enroll(&w, "s2@x.com");
        let silent = start_attempt(&w.conn, &s2, &id, NOW).unwrap();
        assert_eq!(settle_all(&w.conn, NOW + 4 * 60_000).unwrap(), 0, "not overdue yet");
        let sum = results_summary(&w.conn, &w.admin, &id, NOW + 6 * 60_000 + GRACE_MS + 1).unwrap();
        assert_eq!((sum.submitted, sum.attempts.len()), (1, 2), "the saved attempt counts as submitted, the silent one as expired");
        assert_eq!(settle_all(&w.conn, NOW + 99 * 60_000).unwrap(), 0, "already settled: the sweep is idempotent");
        let _ = silent;
        // the sweep alone does the same for an exam nobody opens
        let s3 = enroll(&w, "s3@x.com");
        start_attempt(&w.conn, &s3, &id, NOW + 100 * 60_000).unwrap();
        assert_eq!(settle_all(&w.conn, NOW + 200 * 60_000).unwrap(), 1);
    }

    // ── integrity events

    #[test]
    fn tab_leaves_are_counted_capped_owned_and_only_shown_to_graders() {
        let w = world();
        let id = exam(&w, json!({}));
        let s = start_attempt(&w.conn, &w.student, &id, NOW).unwrap();
        for _ in 0..3 {
            record_event(&w.conn, &w.student, &s.attempt_id, "tab_leave").unwrap();
        }
        assert_eq!(record_event(&w.conn, &w.student, &s.attempt_id, "screenshot").unwrap_err().0, StatusCode::BAD_REQUEST);
        let other = enroll(&w, "o@x.com");
        assert_eq!(record_event(&w.conn, &other, &s.attempt_id, "tab_leave").unwrap_err().0, StatusCode::NOT_FOUND, "cannot touch another student's attempt");
        assert_eq!(record_event(&w.conn, &w.student, "ghost", "tab_leave").unwrap_err().0, StatusCode::NOT_FOUND);
        w.conn.execute("UPDATE attempts SET tab_leaves = 999 WHERE id = ?1", params![s.attempt_id]).unwrap();
        record_event(&w.conn, &w.student, &s.attempt_id, "tab_leave").unwrap();
        record_event(&w.conn, &w.student, &s.attempt_id, "tab_leave").unwrap();
        let n: i64 = w.conn.query_row("SELECT tab_leaves FROM attempts WHERE id = ?1", params![s.attempt_id], |r| r.get(0)).unwrap();
        assert_eq!(n, MAX_TAB_LEAVES, "capped, and hitting the cap is not an error");
        let r = submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[]), NOW + 10).unwrap();
        assert_eq!(r.tab_leaves, None, "the student never sees the counter");
        record_event(&w.conn, &w.student, &s.attempt_id, "tab_leave").unwrap();
        let after: i64 = w.conn.query_row("SELECT tab_leaves FROM attempts WHERE id = ?1", params![s.attempt_id], |r| r.get(0)).unwrap();
        assert_eq!(after, MAX_TAB_LEAVES, "events after the attempt ended are ignored");
        assert_eq!(attempt_result(&w.conn, &w.admin, &s.attempt_id).unwrap().tab_leaves, Some(MAX_TAB_LEAVES), "the admin sees it");
        let row = &results_summary(&w.conn, &w.admin, &id, NOW + 20).unwrap().attempts[0];
        assert_eq!(row.tab_leaves, MAX_TAB_LEAVES);
    }

    // ── release timing & pass mark

    #[test]
    fn results_wait_for_the_close_when_release_is_after_close_and_the_server_withholds_them() {
        let w = world();
        let soon = now_ms() + 3_600_000;
        let id = exam(&w, json!({"release_mode": "after_close", "closes_at": soon, "pass_mark": 50}));
        let s = start_attempt(&w.conn, &w.student, &id, now_ms()).unwrap();
        let r = submit_attempt(&w.conn, &w.student, &s.attempt_id, &solve(&s.questions, &originals(), true), now_ms()).unwrap();
        assert!(!r.released && r.release_at == Some(soon));
        assert_eq!((r.score, r.pending, r.passed), (0.0, 0, None), "no score, no pending count, no pass/fail while embargoed");
        assert!(r.items.is_none(), "no per-question details either");
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("correct_answer") && !json.contains("\"score\":6"), "{json}");
        let mine = my_attempts(&w.conn, &w.student.id, None).unwrap();
        assert_eq!((mine[0].released, mine[0].score, mine[0].passed), (false, 0.0, None), "the attempts list is gated too");
        let admin_view = attempt_result(&w.conn, &w.admin, &s.attempt_id).unwrap();
        assert!(admin_view.released && admin_view.score == 6.0 && admin_view.items.is_some(), "graders always see everything");
        // closing the exam releases the results
        crate::platform_exam_admin::act(&w.conn, &w.admin, &id, "close", now_ms()).unwrap();
        let after = attempt_result(&w.conn, &w.student, &s.attempt_id).unwrap();
        assert!(after.released && after.release_at.is_none() && after.score == 6.0 && after.items.is_some());
        assert!(my_attempts(&w.conn, &w.student.id, None).unwrap()[0].released);
        // a closing time in the past releases them as well, even if nobody pressed "close"
        let id2 = exam(&w, json!({"title": "ثان", "release_mode": "after_close", "closes_at": soon, "max_attempts": 2}));
        let s2 = start_attempt(&w.conn, &w.student, &id2, now_ms()).unwrap();
        let r2 = submit_attempt(&w.conn, &w.student, &s2.attempt_id, &ans(&[]), now_ms()).unwrap();
        assert!(!r2.released);
        w.conn.execute("UPDATE assessments SET closes_at = 1 WHERE id = ?1", params![id2]).unwrap();
        assert!(attempt_result(&w.conn, &w.student, &s2.attempt_id).unwrap().released, "closes_at passed");
        // immediate exams are never embargoed
        let id3 = exam(&w, json!({"title": "فوري"}));
        let s3 = start_attempt(&w.conn, &w.student, &id3, now_ms()).unwrap();
        assert!(submit_attempt(&w.conn, &w.student, &s3.attempt_id, &ans(&[]), now_ms()).unwrap().released);
    }

    #[test]
    fn pass_mark_is_a_percentage_of_the_total_and_waits_for_manual_grading() {
        let w = world();
        // total 10 points: 1+1+1+2+1 objective, 4 for the short answer. 60% = 6 points.
        let id = exam(&w, json!({"pass_mark": 60, "max_attempts": 5}));
        let attempt = |answers: HashMap<String, String>, who: &User| {
            let s = start_attempt(&w.conn, who, &id, now_ms()).unwrap();
            submit_attempt(&w.conn, who, &s.attempt_id, &answers, now_ms()).unwrap()
        };
        let s2 = enroll(&w, "s2@x.com");
        let full = attempt(ans(&[("q1", "B"), ("q2", "D"), ("q3", "A"), ("q4", "AC"), ("q5", "A"), ("q6", "كتبت إجابة")]), &s2);
        assert_eq!((full.score, full.pending, full.passed), (6.0, 1, None), "pending short answer → the outcome is not decided yet");
        let g = grade_attempt(&w.conn, &w.admin, &full.attempt_id, &GradeReq { grades: [("q6".to_string(), 0.0)].into() }).unwrap();
        assert_eq!((g.score, g.passed), (6.0, Some(true)), "exactly 60% passes (the mark is inclusive)");
        let s3 = enroll(&w, "s3@x.com");
        let low = attempt(ans(&[("q1", "B"), ("q2", "D"), ("q3", "A"), ("q4", "A"), ("q6", "إجابة ضعيفة")]), &s3);
        let low = grade_attempt(&w.conn, &w.admin, &low.attempt_id, &GradeReq { grades: [("q6".to_string(), 0.0)].into() }).unwrap();
        assert_eq!((low.score, low.passed), (3.0, Some(false)));
        let sum = results_summary(&w.conn, &w.admin, &id, now_ms()).unwrap();
        assert_eq!((sum.submitted, sum.passed), (2, 1), "the summary counts passers");
        assert!(sum.attempts.iter().any(|r| r.passed == Some(false)) && sum.attempts.iter().any(|r| r.passed == Some(true)));
        // no pass mark → never a verdict
        let id2 = exam(&w, json!({"title": "بلا نجاح"}));
        let s = start_attempt(&w.conn, &w.student, &id2, now_ms()).unwrap();
        assert_eq!(submit_attempt(&w.conn, &w.student, &s.attempt_id, &ans(&[]), now_ms()).unwrap().passed, None);
        assert_eq!(passed_flag(5.0, 0.0, 0, Some(50.0)), None, "an exam worth 0 points has no percentage");
    }
}
