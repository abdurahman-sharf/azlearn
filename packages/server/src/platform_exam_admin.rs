//! The exam builder engine: create/edit exams with the full 1-5 settings, the lifecycle
//! (draft → published → closed → archived), duplication and guarded deletion — for **both** actors.
//!
//! One engine, two doors: the admin routes (`/admin/exams*`, this module's handlers) and the teacher routes
//! (`/teacher/exams*`, `platform_exam_teacher`) call the same functions with the signed-in `User`, and the rules below
//! branch on who is acting:
//!
//! * **Ownership** ([`AssessmentInfo::owned_by`]): a teacher owns the exams with their id; any admin owns the exams
//!   created by an admin (`teacher_id` NULL). Content and settings are only ever changed by the owner — an admin
//!   *moderates* a teacher's exam (unpublish, close, reopen, archive, restore, delete, duplicate) but never rewrites it.
//! * **Locked** ([`AssessmentInfo::is_locked`]): a teacher's exam that somebody else closed or archived stays closed for
//!   its owner (they may duplicate or, when nobody sat it, delete it); only an admin lifts it — by reopening it, or by
//!   `unlock`, which hands it back without touching its state (and so works after the closing time has passed, and for
//!   rows from before the actor was recorded). An admin's change to a teacher's exam notifies the owner.
//! * An exam is a fixed *snapshot* of its questions: nothing here references the question bank, and a bank item's id
//!   is only remembered in `source_map` for statistics. Once the first attempt exists the questions, scores and fairness
//!   settings are frozen (PRD E6, [`check_freeze`]); the answer key is then corrected through `platform_exam_key`.
//! * Anything that turns an exam on needs its owner's live assignment ([`require_assignment`]); whether teachers may
//!   create new exams at all (`teachers.can_create_exams`) is decided by the teacher HTTP handlers, not here.

use crate::platform::{audit, bad, db_err, lock, new_id, opt_text, require_admin, text, Res, User};
use crate::platform_content::{exam_reason_sql, presence, require_assignment, HiddenReason};
use crate::platform_exams::{
    announce, check_closing_ahead, check_duration, check_freeze, check_window, clean_questions, get_info, points_of, round2, shown_state, AssessmentInfo, Cfg, INFO_COLS, INFO_FROM,
};
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

#[derive(Deserialize, Default, Clone)]
pub struct ExamReq {
    pub(crate) subject_id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) questions: Option<Vec<Question>>,
    /// Question id → bank item id it was copied from (statistics only).
    pub(crate) sources: Option<HashMap<String, String>>,
    pub(crate) duration_min: Option<i64>,
    pub(crate) clear_duration: Option<bool>,
    pub(crate) opens_at: Option<i64>,
    pub(crate) clear_opens: Option<bool>,
    pub(crate) closes_at: Option<i64>,
    pub(crate) clear_closes: Option<bool>,
    pub(crate) max_attempts: Option<i64>,
    pub(crate) show_answers: Option<bool>,
    pub(crate) shuffle_questions: Option<bool>,
    pub(crate) shuffle_options: Option<bool>,
    /// Percentage of the total points (1–100).
    pub(crate) pass_mark: Option<f64>,
    pub(crate) clear_pass_mark: Option<bool>,
    /// `immediate` or `after_close`.
    pub(crate) release_mode: Option<String>,
    /// Create only: `draft` (default) or `published`. Later changes go through the lifecycle actions.
    pub(crate) status: Option<String>,
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

/// Whether students can reach the subject: it AND its institution are switched on (404 when it does not exist).
fn subject_state(conn: &Connection, id: &str) -> Res<bool> {
    conn.query_row(
        "SELECT s.is_active AND i.is_active FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE s.id = ?1",
        params![id],
        |r| r.get(0),
    )
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
    pub(crate) info: AssessmentInfo,
    /// Effective lifecycle phase: a published exam past its closing time reads as `closed`.
    pub(crate) phase: String,
    /// The viewer may change this exam's content and settings right now: they own it, it is not archived and not locked.
    pub(crate) can_edit: bool,
    /// The viewer owns it (see [`AssessmentInfo::owned_by`]).
    pub(crate) owned: bool,
    /// A teacher's exam whose closed / archived state was set by someone other than its owner (see
    /// [`AssessmentInfo::is_locked`]); only meaningful to the owner, but the same for every viewer.
    pub(crate) locked: bool,
    /// Submitted attempts.
    pub(crate) submitted: i64,
    /// Written answers of submitted attempts still waiting for a grade.
    pub(crate) pending_answers: i64,
    /// Whether students can see the exam right now, and when it is published / closed but not shown, why — the same
    /// answer as `GET /assessments/mine` (one rule: `platform_content::standing`).
    pub(crate) visible: bool,
    pub(crate) hidden_reason: Option<HiddenReason>,
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
    pub(crate) row: ExamRow,
    pub(crate) questions: Vec<Question>,
    pub(crate) sources: HashMap<String, String>,
}

fn phase_of(i: &AssessmentInfo, now: i64) -> String {
    if i.status == "published" && i.closes_at.map_or(false, |c| c <= now) {
        "closed".into()
    } else {
        i.status.clone()
    }
}

/// What one query returns per exam: the info, its submitted / pending counts and the hidden reason.
type RowData = (AssessmentInfo, i64, i64, Option<String>);

/// `SELECT` of everything an [`ExamRow`] needs in ONE statement (counts and the reason are correlated subqueries, so a
/// page of exams costs one query, not one per row). `?1` = the viewer (for `attempts_used`). Columns: the 26 of
/// `map_info`, then `submitted` (26), `pending_answers` (27) and the reason (28).
fn row_select() -> String {
    format!(
        "SELECT {INFO_COLS},
           (SELECT count(*) FROM attempts x WHERE x.assessment_id = a.id AND x.status = 'submitted'),
           (SELECT COALESCE(sum(x.pending), 0) FROM attempts x WHERE x.assessment_id = a.id AND x.status = 'submitted'),
           ({}) {INFO_FROM}",
        exam_reason_sql("a")
    )
}

fn map_row(r: &rusqlite::Row) -> rusqlite::Result<RowData> {
    Ok((crate::platform_exams::map_info(r)?, r.get(26)?, r.get(27)?, r.get(28)?))
}

fn row_of(viewer: &User, (info, submitted, pending_answers, reason): RowData, now: i64) -> ExamRow {
    let (visible, hidden_reason) = presence(shown_state(&info.status), reason);
    let owned = info.owned_by(viewer);
    let locked = info.is_locked();
    ExamRow {
        phase: phase_of(&info, now),
        can_edit: owned && info.status != "archived" && !locked,
        owned,
        locked,
        submitted,
        pending_answers,
        visible,
        hidden_reason,
        info,
    }
}

fn load_row(conn: &Connection, viewer: &User, id: &str, now: i64) -> Res<ExamRow> {
    conn.query_row(&format!("{} WHERE a.id = ?2", row_select()), params![viewer.id, id], map_row)
        .optional()
        .map_err(db_err)?
        .map(|d| row_of(viewer, d, now))
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn load_snapshot(conn: &Connection, id: &str) -> Res<(Vec<Question>, HashMap<String, String>)> {
    let (q, s): (String, Option<String>) = conn
        .query_row("SELECT questions, source_map FROM assessments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(db_err)?;
    Ok((serde_json::from_str(&q).map_err(db_err)?, s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()))
}

/// An admin reads any exam; a teacher only their own (the questions come with their answers) — 403 otherwise.
pub fn get_exam(conn: &Connection, user: &User, id: &str, now: i64) -> Res<ExamDetail> {
    let row = load_row(conn, user, id, now)?;
    if user.role == "teacher" && !row.owned {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let (questions, sources) = load_snapshot(conn, id)?;
    Ok(ExamDetail { row, questions, sources })
}

/// The `teacher_id` a new exam gets: the teacher themself, nobody for an admin (an admin exam). Anyone else: 403.
fn owner_for(user: &User) -> Res<Option<&str>> {
    match user.role.as_str() {
        "teacher" => Ok(Some(user.id.as_str())),
        "admin" => Ok(None),
        _ => Err(err(StatusCode::FORBIDDEN, "forbidden")),
    }
}

// ───────── create / update ─────────

pub fn create_exam(conn: &Connection, user: &User, r: &ExamReq, now: i64) -> Res<ExamDetail> {
    let owner = owner_for(user)?;
    let subject_id = r.subject_id.as_deref().unwrap_or("");
    // A teacher creates only in a subject they are approved for (active subject, active institution): 403
    // `not_assigned` before anything about the content is looked at.
    if let Some(teacher) = owner {
        require_assignment(conn, teacher, subject_id)?;
    }
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
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?21)",
        params![
            id, owner, user.id, subject_id, title, description, serde_json::to_string(&questions).map_err(db_err)?, questions.len() as i64, total_of(&questions),
            cfg.duration_min, cfg.opens_at, cfg.closes_at, cfg.max_attempts, cfg.show_answers, cfg.shuffle_questions, cfg.shuffle_options, cfg.pass_mark,
            cfg.release_mode, status, serde_json::to_string(&sources).map_err(db_err)?, now
        ],
    )
    .map_err(db_err)?;
    audit(conn, &user.id, &id, "exam_create", status);
    if status == "published" {
        audit(conn, &user.id, &id, "exam_publish", "on create");
        announce(conn, user, subject_id, &id, &title, now);
    }
    get_exam(conn, user, &id, now)
}

/// What publishing requires: at least one question, an active subject and a closing time still ahead.
fn check_publishable(questions: &[Question], subject_active: bool, cfg: &Cfg, now: i64) -> Res<()> {
    if questions.is_empty() {
        return Err(bad("invalid_question_count"));
    }
    if !subject_active {
        return Err(bad("subject_inactive"));
    }
    check_closing_ahead(cfg.closes_at, now)
}

/// Content and settings are the owner's alone: 403 for anyone else (an admin moderates a teacher's exam, a teacher
/// never touches another's), 409 `archived` for an archived exam and 409 `locked` for one somebody else closed.
fn editable(user: &User, info: &AssessmentInfo) -> Res<()> {
    if !info.owned_by(user) {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    if info.status == "archived" {
        return Err(err(StatusCode::CONFLICT, "archived"));
    }
    if info.is_locked() {
        return Err(err(StatusCode::CONFLICT, "locked"));
    }
    Ok(())
}

pub fn update_exam(conn: &Connection, user: &User, id: &str, r: &ExamReq, now: i64) -> Res<ExamDetail> {
    let cur = get_info(conn, &user.id, id)?;
    editable(user, &cur)?;
    if r.status.is_some() {
        return Err(bad("invalid_status")); // lifecycle changes use their own actions
    }
    let frozen = cur.attempt_count > 0;
    let base = Cfg::of(&cur);
    let cfg = merge(&base, r)?;

    let new_subject = r.subject_id.as_deref().filter(|s| *s != cur.subject_id);
    if frozen {
        check_freeze(&base, &cfg, r.questions.is_some() || r.sources.is_some() || new_subject.is_some())?;
    }

    let subject_id = match r.subject_id.as_deref() {
        Some(s) => {
            // moving a teacher's exam into another subject needs an approved assignment there, like creating one
            if new_subject.is_some() && user.role == "teacher" {
                require_assignment(conn, &user.id, s)?;
            }
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
    // extending a closing time that had already passed reopens the exam (see `reopen`)
    if base.closes_at.map_or(false, |c| c <= now) && cfg.closes_at.map_or(true, |c| c > now) {
        crate::platform_reminders::reset_results(conn, id);
    }
    audit(conn, &user.id, id, "exam_update", if frozen { "frozen" } else { "" });
    get_exam(conn, user, id, now)
}

// ───────── lifecycle ─────────

/// A new lifecycle state together with who put the exam there (`closed_by` / `archived_by`: what `is_locked` reads).
struct Move<'a> {
    status: &'a str,
    closed_at: Option<i64>,
    closed_by: Option<&'a str>,
    archived_at: Option<i64>,
    archived_by: Option<&'a str>,
}

impl<'a> Move<'a> {
    /// A state with no closing or archiving record (draft / published).
    fn live(status: &'a str) -> Self {
        Move { status, closed_at: None, closed_by: None, archived_at: None, archived_by: None }
    }
}

/// The state-changing actions of [`act`] (`duplicate` creates a new exam and is [`duplicate`]). `unlock` is the admin's
/// way to hand a locked exam back to its owner without changing its state (reopening needs a closing time still ahead
/// and the owner's live assignment, so on its own it cannot always lift a lock).
const ACTIONS: [&str; 7] = ["publish", "unpublish", "close", "reopen", "archive", "restore", "unlock"];

/// Who may run a lifecycle action on `cur` at all: a teacher only on their own exam and not while it is locked; an
/// admin on any exam (each action below then decides what an admin may do to a teacher's exam).
fn may_act(user: &User, cur: &AssessmentInfo) -> Res<()> {
    match user.role.as_str() {
        "admin" => Ok(()),
        "teacher" if !cur.owned_by(user) => Err(err(StatusCode::FORBIDDEN, "forbidden")),
        "teacher" if cur.is_locked() => Err(err(StatusCode::CONFLICT, "locked")),
        "teacher" => Ok(()),
        _ => Err(err(StatusCode::FORBIDDEN, "forbidden")),
    }
}

pub fn act(conn: &Connection, user: &User, id: &str, action: &str, now: i64) -> Res<ExamDetail> {
    let cur = get_info(conn, &user.id, id)?;
    if !ACTIONS.contains(&action) {
        return Err(bad("invalid_action"));
    }
    may_act(user, &cur)?;
    let set = |s: Move| -> Res<()> {
        conn.execute(
            "UPDATE assessments SET status = ?2, closed_at = ?3, closed_by = ?4, archived_at = ?5, archived_by = ?6, updated_at = ?7 WHERE id = ?1",
            params![id, s.status, s.closed_at, s.closed_by, s.archived_at, s.archived_by, now],
        )
        .map_err(db_err)?;
        Ok(())
    };
    let conflict = |code: &str| Err(err(StatusCode::CONFLICT, code));
    let actor = user.id.as_str();
    match action {
        "publish" => {
            editable(user, &cur)?;
            if cur.status != "draft" {
                return conflict("invalid_transition");
            }
            // turning an exam on needs its OWNER's live assignment (an admin exam has no owner to check)
            if let Some(owner) = cur.teacher_id.as_deref() {
                require_assignment(conn, owner, &cur.subject_id)?;
            }
            let (questions, _) = load_snapshot(conn, id)?;
            check_publishable(&questions, subject_state(conn, &cur.subject_id)?, &Cfg::of(&cur), now)?;
            set(Move::live("published"))?;
            announce(conn, user, &cur.subject_id, id, &cur.title, now);
        }
        "unpublish" => {
            if cur.status != "published" {
                return conflict("invalid_transition");
            }
            if cur.attempt_count > 0 {
                return conflict("has_attempts"); // close it instead
            }
            set(Move::live("draft"))?;
        }
        "close" => {
            if cur.status != "published" {
                return conflict("invalid_transition");
            }
            set(Move { status: "closed", closed_at: Some(now), closed_by: Some(actor), archived_at: None, archived_by: None })?;
        }
        "reopen" => {
            if cur.status != "closed" {
                return conflict("invalid_transition");
            }
            if cur.closes_at.map_or(false, |c| c <= now) {
                return Err(bad("invalid_time")); // extend the closing time first
            }
            // it goes live again, so (as for publishing) its owner must still hold the assignment; an admin reopening a
            // teacher's exam is subject to the same rule. Reopening does not need the "teachers may create exams" switch.
            if let Some(owner) = cur.teacher_id.as_deref() {
                require_assignment(conn, owner, &cur.subject_id)?;
            }
            set(Move::live("published"))?;
            // results are withheld again until the next close, so "results available" will be due again then
            crate::platform_reminders::reset_results(conn, id);
        }
        "archive" => {
            if cur.status == "published" {
                return conflict("close_first");
            }
            if cur.status == "archived" {
                return conflict("invalid_transition");
            }
            set(Move { status: "archived", closed_at: cur.closed_at, closed_by: cur.closed_by.as_deref(), archived_at: Some(now), archived_by: Some(actor) })?;
        }
        "restore" => {
            if cur.status != "archived" {
                return conflict("invalid_transition");
            }
            // back to where it came from: closed if it ever ran, otherwise a draft. A restored exam that had been closed
            // keeps whoever closed it; one that merely has attempts counts as closed by whoever restores it.
            if cur.closed_at.is_some() || cur.attempt_count > 0 {
                let by = if cur.closed_at.is_some() { cur.closed_by.as_deref() } else { Some(actor) };
                set(Move { status: "closed", closed_at: cur.closed_at.or(Some(now)), closed_by: by, archived_at: None, archived_by: None })?;
            } else {
                set(Move::live("draft"))?;
            }
        }
        "unlock" => {
            // Admins only (a teacher's locked exam never gets this far: `may_act` answered 409 `locked`). It changes no
            // state: the actor columns that made the exam locked simply name its owner from now on.
            if user.role != "admin" {
                return Err(err(StatusCode::FORBIDDEN, "forbidden"));
            }
            let owner = match cur.teacher_id.as_deref() {
                Some(owner) if cur.is_locked() => owner,
                _ => return conflict("invalid_transition"), // an admin exam, or one that is not locked
            };
            let closed_by = if cur.closed_at.is_some() { Some(owner) } else { cur.closed_by.as_deref() };
            if cur.status == "archived" {
                set(Move { status: "archived", closed_at: cur.closed_at, closed_by, archived_at: cur.archived_at, archived_by: Some(owner) })?;
            } else {
                set(Move { status: "closed", closed_at: cur.closed_at, closed_by: Some(owner), archived_at: None, archived_by: None })?;
            }
        }
        _ => unreachable!("validated against ACTIONS"),
    }
    audit(conn, &user.id, id, &format!("exam_{action}"), &cur.status);
    tell_owner(conn, user, &cur, id, action);
    get_exam(conn, user, id, now)
}

/// An admin changed the state of a teacher's exam: the owner is told (they cannot tell otherwise why it closed, or that
/// they may manage it again). Teachers acting on their own exams, and admins on admin exams, notify nobody.
fn tell_owner(conn: &Connection, actor: &User, cur: &AssessmentInfo, id: &str, action: &str) {
    let Some(owner) = cur.teacher_id.as_deref().filter(|o| *o != actor.id.as_str()) else { return };
    let kind = match action {
        "unpublish" => "content_unpublished",
        "close" => "exam_admin_closed",
        "archive" => "exam_admin_archived",
        "reopen" | "restore" | "unlock" => "exam_admin_released",
        _ => return,
    };
    crate::platform_engage::notify(conn, owner, kind, serde_json::json!({ "title": cur.title }), &format!("/platform/assessments/{id}"));
}

/// A fresh draft copy for `user`: same snapshot and settings, no stale opening/closing times, owned by whoever copies
/// it. A teacher copies only their own exam (even a locked one) and needs the assignment to publish the copy later.
pub fn duplicate(conn: &Connection, user: &User, id: &str, now: i64) -> Res<ExamDetail> {
    let cur = get_info(conn, &user.id, id)?;
    let owner = owner_for(user)?;
    if let Some(teacher) = owner {
        if !cur.owned_by(user) {
            return Err(err(StatusCode::FORBIDDEN, "forbidden"));
        }
        require_assignment(conn, teacher, &cur.subject_id)?;
    }
    let (_, sources) = load_snapshot(conn, id)?;
    let mut title = cur.title.clone();
    if title.chars().count() + COPY_SUFFIX.chars().count() > 200 {
        title = title.chars().take(200 - COPY_SUFFIX.chars().count()).collect();
    }
    title.push_str(COPY_SUFFIX);
    let new = new_id();
    conn.execute(
        "INSERT INTO assessments(id, teacher_id, created_by, subject_id, title, description, questions, question_count, total_points, duration_min,
           opens_at, closes_at, max_attempts, show_answers, shuffle_questions, shuffle_options, pass_mark, release_mode, status, source_map, created_at, updated_at)
         SELECT ?2, ?3, ?4, subject_id, ?5, description, questions, question_count, total_points, duration_min,
           NULL, NULL, max_attempts, show_answers, shuffle_questions, shuffle_options, pass_mark,
           CASE WHEN release_mode = 'after_close' THEN 'immediate' ELSE release_mode END, 'draft', ?6, ?7, ?7 FROM assessments WHERE id = ?1",
        params![id, new, owner, user.id, title, serde_json::to_string(&sources).map_err(db_err)?, now],
    )
    .map_err(db_err)?;
    audit(conn, &user.id, &new, "exam_duplicate", id);
    get_exam(conn, user, &new, now)
}

/// Runs a lifecycle action — including `duplicate`, which answers 201 with the new draft — for either actor.
pub fn lifecycle(conn: &Connection, user: &User, id: &str, action: &str, now: i64) -> Res<(StatusCode, ExamDetail)> {
    if action == "duplicate" {
        duplicate(conn, user, id, now).map(|d| (StatusCode::CREATED, d))
    } else {
        act(conn, user, id, action, now).map(|d| (StatusCode::OK, d))
    }
}

/// Deleting an exam that has attempts takes them and every grade with it. An **admin** may do it after typing the
/// exam's exact title (and the deletion is audited with the attempt count); a **teacher** never can — any attempt means
/// 409 `has_attempts` and the way out is closing or archiving. A teacher deletes only their own exam.
pub fn delete_exam(conn: &Connection, user: &User, id: &str, confirm_title: Option<&str>) -> Res<()> {
    let cur = get_info(conn, &user.id, id)?;
    match user.role.as_str() {
        "admin" => {
            if cur.attempt_count > 0 && confirm_title.map(str::trim) != Some(cur.title.trim()) {
                return Err(err(StatusCode::CONFLICT, "confirm_required"));
            }
        }
        "teacher" => {
            if !cur.owned_by(user) {
                return Err(err(StatusCode::FORBIDDEN, "forbidden"));
            }
            if cur.attempt_count > 0 {
                return Err(err(StatusCode::CONFLICT, "has_attempts"));
            }
        }
        _ => return Err(err(StatusCode::FORBIDDEN, "forbidden")),
    }
    conn.execute("DELETE FROM assessments WHERE id = ?1", params![id]).map_err(db_err)?;
    audit(conn, &user.id, id, "exam_delete", &format!("attempts={}", cur.attempt_count));
    Ok(())
}

// ───────── list ─────────

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub(crate) subject_id: Option<String>,
    /// `draft`, `published`, `closed`, `archived` (effective phase) or `all`. Default: everything except archived.
    pub(crate) status: Option<String>,
    pub(crate) q: Option<String>,
    pub(crate) limit: Option<i64>,
    pub(crate) offset: Option<i64>,
    /// Teachers only: `mine` (default) or `admin` (read-only list of the admin's published / closed exams in the
    /// subjects the teacher is approved for). Ignored for admins.
    pub(crate) scope: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct ExamPage {
    pub(crate) items: Vec<ExamRow>,
    pub(crate) total: i64,
}

const PHASE_SQL: &str = "CASE WHEN a.status = 'published' AND a.closes_at IS NOT NULL AND a.closes_at <= ?2 THEN 'closed' ELSE a.status END";

/// An admin lists every exam; a teacher `scope=mine` their own (default) or `scope=admin` the admin's published /
/// closed exams for their approved subjects (never drafts or archived ones, never editable).
pub fn list_exams(conn: &Connection, user: &User, f: &ListQuery, now: i64) -> Res<ExamPage> {
    // ?1 = viewer (for attempts_used in the row select), ?2 = now; filters follow.
    let mut args: Vec<Value> = vec![Value::Text(user.id.clone()), Value::Integer(now)];
    let mut conds: Vec<String> = vec![];
    match user.role.as_str() {
        "admin" => {}
        "teacher" => match f.scope.as_deref().unwrap_or("mine") {
            "mine" => conds.push("a.teacher_id = ?1".into()),
            "admin" => conds.push(
                "a.teacher_id IS NULL AND a.status IN ('published','closed') AND EXISTS(SELECT 1 FROM teacher_subjects ts
                   WHERE ts.teacher_id = ?1 AND ts.subject_id = a.subject_id AND ts.status = 'approved')"
                    .into(),
            ),
            _ => return Err(bad("invalid_filter")),
        },
        _ => return Err(err(StatusCode::FORBIDDEN, "forbidden")),
    }
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
        .prepare(&format!("{} {wh} ORDER BY a.updated_at DESC, a.id LIMIT {limit} OFFSET {offset}", row_select()))
        .map_err(db_err)?
        .query_map(params_from_iter(args.iter()), map_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?
        .into_iter()
        .map(|d| row_of(user, d, now))
        .collect();
    Ok(ExamPage { items, total })
}

// ───────── admin handlers ─────────

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
    lifecycle(&*lock(&s)?, &a, &id, &action, now_ms()).map(|(code, d)| (code, Json(d)))
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

    #[test]
    fn rows_carry_ownership_counts_and_visibility_and_the_lifecycle_records_who_acted() {
        let w = world();
        let id = create_exam(&w.conn, &w.admin, &basic(json!({"status": "published", "closes_at": NOW + 100_000})), NOW).unwrap().row.info.id;
        let r = get_exam(&w.conn, &w.admin, &id, NOW).unwrap().row;
        assert_eq!((r.owned, r.can_edit, r.locked, r.submitted, r.pending_answers, r.visible, r.hidden_reason), (true, true, false, 0, 0, true, None));
        let a1 = add_attempt(&w, &id);
        add_attempt(&w, &id);
        w.conn.execute("UPDATE attempts SET pending = 2 WHERE student_id = ?1", params![a1]).unwrap();
        w.conn.execute("INSERT INTO attempts(id, assessment_id, student_id, started_at, status, pending) VALUES ('open', ?1, ?2, 0, 'in_progress', 5)", params![id, a1]).unwrap();
        let r = get_exam(&w.conn, &w.admin, &id, NOW).unwrap().row;
        assert_eq!((r.submitted, r.pending_answers, r.info.attempt_count), (2, 2, 3), "only submitted attempts count, and an unfinished one's pending never does");
        // who acted is recorded on every transition into closed / archived, and cleared when the state is left
        let by = || -> (Option<String>, Option<String>) { w.conn.query_row("SELECT closed_by, archived_by FROM assessments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() };
        let admin = Some(w.admin.id.clone());
        assert_eq!(by(), (None, None));
        act(&w.conn, &w.admin, &id, "close", NOW + 1).unwrap();
        assert_eq!(by(), (admin.clone(), None));
        act(&w.conn, &w.admin, &id, "reopen", NOW + 2).unwrap();
        assert_eq!(by(), (None, None));
        act(&w.conn, &w.admin, &id, "close", NOW + 3).unwrap();
        let archived = act(&w.conn, &w.admin, &id, "archive", NOW + 4).unwrap().row;
        assert_eq!((by(), archived.can_edit, archived.locked, archived.owned), ((admin.clone(), admin.clone()), false, false, true), "an archived admin exam is read-only for everybody, but never 'locked'");
        act(&w.conn, &w.admin, &id, "restore", NOW + 5).unwrap();
        assert_eq!(by(), (admin.clone(), None), "restoring clears the archiver and keeps the closer");
        // the JSON of a row carries every field of the contract and none of the internals
        let v = serde_json::to_value(get_exam(&w.conn, &w.admin, &id, NOW).unwrap()).unwrap();
        for key in ["id", "phase", "can_edit", "owned", "locked", "submitted", "pending_answers", "visible", "hidden_reason", "questions", "sources"] {
            assert!(v.get(key).is_some(), "{key}");
        }
        assert!(v.get("closed_by").is_none() && v.get("archived_by").is_none());
        // visibility reasons come from the same rule students are served by
        act(&w.conn, &w.admin, &id, "reopen", NOW + 6).unwrap();
        w.conn.execute("UPDATE institutions SET is_active = 0", []).unwrap();
        let hidden = get_exam(&w.conn, &w.admin, &id, NOW).unwrap().row;
        assert_eq!((hidden.visible, hidden.hidden_reason), (false, Some(HiddenReason::InstitutionInactive)));
    }

    #[test]
    fn an_exam_cannot_be_published_into_a_hidden_institution_but_drafting_stays_possible() {
        let w = world();
        let draft = create_exam(&w.conn, &w.admin, &basic(json!({})), NOW).unwrap();
        let id = draft.row.info.id.clone();
        w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        let e = act(&w.conn, &w.admin, &id, "publish", NOW).unwrap_err();
        assert_eq!((e.0, e.1.contains("subject_inactive")), (StatusCode::BAD_REQUEST, true), "the subject is on, its institution is off");
        let e = create_exam(&w.conn, &w.admin, &basic(json!({"status": "published"})), NOW).unwrap_err();
        assert_eq!((e.0, e.1.contains("subject_inactive")), (StatusCode::BAD_REQUEST, true));
        assert!(create_exam(&w.conn, &w.admin, &basic(json!({"title": "مسودة أخرى"})), NOW).is_ok(), "a draft can still be prepared");
        w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        assert_eq!(act(&w.conn, &w.admin, &id, "publish", NOW).unwrap().row.info.status, "published");
    }
}
