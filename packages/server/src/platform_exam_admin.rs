//! Admin exam builder backend: create/edit exams with the full 1-5 settings, lifecycle
//! (draft → published → closed → archived), duplication and guarded deletion.
//!
//! Exams created here have no owning teacher (`teacher_id` NULL, `created_by` = the admin). An exam
//! is a fixed *snapshot* of its questions: nothing here references the question bank, and a bank
//! item's id is only remembered in `source_map` for statistics. Once the first attempt exists the
//! questions, scores and fairness settings are frozen (PRD E6); an admin may moderate (close,
//! archive, delete) a teacher's exam but never rewrite it.

use crate::platform::{audit, bad, db_err, lock, new_id, opt_text, require_admin, text, Res, User};
use crate::platform_bank::clean_exam_question;
use crate::platform_exams::{announce, check_duration, check_window, get_info, map_info, points_of, round2, AssessmentInfo, INFO_SELECT, MAX_QUESTIONS};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use exameow_core::exam::Question;
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const COPY_SUFFIX: &str = " (نسخة)";

// ───────── settings ─────────

/// The validated, effective settings of an exam.
#[derive(Clone, Debug)]
struct Cfg {
    duration_min: Option<i64>,
    opens_at: Option<i64>,
    closes_at: Option<i64>,
    max_attempts: i64,
    show_answers: bool,
    shuffle_questions: bool,
    shuffle_options: bool,
    pass_mark: Option<f64>,
    release_mode: String,
}

impl Cfg {
    fn defaults() -> Cfg {
        Cfg {
            duration_min: None,
            opens_at: None,
            closes_at: None,
            max_attempts: 1,
            show_answers: true,
            shuffle_questions: false,
            shuffle_options: false,
            pass_mark: None,
            release_mode: "immediate".into(),
        }
    }
    fn of(i: &AssessmentInfo) -> Cfg {
        Cfg {
            duration_min: i.duration_min,
            opens_at: i.opens_at,
            closes_at: i.closes_at,
            max_attempts: i.max_attempts,
            show_answers: i.show_answers,
            shuffle_questions: i.shuffle_questions,
            shuffle_options: i.shuffle_options,
            pass_mark: i.pass_mark,
            release_mode: i.release_mode.clone(),
        }
    }
}

#[derive(Deserialize, Default, Clone)]
pub struct ExamReq {
    subject_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    questions: Option<Vec<Question>>,
    /// Question id → bank item id it was copied from (statistics only).
    sources: Option<HashMap<String, String>>,
    duration_min: Option<i64>,
    clear_duration: Option<bool>,
    opens_at: Option<i64>,
    clear_opens: Option<bool>,
    closes_at: Option<i64>,
    clear_closes: Option<bool>,
    max_attempts: Option<i64>,
    show_answers: Option<bool>,
    shuffle_questions: Option<bool>,
    shuffle_options: Option<bool>,
    /// Percentage of the total points (1–100).
    pass_mark: Option<f64>,
    clear_pass_mark: Option<bool>,
    /// `immediate` or `after_close`.
    release_mode: Option<String>,
    /// Create only: `draft` (default) or `published`. Later changes go through the lifecycle actions.
    status: Option<String>,
}

/// Applies the request on top of `base`, validating every field.
fn merge(base: &Cfg, r: &ExamReq) -> Res<Cfg> {
    let mut c = base.clone();
    if r.clear_duration == Some(true) {
        c.duration_min = None;
    } else if r.duration_min.is_some() {
        c.duration_min = r.duration_min;
    }
    if r.clear_opens == Some(true) {
        c.opens_at = None;
    } else if r.opens_at.is_some() {
        c.opens_at = r.opens_at;
    }
    if r.clear_closes == Some(true) {
        c.closes_at = None;
    } else if r.closes_at.is_some() {
        c.closes_at = r.closes_at;
    }
    if let Some(n) = r.max_attempts {
        c.max_attempts = n;
    }
    if let Some(b) = r.show_answers {
        c.show_answers = b;
    }
    if let Some(b) = r.shuffle_questions {
        c.shuffle_questions = b;
    }
    if let Some(b) = r.shuffle_options {
        c.shuffle_options = b;
    }
    if r.clear_pass_mark == Some(true) {
        c.pass_mark = None;
    } else if r.pass_mark.is_some() {
        c.pass_mark = r.pass_mark;
    }
    if let Some(m) = &r.release_mode {
        c.release_mode = m.clone();
    }
    check_duration(c.duration_min)?;
    check_window(c.opens_at, c.closes_at)?;
    if !(1..=10).contains(&c.max_attempts) {
        return Err(bad("invalid_attempts"));
    }
    if c.pass_mark.map_or(false, |p| !p.is_finite() || !(1.0..=100.0).contains(&p)) {
        return Err(bad("invalid_pass_mark"));
    }
    if !["immediate", "after_close"].contains(&c.release_mode.as_str()) {
        return Err(bad("invalid_release"));
    }
    // "after the exam closes" needs a closing time to wait for
    if c.release_mode == "after_close" && c.closes_at.is_none() {
        return Err(bad("release_needs_close"));
    }
    Ok(c)
}

fn with_question(e: crate::relay::Err, q: &Question, index: usize) -> crate::relay::Err {
    let mut v: serde_json::Value = serde_json::from_str(&e.1).unwrap_or_else(|_| serde_json::json!({ "error": "invalid_question" }));
    v["question_id"] = serde_json::Value::String(q.id.clone());
    v["index"] = serde_json::Value::from(index);
    (e.0, v.to_string())
}

/// Cleans and checks the exam's questions (ids unique, shapes valid). Empty is fine for a draft only.
fn clean_questions(qs: &[Question]) -> Res<Vec<Question>> {
    if qs.len() > MAX_QUESTIONS {
        return Err(bad("invalid_question_count"));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(qs.len());
    for (i, q) in qs.iter().enumerate() {
        let c = clean_exam_question(q).map_err(|e| with_question(e, q, i))?;
        if c.id.is_empty() || c.id.chars().count() > 100 || !seen.insert(c.id.clone()) {
            return Err(with_question(bad("invalid_question_id"), q, i));
        }
        out.push(c);
    }
    Ok(out)
}

fn clean_sources(sources: &Option<HashMap<String, String>>, questions: &[Question]) -> Res<HashMap<String, String>> {
    let ids: HashSet<&str> = questions.iter().map(|q| q.id.as_str()).collect();
    let mut out = HashMap::new();
    for (qid, bank) in sources.iter().flatten() {
        if !ids.contains(qid.as_str()) || bank.is_empty() || bank.chars().count() > 100 {
            continue; // stale entries (a removed question) are dropped, not an error
        }
        out.insert(qid.clone(), bank.clone());
    }
    Ok(out)
}

fn subject_state(conn: &Connection, id: &str) -> Res<bool> {
    conn.query_row("SELECT is_active FROM subjects WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn total_of(qs: &[Question]) -> f64 {
    round2(qs.iter().map(points_of).sum())
}

// ───────── views ─────────

#[derive(Serialize, Debug)]
pub struct ExamRow {
    #[serde(flatten)]
    info: AssessmentInfo,
    /// Effective lifecycle phase: a published exam past its closing time reads as `closed`.
    phase: String,
    /// Admin-created exams can be edited; teachers' exams only moderated.
    can_edit: bool,
}

impl ExamDetail {
    /// The exam id (used by cross-module tests).
    #[cfg(test)]
    pub(crate) fn info_id(&self) -> String {
        self.row.info.id.clone()
    }
}

#[derive(Serialize, Debug)]
pub struct ExamDetail {
    #[serde(flatten)]
    row: ExamRow,
    questions: Vec<Question>,
    sources: HashMap<String, String>,
}

fn phase_of(i: &AssessmentInfo, now: i64) -> String {
    if i.status == "published" && i.closes_at.map_or(false, |c| c <= now) {
        "closed".into()
    } else {
        i.status.clone()
    }
}

fn row_of(i: AssessmentInfo, now: i64) -> ExamRow {
    ExamRow { phase: phase_of(&i, now), can_edit: i.teacher_id.is_none(), info: i }
}

fn load_snapshot(conn: &Connection, id: &str) -> Res<(Vec<Question>, HashMap<String, String>)> {
    let (q, s): (String, Option<String>) = conn
        .query_row("SELECT questions, source_map FROM assessments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(db_err)?;
    Ok((serde_json::from_str(&q).map_err(db_err)?, s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()))
}

pub fn get_exam(conn: &Connection, admin: &User, id: &str, now: i64) -> Res<ExamDetail> {
    let info = get_info(conn, &admin.id, id)?;
    let (questions, sources) = load_snapshot(conn, id)?;
    Ok(ExamDetail { row: row_of(info, now), questions, sources })
}

// ───────── create / update ─────────

pub fn create_exam(conn: &Connection, admin: &User, r: &ExamReq, now: i64) -> Res<ExamDetail> {
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    let subject_active = subject_state(conn, subject_id)?;
    let title = text(r.title.as_deref().unwrap_or(""), 200, "invalid_title")?;
    let description = opt_text(&r.description, 2000, "description_too_long")?;
    let questions = clean_questions(r.questions.as_deref().unwrap_or(&[]))?;
    let sources = clean_sources(&r.sources, &questions)?;
    let cfg = merge(&Cfg::defaults(), r)?;
    let status = r.status.as_deref().unwrap_or("draft");
    if !["draft", "published"].contains(&status) {
        return Err(bad("invalid_status"));
    }
    if status == "published" {
        check_publishable(&questions, subject_active, &cfg, now)?;
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO assessments(id, teacher_id, created_by, subject_id, title, description, questions, question_count, total_points, duration_min,
           opens_at, closes_at, max_attempts, show_answers, shuffle_questions, shuffle_options, pass_mark, release_mode, status, source_map, created_at, updated_at)
         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?20)",
        params![
            id, admin.id, subject_id, title, description, serde_json::to_string(&questions).map_err(db_err)?, questions.len() as i64, total_of(&questions),
            cfg.duration_min, cfg.opens_at, cfg.closes_at, cfg.max_attempts, cfg.show_answers, cfg.shuffle_questions, cfg.shuffle_options, cfg.pass_mark,
            cfg.release_mode, status, serde_json::to_string(&sources).map_err(db_err)?, now
        ],
    )
    .map_err(db_err)?;
    audit(conn, &admin.id, &id, "exam_create", status);
    if status == "published" {
        audit(conn, &admin.id, &id, "exam_publish", "on create");
        announce(conn, admin, subject_id, &id, &title);
    }
    get_exam(conn, admin, &id, now)
}

/// What publishing requires: at least one question, an active subject and a closing time still ahead.
fn check_publishable(questions: &[Question], subject_active: bool, cfg: &Cfg, now: i64) -> Res<()> {
    if questions.is_empty() {
        return Err(bad("invalid_question_count"));
    }
    if !subject_active {
        return Err(bad("subject_inactive"));
    }
    if cfg.closes_at.map_or(false, |c| c <= now) {
        return Err(bad("invalid_time"));
    }
    Ok(())
}

fn editable(info: &AssessmentInfo) -> Res<()> {
    if info.teacher_id.is_some() {
        return Err(err(StatusCode::FORBIDDEN, "forbidden")); // moderation only for teachers' exams
    }
    if info.status == "archived" {
        return Err(err(StatusCode::CONFLICT, "archived"));
    }
    Ok(())
}

pub fn update_exam(conn: &Connection, admin: &User, id: &str, r: &ExamReq, now: i64) -> Res<ExamDetail> {
    let cur = get_info(conn, &admin.id, id)?;
    editable(&cur)?;
    if r.status.is_some() {
        return Err(bad("invalid_status")); // lifecycle changes use their own actions
    }
    let frozen = cur.attempt_count > 0;
    let base = Cfg::of(&cur);
    let cfg = merge(&base, r)?;

    if frozen {
        // PRD E6: after the first attempt only these may change: title/description, a later (or no) closing
        // time, show-answers, release timing, pass mark, and more attempts.
        let moved_start = cfg.opens_at != base.opens_at;
        let shortened = match (base.closes_at, cfg.closes_at) {
            (Some(old), Some(new)) => new < old,
            (None, Some(_)) => true,
            _ => false,
        };
        if r.questions.is_some() || r.sources.is_some() || r.subject_id.as_deref().map_or(false, |s| s != cur.subject_id)
            || cfg.duration_min != base.duration_min || moved_start || cfg.shuffle_questions != base.shuffle_questions
            || cfg.shuffle_options != base.shuffle_options
        {
            return Err(err(StatusCode::CONFLICT, "has_attempts"));
        }
        if shortened || cfg.max_attempts < base.max_attempts {
            return Err(err(StatusCode::CONFLICT, "only_extend"));
        }
    }

    let subject_id = match r.subject_id.as_deref() {
        Some(s) => {
            subject_state(conn, s)?;
            s.to_string()
        }
        None => cur.subject_id.clone(),
    };
    let title = match &r.title {
        Some(t) => text(t, 200, "invalid_title")?,
        None => cur.title.clone(),
    };
    let description = if r.description.is_some() { opt_text(&r.description, 2000, "description_too_long")? } else { cur.description.clone() };

    let (questions, sources) = match &r.questions {
        Some(qs) => {
            let cleaned = clean_questions(qs)?;
            let src = clean_sources(&r.sources, &cleaned)?;
            (cleaned, src)
        }
        None => {
            let (q, mut s) = load_snapshot(conn, id)?;
            if r.sources.is_some() {
                s = clean_sources(&r.sources, &q)?;
            }
            (q, s)
        }
    };
    if cur.status == "published" && questions.is_empty() {
        return Err(bad("invalid_question_count"));
    }
    if cur.status == "published" && cfg.closes_at.map_or(false, |c| c <= now) && !frozen {
        return Err(bad("invalid_time"));
    }
    conn.execute(
        "UPDATE assessments SET subject_id = ?2, title = ?3, description = ?4, questions = ?5, question_count = ?6, total_points = ?7, duration_min = ?8,
           opens_at = ?9, closes_at = ?10, max_attempts = ?11, show_answers = ?12, shuffle_questions = ?13, shuffle_options = ?14, pass_mark = ?15,
           release_mode = ?16, source_map = ?17, updated_at = ?18 WHERE id = ?1",
        params![
            id, subject_id, title, description, serde_json::to_string(&questions).map_err(db_err)?, questions.len() as i64, total_of(&questions),
            cfg.duration_min, cfg.opens_at, cfg.closes_at, cfg.max_attempts, cfg.show_answers, cfg.shuffle_questions, cfg.shuffle_options, cfg.pass_mark,
            cfg.release_mode, serde_json::to_string(&sources).map_err(db_err)?, now
        ],
    )
    .map_err(db_err)?;
    audit(conn, &admin.id, id, "exam_update", if frozen { "frozen" } else { "" });
    get_exam(conn, admin, id, now)
}

// ───────── lifecycle ─────────

pub fn act(conn: &Connection, admin: &User, id: &str, action: &str, now: i64) -> Res<ExamDetail> {
    let cur = get_info(conn, &admin.id, id)?;
    let phase = phase_of(&cur, now);
    let set = |status: &str, closed: Option<i64>, archived: Option<i64>| -> Res<()> {
        conn.execute("UPDATE assessments SET status = ?2, closed_at = ?3, archived_at = ?4, updated_at = ?5 WHERE id = ?1", params![id, status, closed, archived, now])
            .map_err(db_err)?;
        Ok(())
    };
    let conflict = |code: &str| Err(err(StatusCode::CONFLICT, code));
    match action {
        "publish" => {
            editable(&cur)?;
            if cur.status != "draft" {
                return conflict("invalid_transition");
            }
            let (questions, _) = load_snapshot(conn, id)?;
            check_publishable(&questions, subject_state(conn, &cur.subject_id)?, &Cfg::of(&cur), now)?;
            set("published", None, None)?;
            announce(conn, admin, &cur.subject_id, id, &cur.title);
        }
        "unpublish" => {
            if cur.status != "published" || cur.teacher_id.is_some() && false {
                return conflict("invalid_transition");
            }
            if cur.attempt_count > 0 {
                return conflict("has_attempts"); // close it instead
            }
            set("draft", None, None)?;
        }
        "close" => {
            if cur.status != "published" {
                return conflict("invalid_transition");
            }
            set("closed", Some(now), None)?;
        }
        "reopen" => {
            if cur.status != "closed" {
                return conflict("invalid_transition");
            }
            editable(&cur)?;
            if cur.closes_at.map_or(false, |c| c <= now) {
                return Err(bad("invalid_time")); // extend the closing time first
            }
            set("published", None, None)?;
        }
        "archive" => {
            if cur.status == "published" {
                return conflict("close_first");
            }
            if cur.status == "archived" {
                return conflict("invalid_transition");
            }
            set("archived", cur.closed_at, Some(now))?;
        }
        "restore" => {
            if cur.status != "archived" {
                return conflict("invalid_transition");
            }
            // back to where it came from: closed if it ever ran, otherwise a draft
            let back = if cur.closed_at.is_some() || cur.attempt_count > 0 { "closed" } else { "draft" };
            let closed_at = if back == "closed" { cur.closed_at.or(Some(now)) } else { None };
            set(back, closed_at, None)?;
        }
        _ => return Err(bad("invalid_action")),
    }
    let _ = phase;
    audit(conn, &admin.id, id, &format!("exam_{action}"), &cur.status);
    get_exam(conn, admin, id, now)
}

pub fn duplicate(conn: &Connection, admin: &User, id: &str, now: i64) -> Res<ExamDetail> {
    let cur = get_info(conn, &admin.id, id)?;
    let (questions, sources) = load_snapshot(conn, id)?;
    let mut title = cur.title.clone();
    if title.chars().count() + COPY_SUFFIX.chars().count() > 200 {
        title = title.chars().take(200 - COPY_SUFFIX.chars().count()).collect();
    }
    title.push_str(COPY_SUFFIX);
    let new = new_id();
    // A copy is a fresh draft: same snapshot and settings, but no stale opening/closing times and owned by the admin.
    conn.execute(
        "INSERT INTO assessments(id, teacher_id, created_by, subject_id, title, description, questions, question_count, total_points, duration_min,
           opens_at, closes_at, max_attempts, show_answers, shuffle_questions, shuffle_options, pass_mark, release_mode, status, source_map, created_at, updated_at)
         SELECT ?2, NULL, ?3, subject_id, ?4, description, questions, question_count, total_points, duration_min,
           NULL, NULL, max_attempts, show_answers, shuffle_questions, shuffle_options, pass_mark,
           CASE WHEN release_mode = 'after_close' THEN 'immediate' ELSE release_mode END, 'draft', ?5, ?6, ?6 FROM assessments WHERE id = ?1",
        params![id, new, admin.id, title, serde_json::to_string(&sources).map_err(db_err)?, now],
    )
    .map_err(db_err)?;
    let _ = questions;
    audit(conn, &admin.id, &new, "exam_duplicate", id);
    get_exam(conn, admin, &new, now)
}

/// Deleting an exam that has attempts needs the exam's exact title as confirmation (and is audited).
pub fn delete_exam(conn: &Connection, admin: &User, id: &str, confirm_title: Option<&str>) -> Res<()> {
    let cur = get_info(conn, &admin.id, id)?;
    if cur.attempt_count > 0 && confirm_title.map(str::trim) != Some(cur.title.trim()) {
        return Err(err(StatusCode::CONFLICT, "confirm_required"));
    }
    conn.execute("DELETE FROM assessments WHERE id = ?1", params![id]).map_err(db_err)?;
    audit(conn, &admin.id, id, "exam_delete", &format!("attempts={}", cur.attempt_count));
    Ok(())
}

// ───────── list ─────────

#[derive(Deserialize, Default)]
pub struct ListQuery {
    subject_id: Option<String>,
    /// `draft`, `published`, `closed`, `archived` (effective phase) or `all`. Default: everything except archived.
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct ExamPage {
    items: Vec<ExamRow>,
    total: i64,
}

const PHASE_SQL: &str = "CASE WHEN a.status = 'published' AND a.closes_at IS NOT NULL AND a.closes_at <= ?2 THEN 'closed' ELSE a.status END";

pub fn list_exams(conn: &Connection, admin: &User, f: &ListQuery, now: i64) -> Res<ExamPage> {
    // ?1 = viewer (for attempts_used in INFO_SELECT), ?2 = now; filters follow.
    let mut args: Vec<Value> = vec![Value::Text(admin.id.clone()), Value::Integer(now)];
    let mut conds: Vec<String> = vec![];
    if let Some(s) = f.subject_id.as_deref().filter(|s| !s.is_empty()) {
        args.push(Value::Text(s.to_string()));
        conds.push(format!("a.subject_id = ?{}", args.len()));
    }
    match f.status.as_deref().unwrap_or("") {
        "" => conds.push("a.status <> 'archived'".into()),
        "all" => {}
        st @ ("draft" | "published" | "closed" | "archived") => {
            args.push(Value::Text(st.to_string()));
            conds.push(format!("{PHASE_SQL} = ?{}", args.len()));
        }
        _ => return Err(bad("invalid_filter")),
    }
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        args.push(Value::Text(format!("%{}%", q.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))));
        conds.push(format!("lower(a.title) LIKE ?{} ESCAPE '\\'", args.len()));
    }
    // ?2 (now) is always referenced so the parameter count matches even when no filter uses it.
    let wh = format!("WHERE (?2 IS NOT NULL){}", conds.iter().map(|c| format!(" AND {c}")).collect::<String>());
    let limit = f.limit.unwrap_or(25).clamp(1, 100);
    let offset = f.offset.unwrap_or(0).clamp(0, 10_000_000);
    // The count query reuses the same numbering, so it also receives ?1/?2 (only a.* columns are referenced).
    let total: i64 = conn
        .query_row(&format!("SELECT count(*) FROM assessments a {wh} AND (?1 IS NOT NULL)"), params_from_iter(args.iter()), |r| r.get(0))
        .map_err(db_err)?;
    let items = conn
        .prepare(&format!("{INFO_SELECT} {wh} ORDER BY a.updated_at DESC, a.id LIMIT {limit} OFFSET {offset}"))
        .map_err(db_err)?
        .query_map(params_from_iter(args.iter()), map_info)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?
        .into_iter()
        .map(|i| row_of(i, now))
        .collect();
    Ok(ExamPage { items, total })
}

// ───────── handlers ─────────

pub async fn list_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(f): Query<ListQuery>) -> Res<Json<ExamPage>> {
    let a = require_admin(&s, &h)?;
    list_exams(&*lock(&s)?, &a, &f, now_ms()).map(Json)
}

pub async fn create_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<ExamReq>) -> Res<(StatusCode, Json<ExamDetail>)> {
    let a = require_admin(&s, &h)?;
    create_exam(&*lock(&s)?, &a, &r, now_ms()).map(|d| (StatusCode::CREATED, Json(d)))
}

pub async fn get_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>) -> Res<Json<ExamDetail>> {
    let a = require_admin(&s, &h)?;
    get_exam(&*lock(&s)?, &a, &id, now_ms()).map(Json)
}

pub async fn update_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<ExamReq>) -> Res<Json<ExamDetail>> {
    let a = require_admin(&s, &h)?;
    update_exam(&*lock(&s)?, &a, &id, &r, now_ms()).map(Json)
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    confirm_title: Option<String>,
}

pub async fn delete_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Query(q): Query<DeleteQuery>) -> Res<StatusCode> {
    let a = require_admin(&s, &h)?;
    delete_exam(&*lock(&s)?, &a, &id, q.confirm_title.as_deref())?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn action_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path((id, action)): Path<(String, String)>) -> Res<(StatusCode, Json<ExamDetail>)> {
    let a = require_admin(&s, &h)?;
    let conn = lock(&s)?;
    if action == "duplicate" {
        return duplicate(&conn, &a, &id, now_ms()).map(|d| (StatusCode::CREATED, Json(d)));
    }
    act(&conn, &a, &id, &action, now_ms()).map(|d| (StatusCode::OK, Json(d)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};
    use serde_json::json;

    struct W {
        conn: Connection,
        admin: User,
    }
    fn world() -> W {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        conn.execute("INSERT INTO institutions(id, type, name_ar, created_at) VALUES ('i1','university','جامعة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, created_at) VALUES ('s1','i1','برمجة',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id, institution_id, name_ar, is_active, created_at) VALUES ('s2','i1','مجمدة',0,0)", []).unwrap();
        W { conn, admin }
    }
    fn qs(n: usize) -> Vec<serde_json::Value> {
        (1..=n).map(|i| json!({"id": format!("q{i}"), "type": "single_choice", "stem": format!("سؤال {i}"), "options": ["أ", "ب", "ج"], "answer": "b", "analysis": "", "score": 2})).collect()
    }
    fn req(v: serde_json::Value) -> ExamReq {
        serde_json::from_value(v).unwrap()
    }
    fn basic(extra: serde_json::Value) -> ExamReq {
        let mut v = json!({"subject_id": "s1", "title": "امتحان", "questions": qs(3)});
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        req(v)
    }
    const NOW: i64 = 1_000_000_000_000;

    fn add_attempt(w: &W, exam: &str) -> String {
        let student = insert_test_user(&w.conn, &format!("{}@x.com", new_id()), "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, 's1', 0)", params![student.id]).ok();
        let id = new_id();
        w.conn.execute("INSERT INTO attempts(id, assessment_id, student_id, started_at, status) VALUES (?1, ?2, ?3, ?4, 'submitted')", params![id, exam, student.id, NOW]).unwrap();
        student.id
    }

    #[test]
    fn create_snapshots_cleaned_questions_and_applies_defaults() {
        let w = world();
        let d = create_exam(&w.conn, &w.admin, &basic(json!({"sources": {"q1": "bank-1", "ghost": "x"}})), NOW).unwrap();
        let i = &d.row.info;
        assert_eq!((i.teacher_id.clone(), i.created_by.clone(), i.status.as_str(), i.question_count, i.total_points), (None, Some(w.admin.id.clone()), "draft", 3, 6.0));
        assert_eq!((i.max_attempts, i.show_answers, i.shuffle_questions, i.shuffle_options, i.pass_mark, i.release_mode.as_str()), (1, true, false, false, None, "immediate"));
        assert_eq!(d.questions[0].answer, "B", "answers are normalised like the bank's");
        assert_eq!(d.sources.len(), 1, "sources for unknown question ids are dropped");
        assert!(d.row.can_edit && d.row.phase == "draft");
        assert_eq!(i.teacher_name, "T", "the creating admin is shown where a teacher would be");
    }

    #[test]
    fn validation_rejects_bad_input_and_reports_the_offending_question() {
        let w = world();
        let bad_req = |extra: serde_json::Value| create_exam(&w.conn, &w.admin, &basic(extra), NOW).unwrap_err();
        for (label, extra) in [
            ("duration 0", json!({"duration_min": 0})), ("duration 481", json!({"duration_min": 481})), ("attempts 0", json!({"max_attempts": 0})), ("attempts 11", json!({"max_attempts": 11})),
            ("pass 0", json!({"pass_mark": 0})), ("pass 101", json!({"pass_mark": 101})), ("release", json!({"release_mode": "never"})),
            ("after_close without close", json!({"release_mode": "after_close"})), ("window reversed", json!({"opens_at": NOW + 10, "closes_at": NOW})),
            ("status", json!({"status": "closed"})), ("empty title", json!({"title": "  "})),
        ] {
            assert_eq!(bad_req(extra).0, StatusCode::BAD_REQUEST, "{label}");
        }
        assert_eq!(create_exam(&w.conn, &w.admin, &basic(json!({"subject_id": "nope"})), NOW).unwrap_err().0, StatusCode::NOT_FOUND);
        let mut qs_bad = qs(3);
        qs_bad[1]["answer"] = json!("Z");
        let e = bad_req(json!({"questions": qs_bad}));
        let v: serde_json::Value = serde_json::from_str(&e.1).unwrap();
        assert_eq!((e.0, v["error"].as_str(), v["question_id"].as_str(), v["index"].as_i64()), (StatusCode::BAD_REQUEST, Some("invalid_answer"), Some("q2"), Some(1)));
        let mut dup = qs(2);
        dup[1]["id"] = json!("q1");
        assert_eq!(bad_req(json!({"questions": dup})).0, StatusCode::BAD_REQUEST, "duplicate ids");
        assert_eq!(bad_req(json!({"questions": qs(201)})).0, StatusCode::BAD_REQUEST, "more than 200 questions");
        assert_eq!(bad_req(json!({"status": "published", "questions": []})).0, StatusCode::BAD_REQUEST, "cannot publish an empty exam");
        assert_eq!(bad_req(json!({"status": "published", "subject_id": "s2"})).0, StatusCode::BAD_REQUEST, "cannot publish into an inactive subject");
        assert_eq!(bad_req(json!({"status": "published", "closes_at": NOW - 1})).0, StatusCode::BAD_REQUEST, "cannot publish with a closing time in the past");
        assert!(create_exam(&w.conn, &w.admin, &basic(json!({"questions": []})), NOW).is_ok(), "an empty draft is fine");
    }

    #[test]
    fn everything_is_editable_until_the_first_attempt_then_frozen_as_prd_e6() {
        let w = world();
        let d = create_exam(&w.conn, &w.admin, &basic(json!({"closes_at": NOW + 1000, "max_attempts": 2, "status": "published"})), NOW).unwrap();
        let id = d.row.info.id.clone();
        let up = |v: serde_json::Value| update_exam(&w.conn, &w.admin, &id, &req(v), NOW + 1);
        let u = up(json!({"title": "جديد", "questions": qs(5), "duration_min": 30, "shuffle_questions": true, "shuffle_options": true, "pass_mark": 60, "subject_id": "s1"})).unwrap();
        assert_eq!((u.row.info.question_count, u.row.info.total_points, u.row.info.duration_min, u.row.info.pass_mark), (5, 10.0, Some(30), Some(60.0)));
        assert!(up(json!({"clear_duration": true, "clear_pass_mark": true})).unwrap().row.info.duration_min.is_none());
        assert_eq!(up(json!({"status": "closed"})).unwrap_err().0, StatusCode::BAD_REQUEST, "status is not changed by PATCH");
        add_attempt(&w, &id);
        for (label, v) in [("questions", json!({"questions": qs(2)})), ("duration", json!({"duration_min": 10})), ("shuffle", json!({"shuffle_options": false})), ("opens", json!({"opens_at": NOW - 5})), ("sources", json!({"sources": {}}))] {
            assert_eq!(up(v).unwrap_err().0, StatusCode::CONFLICT, "{label} frozen after an attempt");
        }
        assert_eq!(up(json!({"closes_at": NOW + 500})).unwrap_err().0, StatusCode::CONFLICT, "closing earlier is not an extension");
        assert_eq!(up(json!({"max_attempts": 1})).unwrap_err().0, StatusCode::CONFLICT, "attempts can only grow");
        let ok = up(json!({"title": "عنوان معدل", "description": "وصف", "closes_at": NOW + 5000, "max_attempts": 3, "show_answers": false, "pass_mark": 50, "release_mode": "after_close"})).unwrap();
        assert_eq!((ok.row.info.title.as_str(), ok.row.info.closes_at, ok.row.info.max_attempts, ok.row.info.show_answers, ok.row.info.release_mode.as_str()), ("عنوان معدل", Some(NOW + 5000), 3, false, "after_close"));
        assert_eq!(ok.questions.len(), 5, "questions untouched");
        assert!(up(json!({"clear_closes": true, "release_mode": "immediate"})).is_ok(), "removing the closing time is an extension");
    }

    #[test]
    fn lifecycle_transitions_and_their_guards() {
        let w = world();
        let id = create_exam(&w.conn, &w.admin, &basic(json!({"closes_at": NOW + 10_000})), NOW).unwrap().row.info.id;
        let act = |a: &str, now: i64| act(&w.conn, &w.admin, &id, a, now);
        assert_eq!(act("close", NOW).unwrap_err().0, StatusCode::CONFLICT, "a draft cannot be closed");
        assert_eq!(act("explode", NOW).unwrap_err().0, StatusCode::BAD_REQUEST);
        let p = act("publish", NOW).unwrap();
        assert_eq!(p.row.info.status, "published");
        assert_eq!(act("publish", NOW).unwrap_err().0, StatusCode::CONFLICT);
        let notified: i64 = w.conn.query_row("SELECT count(*) FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(notified, 0, "nobody enrolled yet, nobody notified");
        assert_eq!(act("archive", NOW).unwrap_err().0, StatusCode::CONFLICT, "close before archiving a live exam");
        assert_eq!(act("unpublish", NOW).unwrap().row.info.status, "draft");
        act("publish", NOW).unwrap();
        // a published exam past its closing time reads as closed without a status write
        let late = get_exam(&w.conn, &w.admin, &id, NOW + 20_000).unwrap();
        assert_eq!((late.row.info.status.as_str(), late.row.phase.as_str()), ("published", "closed"));
        let c = act("close", NOW + 100).unwrap();
        assert_eq!((c.row.info.status.as_str(), c.row.info.closed_at), ("closed", Some(NOW + 100)));
        assert_eq!(act("reopen", NOW + 20_000).unwrap_err().0, StatusCode::BAD_REQUEST, "closing time already passed: extend it first");
        update_exam(&w.conn, &w.admin, &id, &req(json!({"closes_at": NOW + 99_000})), NOW + 20_000).ok();
        assert_eq!(act("reopen", NOW + 20_000).unwrap().row.info.status, "published");
        act("close", NOW + 21_000).unwrap();
        add_attempt(&w, &id);
        let a = act("archive", NOW + 22_000).unwrap();
        assert!(a.row.info.archived_at.is_some() && a.row.phase == "archived");
        assert_eq!(update_exam(&w.conn, &w.admin, &id, &req(json!({"title": "x"})), NOW).unwrap_err().0, StatusCode::CONFLICT, "archived exams are read-only");
        assert_eq!(act("restore", NOW + 23_000).unwrap().row.info.status, "closed", "an exam that ran comes back as closed");
        act("archive", NOW + 24_000).unwrap();
        let fresh = create_exam(&w.conn, &w.admin, &basic(json!({})), NOW).unwrap().row.info.id;
        act_on(&w, &fresh, "archive");
        assert_eq!(act_on(&w, &fresh, "restore"), "draft", "a never-run exam comes back as a draft");
        let log: String = w.conn.query_row("SELECT group_concat(action, ',') FROM audit_log", [], |r| r.get(0)).unwrap();
        for a in ["exam_create", "exam_publish", "exam_unpublish", "exam_close", "exam_reopen", "exam_archive", "exam_restore"] {
            assert!(log.contains(a), "{a} is audited");
        }
    }
    fn act_on(w: &W, id: &str, a: &str) -> String {
        act(&w.conn, &w.admin, id, a, NOW + 30_000).unwrap().row.info.status
    }

    #[test]
    fn publishing_notifies_the_enrolled_students_and_admin_exams_are_startable_without_a_teacher() {
        let w = world();
        let student = insert_test_user(&w.conn, "s@x.com", "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, 's1', 0)", params![student.id]).unwrap();
        let id = create_exam(&w.conn, &w.admin, &basic(json!({"status": "published", "duration_min": 20})), NOW).unwrap().row.info.id;
        let n: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = 'new_assessment'", params![student.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let avail: Vec<String> = crate::platform_exams::list_available(&w.conn, &student.id).unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(avail, vec![id.clone()], "an admin exam shows up for enrolled students");
        let started = crate::platform_exams::start_attempt(&w.conn, &student, &id, NOW + 1).expect("no teacher needed to take an admin exam");
        assert_eq!(started.questions.len(), 3);
        // a closed exam can no longer be started by someone who has no open attempt
        act(&w.conn, &w.admin, &id, "close", NOW + 2).unwrap();
        let other = insert_test_user(&w.conn, "s2@x.com", "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, 's1', 0)", params![other.id]).unwrap();
        assert!(crate::platform_exams::start_attempt(&w.conn, &other, &id, NOW + 3).is_err(), "closed exams refuse new attempts");
        assert!(crate::platform_exams::start_attempt(&w.conn, &student, &id, NOW + 3).unwrap().resumed, "an attempt already in progress can still be resumed");
        assert!(crate::platform_exams::list_available(&w.conn, &other.id).unwrap().is_empty(), "closed exams leave the student's 'available' list");
    }

    #[test]
    fn admin_grades_admin_exams_and_teachers_exams_are_moderation_only() {
        let w = world();
        let id = create_exam(&w.conn, &w.admin, &basic(json!({"questions": [{"id":"e1","type":"short_answer","stem":"اشرح","answer":"x","score":4}], "status": "published"})), NOW).unwrap().row.info.id;
        let student = insert_test_user(&w.conn, "s@x.com", "student", "active");
        w.conn.execute("INSERT INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, 's1', 0)", params![student.id]).unwrap();
        let start = crate::platform_exams::start_attempt(&w.conn, &student, &id, NOW + 1).unwrap();
        let answers: HashMap<String, String> = [("e1".to_string(), "إجابتي".to_string())].into();
        crate::platform_exams::submit_attempt(&w.conn, &student, &start.attempt_id, &answers, NOW + 2).unwrap();
        let teacher = insert_test_user(&w.conn, "t@x.com", "teacher", "active");
        let grade = |who: &User, pts: f64| crate::platform_exams::grade_attempt(&w.conn, who, &start.attempt_id, &crate::platform_exams::GradeReq { grades: [("e1".to_string(), pts)].into() });
        assert_eq!(grade(&teacher, 3.0).unwrap_err().0, StatusCode::FORBIDDEN, "an unrelated teacher cannot grade an admin exam (yet)");
        assert_eq!(grade(&w.admin, 3.0).unwrap().score, 3.0, "the admin grades it");
        // a teacher-owned exam: the admin may close/archive/delete but not rewrite
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1,'s1','approved',0)", params![teacher.id]).unwrap();
        let treq: crate::platform_exams::AssessmentReq = serde_json::from_value(json!({"subject_id": "s1", "title": "امتحان معلم", "questions": qs(2), "status": "draft"})).unwrap();
        let tid = crate::platform_exams::create_assessment(&w.conn, &teacher, &treq).unwrap().id;
        assert_eq!(update_exam(&w.conn, &w.admin, &tid, &req(json!({"title": "x"})), NOW).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(act(&w.conn, &w.admin, &tid, "publish", NOW).unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(act(&w.conn, &w.admin, &tid, "archive", NOW).unwrap().row.info.status, "archived");
        let copy = duplicate(&w.conn, &w.admin, &tid, NOW).unwrap();
        assert!(copy.row.can_edit && copy.row.info.teacher_id.is_none(), "a copy of a teacher's exam is an admin-owned draft");
    }

    #[test]
    fn duplicate_makes_a_fresh_draft_and_delete_with_attempts_needs_the_title() {
        let w = world();
        let id = create_exam(&w.conn, &w.admin, &basic(json!({"closes_at": NOW + 9000, "opens_at": NOW, "release_mode": "after_close", "pass_mark": 50, "shuffle_options": true, "sources": {"q1": "b1"}, "status": "published"})), NOW + 1).unwrap().row.info.id;
        add_attempt(&w, &id);
        let c = duplicate(&w.conn, &w.admin, &id, NOW + 2).unwrap();
        let i = &c.row.info;
        assert_ne!(i.id, id);
        assert_eq!((i.title.as_str(), i.status.as_str(), i.opens_at, i.closes_at, i.attempt_count, i.pass_mark, i.shuffle_options), ("امتحان (نسخة)", "draft", None, None, 0, Some(50.0), true));
        assert_eq!((i.release_mode.as_str(), c.questions.len(), c.sources.len()), ("immediate", 3, 1), "after_close would be invalid without a closing time");
        // titles that would overflow are trimmed
        let long = create_exam(&w.conn, &w.admin, &basic(json!({"title": "ع".repeat(200)})), NOW).unwrap().row.info.id;
        assert_eq!(duplicate(&w.conn, &w.admin, &long, NOW).unwrap().row.info.title.chars().count(), 200);
        // delete: the attempt-less copy goes freely, the original needs the exact title
        assert!(delete_exam(&w.conn, &w.admin, &i.id, None).is_ok());
        assert_eq!(delete_exam(&w.conn, &w.admin, &id, None).unwrap_err().0, StatusCode::CONFLICT);
        assert_eq!(delete_exam(&w.conn, &w.admin, &id, Some("عنوان آخر")).unwrap_err().0, StatusCode::CONFLICT);
        assert!(delete_exam(&w.conn, &w.admin, &id, Some("  امتحان ")).is_ok());
        let left: i64 = w.conn.query_row("SELECT count(*) FROM attempts", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "the exam's attempts go with it");
        let log: String = w.conn.query_row("SELECT group_concat(detail, ',') FROM audit_log WHERE action = 'exam_delete'", [], |r| r.get(0)).unwrap();
        assert!(log.contains("attempts=1"), "the deletion records how many attempts were lost: {log}");
        assert_eq!(delete_exam(&w.conn, &w.admin, "ghost", None).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn list_filters_by_effective_phase_subject_and_title() {
        let w = world();
        let mk = |title: &str, extra: serde_json::Value| {
            let mut v = json!({"title": title});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            create_exam(&w.conn, &w.admin, &basic(v), NOW).unwrap().row.info.id
        };
        let draft = mk("مسودة", json!({}));
        let live = mk("جارٍ", json!({"status": "published"}));
        let ending = mk("ينتهي", json!({"status": "published", "closes_at": NOW + 100}));
        let archived = mk("قديم", json!({}));
        act(&w.conn, &w.admin, &archived, "archive", NOW).unwrap();
        let ids = |f: ListQuery, now: i64| list_exams(&w.conn, &w.admin, &f, now).unwrap().items.into_iter().map(|r| r.info.id).collect::<HashSet<_>>();
        let q = |status: Option<&str>| ListQuery { status: status.map(String::from), ..Default::default() };
        assert_eq!(ids(q(None), NOW), HashSet::from([draft.clone(), live.clone(), ending.clone()]), "archived hidden by default");
        assert_eq!(ids(q(Some("archived")), NOW), HashSet::from([archived.clone()]));
        assert_eq!(ids(q(Some("all")), NOW).len(), 4);
        assert_eq!(ids(q(Some("published")), NOW), HashSet::from([live.clone(), ending.clone()]));
        assert_eq!(ids(q(Some("published")), NOW + 500), HashSet::from([live.clone()]), "past its closing time it no longer counts as published");
        assert_eq!(ids(q(Some("closed")), NOW + 500), HashSet::from([ending.clone()]), "…it counts as closed");
        assert_eq!(ids(ListQuery { q: Some("مسو".into()), ..Default::default() }, NOW), HashSet::from([draft.clone()]));
        assert!(ids(ListQuery { q: Some("%".into()), ..Default::default() }, NOW).is_empty(), "wildcards are literal");
        assert!(ids(ListQuery { subject_id: Some("s2".into()), ..Default::default() }, NOW).is_empty());
        assert!(list_exams(&w.conn, &w.admin, &q(Some("weird")), NOW).is_err());
        let page = list_exams(&w.conn, &w.admin, &ListQuery { limit: Some(2), status: Some("all".into()), ..Default::default() }, NOW).unwrap();
        assert_eq!((page.items.len(), page.total), (2, 4));
        let page2 = list_exams(&w.conn, &w.admin, &ListQuery { limit: Some(2), offset: Some(2), status: Some("all".into()), ..Default::default() }, NOW).unwrap();
        assert!(page.items.iter().all(|a| page2.items.iter().all(|b| a.info.id != b.info.id)));
    }
}
