//! Operations: content reports (with auto-hide), admin stats, audit viewer and account self-service (change password,
//! delete account). (The unified search moved to `platform_search`.)

use crate::platform::{audit, bad, db_err, hash_password, lock, new_id, rate_limit, require_active, require_admin, text, verify_password, Res, User};
use crate::relay::{err, now_ms, sha256_hex};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::routes::AppState;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS reports (
  id TEXT PRIMARY KEY,
  reporter_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_type TEXT NOT NULL CHECK (target_type IN ('post','course','live','assessment','review','teacher')),
  target_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('open','resolved','dismissed')),
  note TEXT,
  created_at INTEGER NOT NULL,
  resolved_by TEXT,
  resolved_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_reports_target ON reports(target_type, target_id, status);
CREATE UNIQUE INDEX IF NOT EXISTS idx_reports_open_unique ON reports(reporter_id, target_type, target_id) WHERE status = 'open';
";

const REPORTS_PER_DAY: i64 = 20;
const AUTO_HIDE_THRESHOLD: i64 = 3;
const HOUR_MS: i64 = 3_600_000;

/// How old a reporter's account must be for its report to count towards the automatic hide (phase 4-2).
///
/// A burst of brand-new accounts (three registrations in a minute) must not be able to take someone's post, course, live
/// session or exam down: only reports from accounts at least `min_account_age_hours` old (default 24) are counted towards
/// [`AUTO_HIDE_THRESHOLD`]. The reports themselves are always stored and an admin sees all of them; only the automatic
/// consequence waits for reporters who have been around for a day. Read once at start
/// (`PLATFORM_REPORT_MIN_ACCOUNT_AGE_HOURS`, a whole number `>= 0`, `0` = every account counts, anything else keeps the
/// default rather than switching the protection off by accident).
#[derive(Clone, Debug, PartialEq)]
pub struct ReportLimits {
    pub min_account_age_hours: i64,
}

impl Default for ReportLimits {
    fn default() -> Self {
        ReportLimits { min_account_age_hours: 24 }
    }
}

impl ReportLimits {
    pub fn from_env() -> Self {
        Self::parse(|k| std::env::var(k).ok())
    }

    pub fn parse(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = ReportLimits::default();
        ReportLimits { min_account_age_hours: get("PLATFORM_REPORT_MIN_ACCOUNT_AGE_HOURS").and_then(|v| v.trim().parse::<u32>().ok()).map_or(d.min_account_age_hours, i64::from) }
    }

    pub fn describe(&self) -> String {
        if self.min_account_age_hours == 0 {
            "reports from every account count towards the automatic hide".to_string()
        } else {
            format!("only reports from accounts at least {} h old count towards the automatic hide", self.min_account_age_hours)
        }
    }

    /// The newest `users.created_at` a reporter may have for its report to count at `now`.
    fn newest_counting_account(&self, now: i64) -> i64 {
        if self.min_account_age_hours == 0 {
            i64::MAX
        } else {
            now.saturating_sub(self.min_account_age_hours.saturating_mul(HOUR_MS))
        }
    }
}

// ───────── reports ─────────

struct Target {
    title: String,
    link: String,
    owner_id: String,
}

/// Looks the target up and returns what the admin needs (and who owns it). None = does not exist.
fn target_info(conn: &Connection, ttype: &str, id: &str) -> Res<Option<Target>> {
    let one = |sql: &str, f: &dyn Fn(&rusqlite::Row) -> rusqlite::Result<Target>| -> Res<Option<Target>> {
        conn.query_row(sql, params![id], |r| f(r)).optional().map_err(db_err)
    };
    match ttype {
        "post" => one("SELECT title, teacher_id FROM posts WHERE id = ?1", &|r| {
            Ok(Target { title: r.get(0)?, link: format!("/platform/posts/{id}"), owner_id: r.get(1)? })
        }),
        "course" => one("SELECT title, teacher_id FROM courses WHERE id = ?1", &|r| {
            Ok(Target { title: r.get(0)?, link: format!("/platform/courses/{id}"), owner_id: r.get(1)? })
        }),
        "live" => one("SELECT title, teacher_id, subject_id FROM live_sessions WHERE id = ?1", &|r| {
            Ok(Target { title: r.get(0)?, link: format!("/platform/subjects/{}", r.get::<_, String>(2)?), owner_id: r.get(1)? })
        }),
        // An admin-built exam has no teacher (`teacher_id` NULL): its owner for "cannot report your own" and for the
        // "taken down" notice is the admin who created it, or nobody ('') if that account is gone.
        "assessment" => one("SELECT title, COALESCE(teacher_id, created_by, '') FROM assessments WHERE id = ?1", &|r| {
            Ok(Target { title: r.get(0)?, link: format!("/platform/assessments/{id}"), owner_id: r.get(1)? })
        }),
        "teacher" => one("SELECT full_name, id FROM users WHERE id = ?1 AND role = 'teacher'", &|r| {
            Ok(Target { title: r.get(0)?, link: format!("/platform/teachers/{id}"), owner_id: r.get(1)? })
        }),
        "review" => one(
            "SELECT substr(COALESCE(comment, ''), 1, 80), student_id, target_type, target_id FROM reviews WHERE id = ?1",
            &|r| {
                let (tt, tid): (String, String) = (r.get(2)?, r.get(3)?);
                let link = if tt == "course" { format!("/platform/courses/{tid}") } else { format!("/platform/teachers/{tid}") };
                Ok(Target { title: r.get(0)?, link, owner_id: r.get(1)? })
            },
        ),
        _ => Err(bad("invalid_target")),
    }
}

#[derive(Deserialize)]
pub struct ReportReq {
    target_type: String,
    target_id: String,
    reason: String,
}

fn create_report(conn: &Connection, reporter: &User, r: &ReportReq, limits: &ReportLimits, now: i64) -> Res<()> {
    let reason = text(&r.reason, 500, "invalid_reason")?;
    let target = target_info(conn, &r.target_type, &r.target_id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if target.owner_id == reporter.id {
        return Err(bad("cannot_report_own"));
    }
    rate_limit(conn, &format!("report:{}", reporter.id), 86_400_000, REPORTS_PER_DAY)?;
    conn.execute(
        "INSERT INTO reports(id, reporter_id, target_type, target_id, reason, status, created_at) VALUES (?1,?2,?3,?4,?5,'open',?6)",
        params![new_id(), reporter.id, r.target_type, r.target_id, reason, now],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => err(StatusCode::CONFLICT, "already_reported"),
        _ => db_err(e),
    })?;
    auto_hide_if_needed(conn, &r.target_type, &r.target_id, &target, limits, now)?;
    Ok(())
}

/// How many targets one pass of [`recount_matured_reports`] looks at.
const MATURED_BATCH: i64 = 200;

/// Counts again the reports that did not count when they came in because their reporter's account was too young
/// ([`ReportLimits`]) and whose reporter is old enough *now* - a class that registers together and reports an abusive post on
/// day 1 must not need a fourth, older reporter on day 2. Called by the background loop; returns how many targets it took
/// down (published content that now has [`AUTO_HIDE_THRESHOLD`] counting reporters).
///
/// Only a target with at least one report that was young when it was filed and is old enough now is looked at, and one
/// that was already taken down by reports since its oldest open report is left alone: content its owner published again
/// while the reports are still open is hidden again only by a NEW counting report (exactly as before), not by the clock.
/// Each target is its own transaction. A failure is logged and retried by the next pass.
pub fn recount_matured_reports(conn: &Connection, limits: &ReportLimits, now: i64) -> usize {
    if limits.min_account_age_hours == 0 {
        return 0; // everybody counts at once: nothing can mature
    }
    recount_matured(conn, limits, now).unwrap_or_else(|e| {
        eprintln!("Platform: recounting matured reports failed: {}", e.1);
        0
    })
}

fn recount_matured(conn: &Connection, limits: &ReportLimits, now: i64) -> Res<usize> {
    let age_ms = limits.min_account_age_hours.saturating_mul(HOUR_MS);
    let candidates: Vec<(String, String)> = conn
        .prepare(
            "SELECT r.target_type, r.target_id
               FROM reports r JOIN users u ON u.id = r.reporter_id
              WHERE r.status = 'open' AND r.target_type IN ('post','course','live','assessment')
                AND u.created_at > r.created_at - ?1   -- too young to count when it reported ...
                AND u.created_at <= ?2                  -- ... and old enough now
              GROUP BY r.target_type, r.target_id ORDER BY min(r.created_at) LIMIT ?3",
        )
        .map_err(db_err)?
        .query_map(params![age_ms, limits.newest_counting_account(now), MATURED_BATCH], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut hidden = 0;
    for (ttype, id) in candidates {
        let oldest: i64 = conn
            .query_row("SELECT min(created_at) FROM reports WHERE target_type = ?1 AND target_id = ?2 AND status = 'open'", params![ttype, id], |r| r.get(0))
            .map_err(db_err)?;
        let already: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM audit_log WHERE target_id = ?1 AND action = 'auto_hidden_by_reports' AND created_at >= ?2)",
                params![id, oldest],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if already {
            continue;
        }
        let Some(target) = target_info(conn, &ttype, &id)? else { continue };
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let changed = auto_hide_if_needed(&tx, &ttype, &id, &target, limits, now)?;
        tx.commit().map_err(db_err)?;
        hidden += usize::from(changed);
    }
    Ok(hidden)
}

/// ≥ N distinct reporters on open reports hide published content until an admin reviews it. Only reporters whose account is
/// old enough count ([`ReportLimits`]); the younger ones' reports stay open and visible to the admin.
fn auto_hide_if_needed(conn: &Connection, ttype: &str, id: &str, target: &Target, limits: &ReportLimits, now: i64) -> Res<bool> {
    let n: i64 = conn
        .query_row(
            "SELECT count(DISTINCT r.reporter_id) FROM reports r JOIN users u ON u.id = r.reporter_id
              WHERE r.target_type = ?1 AND r.target_id = ?2 AND r.status = 'open' AND u.created_at <= ?3",
            params![ttype, id, limits.newest_counting_account(now)],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if n < AUTO_HIDE_THRESHOLD {
        return Ok(false);
    }
    let changed = match ttype {
        "post" => conn.execute("UPDATE posts SET status = 'draft' WHERE id = ?1 AND status = 'published'", params![id]),
        "course" => conn.execute("UPDATE courses SET status = 'draft' WHERE id = ?1 AND status = 'published'", params![id]),
        "assessment" => {
            // An exam students have already started cannot go back to a draft (`update_assessment` refuses it with
            // `has_attempts`): it is *closed* instead, which also takes it down for new students but leaves the
            // attempts, the results and any sitting in progress intact. Without attempts it becomes a draft as before.
            let sat: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM attempts WHERE assessment_id = ?1)", params![id], |r| r.get(0)).map_err(db_err)?;
            if sat {
                let now = now_ms();
                // `closed_by` NULL = the system: the exam stays locked for its owner until an admin reopens or unlocks it.
                // Like any close, it ends every attempt still running (from its autosave) in the same transaction.
                conn.execute_batch("SAVEPOINT auto_hide_close").map_err(db_err)?;
                let closed = (|| -> Res<usize> {
                    let n = conn
                        .execute("UPDATE assessments SET status = 'closed', closed_at = ?2, closed_by = NULL, updated_at = ?2 WHERE id = ?1 AND status = 'published'", params![id, now])
                        .map_err(db_err)?;
                    if n > 0 {
                        crate::platform_exams::settle_on_close(conn, id, now)?;
                    }
                    Ok(n)
                })();
                match closed {
                    Ok(n) => {
                        conn.execute_batch("RELEASE auto_hide_close").map_err(db_err)?;
                        Ok(n)
                    }
                    Err(e) => {
                        let _ = conn.execute_batch("ROLLBACK TO auto_hide_close; RELEASE auto_hide_close");
                        return Err(e);
                    }
                }
            } else {
                conn.execute("UPDATE assessments SET status = 'draft' WHERE id = ?1 AND status = 'published'", params![id])
            }
        }
        "live" => conn.execute("UPDATE live_sessions SET status = 'cancelled' WHERE id = ?1 AND status = 'scheduled'", params![id]),
        _ => Ok(0),
    }
    .map_err(db_err)?;
    if changed > 0 {
        audit(conn, "", id, "auto_hidden_by_reports", ttype);
        // The owner hears that REPORTS took it down (`content_auto_hidden`); `content_unpublished` stays for an admin
        // doing it by hand.
        if !target.owner_id.is_empty() {
            crate::platform_engage::notify(conn, &target.owner_id, "content_auto_hidden", json!({ "title": target.title }), &target.link);
        }
        // A live session that was cancelled is cancelled for the people who were going to join it as well.
        if ttype == "live" {
            let session: Option<(String, String)> = conn
                .query_row(
                    "SELECT l.subject_id, u.full_name FROM live_sessions l JOIN users u ON u.id = l.teacher_id WHERE l.id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db_err)?;
            if let Some((subject_id, teacher_name)) = session {
                // a cancelled session has no page of its own (`GET /live/{id}` answers 404 for it): the audience, like the
                // owner, is sent to the subject's page
                crate::platform_engage::notify_audience(conn, &target.owner_id, &subject_id, "live_cancelled", json!({ "title": target.title, "teacher": teacher_name }), &format!("/platform/subjects/{subject_id}"));
            }
        }
    }
    Ok(changed > 0)
}

pub async fn report_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<ReportReq>) -> Res<StatusCode> {
    let u = require_active(&s, &h)?;
    create_report(&*lock(&s)?, &u, &r, &s.platform.report_limits, now_ms())?;
    Ok(StatusCode::CREATED)
}

#[derive(Serialize, Debug)]
pub struct ReportRow {
    id: String,
    target_type: String,
    target_id: String,
    title: String,
    link: String,
    reason: String,
    reporter: String,
    status: String,
    note: Option<String>,
    created_at: i64,
    open_reports_on_target: i64,
}

fn list_reports(conn: &Connection, status: Option<&str>) -> Res<Vec<ReportRow>> {
    let raw: Vec<(String, String, String, String, String, String, Option<String>, i64)> = conn
        .prepare(
            "SELECT r.id, r.target_type, r.target_id, r.reason, u.full_name, r.status, r.note, r.created_at
             FROM reports r JOIN users u ON u.id = r.reporter_id WHERE (?1 IS NULL OR r.status = ?1) ORDER BY r.created_at DESC LIMIT 200",
        )
        .map_err(db_err)?
        .query_map(params![status], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?, x.get(5)?, x.get(6)?, x.get(7)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    let mut out = Vec::with_capacity(raw.len());
    for (id, ttype, tid, reason, reporter, st, note, created) in raw {
        let info = target_info(conn, &ttype, &tid)?;
        let open: i64 = conn
            .query_row("SELECT count(*) FROM reports WHERE target_type = ?1 AND target_id = ?2 AND status = 'open'", params![ttype, tid], |r| r.get(0))
            .map_err(db_err)?;
        out.push(ReportRow {
            id,
            title: info.as_ref().map(|t| t.title.clone()).unwrap_or_default(),
            link: info.map(|t| t.link).unwrap_or_default(),
            target_type: ttype,
            target_id: tid,
            reason,
            reporter,
            status: st,
            note,
            created_at: created,
            open_reports_on_target: open,
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct ReportsQuery {
    status: Option<String>,
}

pub async fn list_reports_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<ReportsQuery>) -> Res<Json<Vec<ReportRow>>> {
    require_admin(&s, &h)?;
    list_reports(&*lock(&s)?, q.status.as_deref()).map(Json)
}

#[derive(Deserialize)]
pub struct ResolveReq {
    status: String,
    note: Option<String>,
}

/// Resolving/dismissing closes every open report on the same target.
fn resolve_report(conn: &Connection, admin: &User, id: &str, r: &ResolveReq) -> Res<()> {
    if !["resolved", "dismissed"].contains(&r.status.as_str()) {
        return Err(bad("invalid_status"));
    }
    let note = crate::platform::opt_text(&r.note, 500, "note_too_long")?;
    let (ttype, tid): (String, String) = conn
        .query_row("SELECT target_type, target_id FROM reports WHERE id = ?1", params![id], |x| Ok((x.get(0)?, x.get(1)?)))
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    conn.execute(
        "UPDATE reports SET status = ?1, note = ?2, resolved_by = ?3, resolved_at = ?4 WHERE target_type = ?5 AND target_id = ?6 AND status = 'open'",
        params![r.status, note, admin.id, now_ms(), ttype, tid],
    )
    .map_err(db_err)?;
    audit(conn, &admin.id, &tid, "reports_closed", &format!("{ttype}:{}", r.status));
    Ok(())
}

pub async fn resolve_report_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Path(id): Path<String>, Json(r): Json<ResolveReq>) -> Res<StatusCode> {
    let admin = require_admin(&s, &h)?;
    resolve_report(&*lock(&s)?, &admin, &id, &r)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── stats & audit ─────────

#[derive(Serialize)]
pub struct Stats {
    users: serde_json::Value,
    pending_teachers: i64,
    pending_teaching: i64,
    /// Written answers waiting for a manual grade (all exams).
    pending_grading: i64,
    open_reports: i64,
    signups_7d: i64,
    institutions: serde_json::Value,
    subjects: i64,
    content: serde_json::Value,
    attempts_submitted: i64,
    /// Active warnings of the system-status page (backups, disk); filled in by the handler.
    system_warnings: i64,
}

fn count(conn: &Connection, sql: &str) -> Res<i64> {
    conn.query_row(sql, [], |r| r.get(0)).map_err(db_err)
}

fn grouped(conn: &Connection, sql: &str) -> Res<serde_json::Value> {
    let rows: Vec<(String, i64)> = conn
        .prepare(sql)
        .map_err(db_err)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(db_err)?
        .collect::<Result<_, _>>()
        .map_err(db_err)?;
    Ok(serde_json::Value::Object(rows.into_iter().map(|(k, v)| (k, json!(v))).collect()))
}

fn stats(conn: &Connection, now: i64) -> Res<Stats> {
    Ok(Stats {
        users: grouped(conn, "SELECT role, count(*) FROM users GROUP BY role")?,
        pending_teachers: count(conn, "SELECT count(*) FROM users WHERE role = 'teacher' AND status = 'pending'")?,
        // Requests from accounts that are not active yet (a pending teacher may already ask for subjects) are not
        // actionable before the account itself is decided, so they do not inflate the badge; see `pending_teachers`.
        pending_teaching: count(
            conn,
            "SELECT count(*) FROM teacher_subjects ts JOIN users u ON u.id = ts.teacher_id WHERE ts.status = 'pending' AND u.status = 'active'",
        )?,
        pending_grading: count(conn, "SELECT COALESCE(SUM(pending), 0) FROM attempts WHERE status = 'submitted'")?,
        open_reports: count(conn, "SELECT count(DISTINCT target_type || target_id) FROM reports WHERE status = 'open'")?,
        signups_7d: conn
            .query_row("SELECT count(*) FROM users WHERE created_at > ?1", params![now - 7 * 86_400_000], |r| r.get(0))
            .map_err(db_err)?,
        institutions: grouped(conn, "SELECT type, count(*) FROM institutions GROUP BY type")?,
        subjects: count(conn, "SELECT count(*) FROM subjects")?,
        content: json!({
            "posts": count(conn, "SELECT count(*) FROM posts WHERE status = 'published'")?,
            "courses": count(conn, "SELECT count(*) FROM courses WHERE status = 'published'")?,
            "lessons": count(conn, "SELECT count(*) FROM lessons")?,
            "live": count(conn, "SELECT count(*) FROM live_sessions WHERE status = 'scheduled'")?,
            "assessments": count(conn, "SELECT count(*) FROM assessments WHERE status = 'published'")?,
        }),
        attempts_submitted: count(conn, "SELECT count(*) FROM attempts WHERE status = 'submitted'")?,
        system_warnings: 0,
    })
}

pub async fn stats_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Stats>> {
    require_admin(&s, &h)?;
    let now = now_ms();
    let mut st = stats(&*lock(&s)?, now)?;
    let state = s.clone();
    st.system_warnings = tokio::task::spawn_blocking(move || crate::platform_backup::warning_count(&state.platform, now)).await.unwrap_or(0);
    Ok(Json(st))
}

#[derive(Serialize, Debug)]
pub struct AuditRow {
    id: i64,
    actor: Option<String>,
    target_id: Option<String>,
    action: String,
    detail: Option<String>,
    created_at: i64,
}

#[derive(Deserialize)]
pub struct AuditQuery {
    before: Option<i64>,
}

fn list_audit(conn: &Connection, before: Option<i64>) -> Res<Vec<AuditRow>> {
    conn.prepare(
        "SELECT a.id, u.full_name, a.target_id, a.action, a.detail, a.created_at FROM audit_log a
         LEFT JOIN users u ON u.id = a.actor_id WHERE (?1 IS NULL OR a.id < ?1) ORDER BY a.id DESC LIMIT 50",
    )
    .map_err(db_err)?
    .query_map(params![before], |r| {
        Ok(AuditRow { id: r.get(0)?, actor: r.get(1)?, target_id: r.get(2)?, action: r.get(3)?, detail: r.get(4)?, created_at: r.get(5)? })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

pub async fn audit_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<AuditQuery>) -> Res<Json<Vec<AuditRow>>> {
    require_admin(&s, &h)?;
    list_audit(&*lock(&s)?, q.before).map(Json)
}

// ───────── account self-service ─────────

#[derive(Deserialize)]
pub struct PasswordReq {
    current: String,
    new: String,
}

fn stored_hash(conn: &Connection, user_id: &str) -> Res<String> {
    conn.query_row("SELECT password_hash FROM users WHERE id = ?1", params![user_id], |r| r.get(0)).map_err(db_err)
}

pub async fn change_password_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<PasswordReq>) -> Res<StatusCode> {
    let user = require_active(&s, &h)?;
    if r.new.chars().count() < 8 || r.new.chars().count() > 128 {
        return Err(bad("invalid_password"));
    }
    let (phc, current_token_hash) = {
        let conn = lock(&s)?;
        rate_limit(&conn, &format!("pw:{}", user.id), 600_000, 10)?;
        (stored_hash(&conn, &user.id)?, crate::platform::bearer(&h).map(sha256_hex).unwrap_or_default())
    };
    let (cur, new) = (r.current.clone(), r.new.clone());
    let new_hash = tokio::task::spawn_blocking(move || if verify_password(&cur, &phc) { hash_password(&new).map(Some) } else { Ok(None) })
        .await
        .map_err(db_err)??
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "wrong_password"))?;
    let conn = lock(&s)?;
    conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![new_hash, user.id]).map_err(db_err)?;
    // Sign out every other device; keep this one.
    conn.execute("DELETE FROM sessions WHERE user_id = ?1 AND token_hash <> ?2", params![user.id, current_token_hash]).map_err(db_err)?;
    audit(&conn, &user.id, &user.id, "password_changed", "");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct DeleteReq {
    password: String,
    /// The account's e-mail, typed again: required when the account has taken exams (`attempts_taken > 0`), whose
    /// results would be deleted with it (409 `confirm_required` otherwise). Compared trimmed and case-insensitively.
    #[serde(default)]
    confirm_email: Option<String>,
}

/// What deleting the caller's account would take with it — their own rows only (phase 3-1). For a teacher
/// `attempts` is the number of student attempts (any status) on their exams; while it is above zero the account
/// cannot be deleted (`delete_account` answers 409 `has_attempts`).
///
/// The student's side (phase 4-1; naturally zero for an account that never studied): `attempts_taken` = the caller's
/// own exam attempts (any status: results and attempts in progress alike), `lessons_completed`, `enrollments` and the
/// `reviews` they wrote. All of it goes with the account.
#[derive(Serialize, Debug, PartialEq)]
pub struct DeletionImpact {
    exams: i64,
    attempts: i64,
    courses: i64,
    posts: i64,
    live: i64,
    attempts_taken: i64,
    lessons_completed: i64,
    enrollments: i64,
    reviews: i64,
}

pub fn deletion_impact(conn: &Connection, user_id: &str) -> Res<DeletionImpact> {
    let n = |sql: &str| -> Res<i64> { conn.query_row(sql, params![user_id], |r| r.get(0)).map_err(db_err) };
    Ok(DeletionImpact {
        exams: n("SELECT count(*) FROM assessments WHERE teacher_id = ?1")?,
        attempts: n("SELECT count(*) FROM attempts t JOIN assessments a ON a.id = t.assessment_id WHERE a.teacher_id = ?1")?,
        courses: n("SELECT count(*) FROM courses WHERE teacher_id = ?1")?,
        posts: n("SELECT count(*) FROM posts WHERE teacher_id = ?1")?,
        live: n("SELECT count(*) FROM live_sessions WHERE teacher_id = ?1")?,
        attempts_taken: n("SELECT count(*) FROM attempts WHERE student_id = ?1")?,
        lessons_completed: n("SELECT count(*) FROM lesson_progress WHERE student_id = ?1")?,
        enrollments: n("SELECT count(*) FROM subject_enrollments WHERE student_id = ?1")?,
        reviews: n("SELECT count(*) FROM reviews WHERE student_id = ?1")?,
    })
}

/// An account that has taken exams loses every result with it, so a password alone is not enough: the e-mail must be
/// typed again (trimmed, any case). 409 `confirm_required` otherwise. Accounts without attempts need nothing more.
fn check_deletion_confirmed(conn: &Connection, user: &User, confirm_email: Option<&str>) -> Res<()> {
    if deletion_impact(conn, &user.id)?.attempts_taken == 0 {
        return Ok(());
    }
    let typed = confirm_email.map(|e| e.trim().to_lowercase()).unwrap_or_default();
    if typed.is_empty() || typed != user.email.trim().to_lowercase() {
        return Err(err(StatusCode::CONFLICT, "confirm_required"));
    }
    Ok(())
}

pub async fn deletion_impact_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<DeletionImpact>> {
    let user = require_active(&s, &h)?;
    deletion_impact(&*lock(&s)?, &user.id).map(Json)
}

/// Refuses to delete an account that cannot go: the last active admin (would lock the platform), and a teacher
/// whose exams students have already sat — deleting the teacher would delete those exams and every attempt and
/// grade with them. Closing or archiving such an exam does NOT lift the block (it is still the teacher's row and
/// still cascades); only removing the exam does, which an admin does from the exams screen after typing its title.
fn can_delete_account(conn: &Connection, user: &User) -> Res<()> {
    if user.role == "admin" {
        let others: i64 = conn
            .query_row("SELECT count(*) FROM users WHERE role = 'admin' AND status = 'active' AND id <> ?1", params![user.id], |r| r.get(0))
            .map_err(db_err)?;
        if others == 0 {
            return Err(err(StatusCode::CONFLICT, "last_admin"));
        }
    }
    if deletion_impact(conn, &user.id)?.attempts > 0 {
        return Err(err(StatusCode::CONFLICT, "has_attempts"));
    }
    Ok(())
}

/// Removes the reviews and reports that point at things the account's deletion is about to remove. Neither table
/// has a foreign key to its target (`target_id` is polymorphic), so they would be left behind as orphans: a rating
/// of a course that no longer exists, a report nobody can open. Runs inside the deletion's transaction, so it only
/// ever touches content that is really deleted.
fn purge_dangling_references(conn: &Connection, user_id: &str) -> Res<()> {
    // Reviews that will be gone: the ones the user wrote (cascade), the ones about their courses and about them.
    const GONE_REVIEWS: &str = "SELECT id FROM reviews WHERE student_id = ?1
         OR (target_type = 'course' AND target_id IN (SELECT id FROM courses WHERE teacher_id = ?1))
         OR (target_type = 'teacher' AND target_id = ?1)";
    // Reports about those reviews first (the subquery reads `reviews`, so it must run before they are deleted), and the
    // teachers' "new review" notices that quote them.
    conn.execute(&format!("DELETE FROM reports WHERE target_type = 'review' AND target_id IN ({GONE_REVIEWS})"), params![user_id]).map_err(db_err)?;
    crate::platform_engage::purge_review_notices(conn, GONE_REVIEWS, user_id)?;
    conn.execute(
        "DELETE FROM reviews WHERE (target_type = 'course' AND target_id IN (SELECT id FROM courses WHERE teacher_id = ?1))
            OR (target_type = 'teacher' AND target_id = ?1)",
        params![user_id],
    )
    .map_err(db_err)?;
    // Students' saved items (bookmarks) of the posts and courses that are about to be deleted. (The bookmarks the user
    // saved themself cascade with the account.)
    conn.execute(
        "DELETE FROM bookmarks WHERE (target_type = 'post' AND target_id IN (SELECT id FROM posts WHERE teacher_id = ?1))
            OR (target_type = 'course' AND target_id IN (SELECT id FROM courses WHERE teacher_id = ?1))",
        params![user_id],
    )
    .map_err(db_err)?;
    // Reports about their content and about the teacher themself.
    conn.execute(
        "DELETE FROM reports WHERE (target_type = 'post' AND target_id IN (SELECT id FROM posts WHERE teacher_id = ?1))
            OR (target_type = 'course' AND target_id IN (SELECT id FROM courses WHERE teacher_id = ?1))
            OR (target_type = 'live' AND target_id IN (SELECT id FROM live_sessions WHERE teacher_id = ?1))
            OR (target_type = 'assessment' AND target_id IN (SELECT id FROM assessments WHERE teacher_id = ?1))
            OR (target_type = 'teacher' AND target_id = ?1)",
        params![user_id],
    )
    .map_err(db_err)?;
    Ok(())
}

/// Removes the reviews and reports that point at ONE piece of content that is being deleted (`post`, `course` or
/// `live`), for the same reason as [`purge_dangling_references`]: neither table has a foreign key to its target.
/// A course takes its reviews, the reports about those reviews and the reports about the course itself with it; a post or a
/// course also takes the students' saved items (`bookmarks`) that point at it (phase 4-2).
/// The caller runs it in the same transaction as the delete, so it never touches content that survives.
pub(crate) fn purge_target_references(conn: &Connection, target_type: &str, target_id: &str) -> Res<()> {
    match target_type {
        "course" => {
            conn.execute(
                "DELETE FROM reports WHERE target_type = 'review'
                   AND target_id IN (SELECT id FROM reviews WHERE target_type = 'course' AND target_id = ?1)",
                params![target_id],
            )
            .map_err(db_err)?;
            crate::platform_engage::purge_review_notices(conn, "SELECT id FROM reviews WHERE target_type = 'course' AND target_id = ?1", target_id)?;
            conn.execute("DELETE FROM reviews WHERE target_type = 'course' AND target_id = ?1", params![target_id]).map_err(db_err)?;
        }
        "post" | "live" => {}
        _ => return Err(bad("invalid_target")),
    }
    if target_type != "live" {
        conn.execute("DELETE FROM bookmarks WHERE target_type = ?1 AND target_id = ?2", params![target_type, target_id]).map_err(db_err)?;
    }
    conn.execute("DELETE FROM reports WHERE target_type = ?1 AND target_id = ?2", params![target_type, target_id]).map_err(db_err)?;
    Ok(())
}

/// Deletes the account and everything it owns, in one transaction (all or nothing). The foreign keys cascade the
/// user's content, sessions, progress, enrolments and notifications; `purge_dangling_references` removes what the
/// foreign keys cannot reach. `assessments.teacher_id` is deliberately never set to NULL: NULL means "admin exam".
pub fn delete_account(conn: &Connection, user: &User) -> Res<()> {
    can_delete_account(conn, user)?;
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    purge_dangling_references(&tx, &user.id)?;
    tx.execute("DELETE FROM users WHERE id = ?1", params![user.id]).map_err(db_err)?;
    audit(&tx, "", &user.id, "account_deleted", &user.role);
    tx.commit().map_err(db_err)
}

/// Ids of the attachments the account owns: their bytes live in `<files dir>/<id>`, which the foreign key cascade
/// (it only removes the `files` rows) cannot reach.
fn owned_file_ids(conn: &Connection, user_id: &str) -> Res<Vec<String>> {
    conn.prepare("SELECT id FROM files WHERE owner_id = ?1")
        .map_err(db_err)?
        .query_map(params![user_id], |r| r.get(0))
        .map_err(db_err)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(db_err)
}

/// Best-effort removal of attachment bytes (a file that is already gone is fine). Ids come from our own table, but a
/// value that is not a plain file name is skipped rather than joined onto the directory.
fn remove_attachment_files(dir: &std::path::Path, ids: &[String]) {
    for id in ids {
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            let _ = std::fs::remove_file(dir.join(id));
        }
    }
}

/// `delete_account` plus the attachment bytes of the deleted account — only after the database deletion succeeded,
/// so a refused deletion (`has_attempts`, `last_admin`) never touches a file.
pub fn delete_account_and_files(conn: &Connection, files_dir: &std::path::Path, user: &User) -> Res<()> {
    let files = owned_file_ids(conn, &user.id)?;
    delete_account(conn, user)?;
    remove_attachment_files(files_dir, &files);
    Ok(())
}

/// What `DELETE /me` does once the password is verified: the typed e-mail when the account has taken exams
/// ([`check_deletion_confirmed`]), then [`delete_account_and_files`].
pub fn delete_account_confirmed(conn: &Connection, files_dir: &std::path::Path, user: &User, confirm_email: Option<&str>) -> Res<()> {
    check_deletion_confirmed(conn, user, confirm_email)?;
    delete_account_and_files(conn, files_dir, user)
}

pub async fn delete_account_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<DeleteReq>) -> Res<StatusCode> {
    let user = require_active(&s, &h)?;
    let phc = {
        let conn = lock(&s)?;
        rate_limit(&conn, &format!("pw:{}", user.id), 600_000, 10)?;
        can_delete_account(&conn, &user)?;
        stored_hash(&conn, &user.id)?
    };
    let password = r.password;
    let ok = tokio::task::spawn_blocking(move || verify_password(&password, &phc)).await.map_err(db_err)?;
    if !ok {
        return Err(err(StatusCode::FORBIDDEN, "wrong_password"));
    }
    // the password comes first (a wrong one never learns whether the e-mail would also be asked for)
    delete_account_confirmed(&*lock(&s)?, &s.platform.files_dir, &user, r.confirm_email.as_deref())?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    /// `create_report` with the default limits and the real clock (the fixtures' accounts were created at time 0, so they are
    /// old enough to count).
    fn create_report(conn: &Connection, reporter: &User, r: &ReportReq) -> Res<()> {
        super::create_report(conn, reporter, r, &ReportLimits::default(), now_ms())
    }

    /// The search as an admin sees it (no exams group; everything else is the same for every role).
    fn search(conn: &Connection, q: &str) -> Res<crate::platform_search::SearchResults> {
        let viewer = User { id: "viewer".into(), email: String::new(), full_name: "V".into(), role: "admin".into(), institution_type: None, status: "active".into(), status_reason: None };
        let query = crate::platform_search::SearchQuery { q: Some(q.into()), ..Default::default() };
        crate::platform_search::search(conn, &viewer, &query, now_ms())
    }

    struct W {
        conn: Connection,
        teacher: User,
        admin: User,
        students: Vec<User>,
    }

    fn world() -> W {
        let conn = create_test_db();
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة صنعاء',1,0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,name_en,is_active,created_at) VALUES ('s1','i1','برمجة الحاسوب','Programming',1,0)", []).unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let students: Vec<User> = (0..4).map(|i| insert_test_user(&conn, &format!("s{i}@x.com"), "student", "active")).collect();
        conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO posts VALUES ('p1', ?1, 's1', 'article', 'مقال عن البرمجة', 'نص', 'published', NULL, 0, 0, NULL)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO courses VALUES ('c1', ?1, 's1', 'دورة بايثون', NULL, 'published', 0, 0, NULL)", params![teacher.id]).unwrap();
        conn.execute("UPDATE users SET full_name = 'د. خالد البرمجي' WHERE id = ?1", params![teacher.id]).unwrap();
        W { conn, teacher, admin, students }
    }

    fn rep(t: &str, id: &str) -> ReportReq {
        ReportReq { target_type: t.into(), target_id: id.into(), reason: "محتوى مسيء".into() }
    }

    #[test]
    fn reports_validate_dedupe_and_auto_hide_at_three_distinct_reporters() {
        let w = world();
        assert_eq!(create_report(&w.conn, &w.students[0], &rep("post", "nope")).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(create_report(&w.conn, &w.students[0], &rep("bogus", "p1")).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create_report(&w.conn, &w.teacher, &rep("post", "p1")).unwrap_err().0, StatusCode::BAD_REQUEST, "own content");
        assert_eq!(create_report(&w.conn, &w.students[0], &ReportReq { reason: "  ".into(), ..rep("post", "p1") }).unwrap_err().0, StatusCode::BAD_REQUEST);
        create_report(&w.conn, &w.students[0], &rep("post", "p1")).unwrap();
        assert_eq!(create_report(&w.conn, &w.students[0], &rep("post", "p1")).unwrap_err().0, StatusCode::CONFLICT, "one open report per user per target");
        create_report(&w.conn, &w.students[1], &rep("post", "p1")).unwrap();
        let status = || -> String { w.conn.query_row("SELECT status FROM posts WHERE id = 'p1'", [], |r| r.get(0)).unwrap() };
        assert_eq!(status(), "published", "two reporters: still visible");
        create_report(&w.conn, &w.students[2], &rep("post", "p1")).unwrap();
        assert_eq!(status(), "draft", "three distinct reporters hide it");
        let n: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = 'content_auto_hidden'", params![w.teacher.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // other targets are unaffected; teacher reports never auto-hide
        for s in &w.students[..3] {
            create_report(&w.conn, s, &rep("teacher", &w.teacher.id)).unwrap();
        }
        let t: String = w.conn.query_row("SELECT status FROM users WHERE id = ?1", params![w.teacher.id], |r| r.get(0)).unwrap();
        assert_eq!(t, "active");
        let c: String = w.conn.query_row("SELECT status FROM courses WHERE id = 'c1'", [], |r| r.get(0)).unwrap();
        assert_eq!(c, "published");
    }

    #[test]
    fn admin_resolution_closes_all_reports_on_target_and_lists_titles() {
        let w = world();
        create_report(&w.conn, &w.students[0], &rep("course", "c1")).unwrap();
        create_report(&w.conn, &w.students[1], &rep("course", "c1")).unwrap();
        let open = list_reports(&w.conn, Some("open")).unwrap();
        assert_eq!((open.len(), open[0].title.as_str(), open[0].open_reports_on_target), (2, "دورة بايثون", 2));
        assert_eq!(resolve_report(&w.conn, &w.admin, &open[0].id, &ResolveReq { status: "bogus".into(), note: None }).unwrap_err().0, StatusCode::BAD_REQUEST);
        resolve_report(&w.conn, &w.admin, &open[0].id, &ResolveReq { status: "dismissed".into(), note: Some("لا مخالفة".into()) }).unwrap();
        assert!(list_reports(&w.conn, Some("open")).unwrap().is_empty());
        assert_eq!(list_reports(&w.conn, Some("dismissed")).unwrap().len(), 2);
        create_report(&w.conn, &w.students[0], &rep("course", "c1")).unwrap(); // can re-report after closure
        assert_eq!(stats(&w.conn, now_ms()).unwrap().open_reports, 1);
    }

    #[test]
    fn stats_count_correctly() {
        let w = world();
        insert_test_user(&w.conn, "p@x.com", "teacher", "pending");
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','pending',0,NULL)", params![w.students[0].id]).unwrap();
        let s = stats(&w.conn, 1).unwrap();
        assert_eq!((s.pending_teachers, s.pending_teaching, s.subjects), (1, 1, 1));
        assert_eq!(s.users["student"], 4);
        assert_eq!(s.content["posts"], 1);
        assert_eq!(s.institutions["university"], 1);
    }

    #[test]
    fn requests_from_accounts_that_are_not_active_yet_do_not_count_as_waiting_for_the_admin() {
        let w = world();
        w.conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s2','i1','مادة ثانية',1,0)", []).unwrap();
        let pending = insert_test_user(&w.conn, "p@x.com", "teacher", "pending");
        let rejected = insert_test_user(&w.conn, "r@x.com", "teacher", "rejected");
        for t in [&pending, &rejected] {
            for s in ["s1", "s2"] {
                w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,?2,'pending',0,NULL)", params![t.id, s]).unwrap();
            }
        }
        // an active teacher has one real request waiting and one already decided
        let active = insert_test_user(&w.conn, "a2@x.com", "teacher", "active");
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s2','pending',0,NULL)", params![active.id]).unwrap();
        w.conn.execute("INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at, decided_at) VALUES (?1,'s1','rejected',0,0)", params![active.id]).unwrap();
        let s = stats(&w.conn, 1).unwrap();
        assert_eq!(s.pending_teaching, 1, "only the active teacher's waiting request is actionable");
        assert_eq!(s.pending_teachers, 1, "the pending ACCOUNT is what the admin has to decide first");
        // approving the account makes its requests count
        w.conn.execute("UPDATE users SET status = 'active' WHERE id = ?1", params![pending.id]).unwrap();
        assert_eq!(stats(&w.conn, 1).unwrap().pending_teaching, 3);
    }

    #[test]
    fn search_is_case_and_wildcard_safe_and_only_shows_visible_content() {
        let w = world();
        let r = search(&w.conn, "برمج").unwrap();
        assert_eq!((r.subjects.len(), r.posts.len(), r.teachers.len(), r.courses.len()), (1, 1, 1, 0), "subject+post+teacher match the stem");
        assert_eq!(search(&w.conn, "PROGRAMMING").unwrap().subjects.len(), 1, "case-insensitive english name");
        assert_eq!(search(&w.conn, "بايثون").unwrap().courses.len(), 1);
        assert!(search(&w.conn, "%%").unwrap().subjects.is_empty(), "wildcards are literal");
        assert_eq!(search(&w.conn, "a").unwrap_err().0, StatusCode::BAD_REQUEST);
        w.conn.execute("UPDATE posts SET status = 'draft'", []).unwrap();
        assert!(search(&w.conn, "مقال").unwrap().posts.is_empty(), "drafts never searchable");
        w.conn.execute("UPDATE teacher_subjects SET status = 'rejected'", []).unwrap();
        assert!(search(&w.conn, "بايثون").unwrap().courses.is_empty(), "unapproved teacher's content hidden");
    }

    #[test]
    fn last_admin_cannot_delete_and_deletion_cascades() {
        let w = world();
        assert_eq!(can_delete_account(&w.conn, &w.admin).unwrap_err().0, StatusCode::CONFLICT);
        let second = insert_test_user(&w.conn, "a2@x.com", "admin", "active");
        assert!(can_delete_account(&w.conn, &w.admin).is_ok() && can_delete_account(&w.conn, &second).is_ok());
        w.conn.execute("DELETE FROM users WHERE id = ?1", params![w.teacher.id]).unwrap();
        let n: i64 = w.conn.query_row("SELECT (SELECT count(*) FROM posts)+(SELECT count(*) FROM courses)+(SELECT count(*) FROM teacher_subjects)", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "teacher content cascades with the account");
    }

    // ───── phase 3-1: account deletion safety ─────

    fn exam_of(conn: &Connection, id: &str, teacher: Option<&User>) {
        conn.execute(
            "INSERT INTO assessments(id,teacher_id,subject_id,title,questions,question_count,total_points,status,created_at,updated_at)
             VALUES (?1,?2,'s1','امتحان','[]',1,1,'published',0,0)",
            params![id, teacher.map(|t| t.id.clone())],
        )
        .unwrap();
    }

    fn attempt_on(conn: &Connection, id: &str, exam: &str, student: &User, status: &str) {
        conn.execute("INSERT INTO attempts(id,assessment_id,student_id,started_at,status) VALUES (?1,?2,?3,0,?4)", params![id, exam, student.id, status]).unwrap();
    }

    fn n(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn review(conn: &Connection, id: &str, student: &User, target_type: &str, target_id: &str) {
        conn.execute(
            "INSERT INTO reviews(id,student_id,target_type,target_id,rating,created_at,updated_at) VALUES (?1,?2,?3,?4,5,0,0)",
            params![id, student.id, target_type, target_id],
        )
        .unwrap();
    }

    fn report_on(conn: &Connection, id: &str, reporter: &User, target_type: &str, target_id: &str) {
        conn.execute(
            "INSERT INTO reports(id,reporter_id,target_type,target_id,reason,status,created_at) VALUES (?1,?2,?3,?4,'سبب','open',0)",
            params![id, reporter.id, target_type, target_id],
        )
        .unwrap();
    }

    fn status_of_exam(conn: &Connection, id: &str) -> (String, Option<i64>) {
        conn.query_row("SELECT status, closed_at FROM assessments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[test]
    fn an_exam_built_by_an_admin_can_be_reported_and_listed() {
        let w = world();
        exam_of(&w.conn, "e-admin", None); // teacher_id NULL = an admin exam
        w.conn.execute("UPDATE assessments SET created_by = ?1 WHERE id = 'e-admin'", params![w.admin.id]).unwrap();
        // used to be a 500 (the NULL owner was read as a String)
        create_report(&w.conn, &w.students[0], &rep("assessment", "e-admin")).unwrap();
        let rows = list_reports(&w.conn, Some("open")).unwrap();
        assert_eq!((rows.len(), rows[0].title.as_str(), rows[0].link.as_str()), (1, "امتحان", "/platform/assessments/e-admin"), "the admin's report list shows it too");
        // its creator is the "owner": the admin cannot report their own exam
        let e = create_report(&w.conn, &w.admin, &rep("assessment", "e-admin")).unwrap_err();
        assert_eq!((e.0, e.1.contains("cannot_report_own")), (StatusCode::BAD_REQUEST, true));
        // three distinct reporters take it down and the creator hears about it
        create_report(&w.conn, &w.students[1], &rep("assessment", "e-admin")).unwrap();
        create_report(&w.conn, &w.students[2], &rep("assessment", "e-admin")).unwrap();
        assert_eq!(status_of_exam(&w.conn, "e-admin").0, "draft");
        let told: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = 'content_auto_hidden'", params![w.admin.id], |r| r.get(0)).unwrap();
        assert_eq!(told, 1);
    }

    #[test]
    fn an_admin_exam_whose_creator_is_gone_is_still_reportable_and_notifies_nobody() {
        let w = world();
        exam_of(&w.conn, "e-orphan", None); // no teacher, no creator (the admin account was deleted)
        for s in &w.students[..3] {
            create_report(&w.conn, s, &rep("assessment", "e-orphan")).unwrap();
        }
        assert_eq!(status_of_exam(&w.conn, "e-orphan").0, "draft");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM notifications"), 0, "there is nobody to tell");
    }

    #[test]
    fn auto_hide_closes_an_exam_students_have_started_and_drafts_one_nobody_has() {
        let w = world();
        exam_of(&w.conn, "e-sat", Some(&w.teacher));
        exam_of(&w.conn, "e-fresh", Some(&w.teacher));
        attempt_on(&w.conn, "a1", "e-sat", &w.students[3], "in_progress");
        for target in ["e-sat", "e-fresh"] {
            for s in &w.students[..3] {
                create_report(&w.conn, s, &rep("assessment", target)).unwrap();
            }
        }
        // the invariant of update_assessment holds here too: a started exam never goes back to a draft
        let (st, closed_at) = status_of_exam(&w.conn, "e-sat");
        assert_eq!(st, "closed");
        assert!(closed_at.is_some(), "closed with a closing moment, like the lifecycle action");
        // (phase 4-3) closing is a hard end: the sitting in progress is ended in the same transaction - nothing was autosaved here, so
        // it is expired - while the exam itself, its other attempts and their grades stay as they are
        assert_eq!(n(&w.conn, "SELECT count(*) FROM attempts WHERE id = 'a1' AND status = 'in_progress'"), 0, "no sitting survives the close");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM attempts WHERE id = 'a1' AND status = 'expired'"), 1);
        assert_eq!(status_of_exam(&w.conn, "e-fresh"), ("draft".to_string(), None));
        // the teacher is told about both
        let told: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = 'content_auto_hidden'", params![w.teacher.id], |r| r.get(0)).unwrap();
        assert_eq!(told, 2);
        // a closed exam is not hidden a second time by a fourth report
        create_report(&w.conn, &w.students[3], &rep("assessment", "e-sat")).unwrap();
        assert_eq!(n(&w.conn, "SELECT count(*) FROM audit_log WHERE action = 'auto_hidden_by_reports'"), 2);
    }

    /// Phase 4-3: the report auto-hide is a close like any other - every sitting still running is ended in the same transaction,
    /// from its autosave, so nobody keeps writing into an exam the platform has taken down.
    #[test]
    fn auto_hide_ends_the_running_sittings_from_their_autosave() {
        let w = world();
        exam_of(&w.conn, "e-sat", Some(&w.teacher));
        w.conn.execute("UPDATE assessments SET questions = ?1, total_points = 2 WHERE id = 'e-sat'", params![r#"[{"id":"q1","type":"single_choice","stem":"س","options":["أ","ب"],"answer":"B","score":2}]"#]).unwrap();
        // one sitting with an autosave, one that never saved anything, one that is already finished
        attempt_on(&w.conn, "saved", "e-sat", &w.students[3], "in_progress");
        w.conn.execute("UPDATE attempts SET answers = '{\"q1\":\"B\"}', saved_at = 5 WHERE id = 'saved'", []).unwrap();
        let silent = insert_test_user(&w.conn, "silent@x.com", "student", "active");
        let done = insert_test_user(&w.conn, "done@x.com", "student", "active");
        attempt_on(&w.conn, "silent", "e-sat", &silent, "in_progress");
        attempt_on(&w.conn, "done", "e-sat", &done, "submitted");
        // really running (the helper starts attempts at the epoch, which would be long overdue)
        w.conn.execute("UPDATE attempts SET started_at = ?1 WHERE status = 'in_progress'", params![now_ms() - 5_000]).unwrap();
        for s in &w.students[..3] {
            create_report(&w.conn, s, &rep("assessment", "e-sat")).unwrap();
        }
        assert_eq!(status_of_exam(&w.conn, "e-sat").0, "closed");
        let row = |id: &str| -> (String, f64, Option<i64>) { w.conn.query_row("SELECT status, score, submitted_at FROM attempts WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap() };
        let (status, score, submitted_at) = row("saved");
        assert_eq!((status.as_str(), score), ("submitted", 2.0), "the autosave was graded");
        assert_eq!(submitted_at, status_of_exam(&w.conn, "e-sat").1, "stamped at the moment of the close");
        assert_eq!(row("silent").0, "expired");
        assert_eq!(row("done").0, "submitted");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM attempts WHERE status = 'in_progress'"), 0);
    }

    #[test]
    fn auto_hide_closes_with_no_actor_so_the_owner_stays_locked_out_until_an_admin_reopens() {
        let w = world();
        exam_of(&w.conn, "e-sat", Some(&w.teacher));
        // a stale actor from an earlier close / reopen must not survive: the system closed it this time
        w.conn.execute("UPDATE assessments SET closed_by = ?1, archived_by = ?1 WHERE id = 'e-sat'", params![w.teacher.id]).unwrap();
        attempt_on(&w.conn, "a1", "e-sat", &w.students[3], "submitted");
        for s in &w.students[..3] {
            create_report(&w.conn, s, &rep("assessment", "e-sat")).unwrap();
        }
        assert_eq!(status_of_exam(&w.conn, "e-sat").0, "closed");
        let (closed_by, archived_by): (Option<String>, Option<String>) = w.conn.query_row("SELECT closed_by, archived_by FROM assessments WHERE id = 'e-sat'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(closed_by, None, "NULL = the system");
        assert_eq!(archived_by.as_deref(), Some(w.teacher.id.as_str()), "only the closing actor is rewritten");
        let info = crate::platform_exams::get_info(&w.conn, &w.teacher.id, "e-sat").unwrap();
        assert!(info.is_locked(), "an exam the reports took down is not the owner's to put back");
        // an admin reopening it lifts the lock
        let reopened = crate::platform_exam_admin::act(&w.conn, &w.admin, "e-sat", "reopen", 5).unwrap();
        assert_eq!((reopened.row.info.status.as_str(), reopened.row.locked), ("published", false));
    }

    #[test]
    fn deletion_impact_counts_only_the_callers_own_rows() {
        let w = world();
        let other = insert_test_user(&w.conn, "t2@x.com", "teacher", "active");
        w.conn.execute("INSERT INTO posts VALUES ('p-other', ?1, 's1', 'article', 'منشور', 'نص', 'draft', NULL, 0, 0, NULL)", params![other.id]).unwrap();
        w.conn.execute("INSERT INTO courses VALUES ('c-other', ?1, 's1', 'دورة', NULL, 'draft', 0, 0, NULL)", params![other.id]).unwrap();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        exam_of(&w.conn, "e2", Some(&w.teacher));
        exam_of(&w.conn, "e-other", Some(&other));
        exam_of(&w.conn, "e-admin", None);
        w.conn.execute(
            "INSERT INTO live_sessions(id,teacher_id,subject_id,title,starts_at,duration_min,join_url,status,created_at) VALUES ('l1',?1,'s1','بث',0,60,'https://x.example','scheduled',0)",
            params![w.teacher.id],
        )
        .unwrap();
        // 3 attempts on the teacher's exams (any status), others elsewhere — and the teacher's own attempt as a student is not "theirs to lose"
        attempt_on(&w.conn, "a1", "e1", &w.students[0], "submitted");
        attempt_on(&w.conn, "a2", "e1", &w.students[1], "in_progress");
        attempt_on(&w.conn, "a3", "e2", &w.students[0], "expired");
        attempt_on(&w.conn, "a4", "e-other", &w.students[0], "submitted");
        attempt_on(&w.conn, "a5", "e-admin", &w.students[0], "submitted");
        // (phase 4-1 added the student's side: attempts_taken / lessons_completed / enrollments / reviews, zero here)
        let teacher_only = |exams, attempts, courses, posts, live| DeletionImpact { exams, attempts, courses, posts, live, attempts_taken: 0, lessons_completed: 0, enrollments: 0, reviews: 0 };
        assert_eq!(deletion_impact(&w.conn, &w.teacher.id).unwrap(), teacher_only(2, 3, 1, 1, 1));
        let other_impact = deletion_impact(&w.conn, &other.id).unwrap();
        assert_eq!((other_impact.exams, other_impact.attempts, other_impact.courses, other_impact.posts, other_impact.live), (1, 1, 1, 1, 0));
        // students and admins own none of the teacher's data (a student's own attempts are their side, phase 4-1)
        assert_eq!(
            deletion_impact(&w.conn, &w.students[0].id).unwrap(),
            DeletionImpact { exams: 0, attempts: 0, courses: 0, posts: 0, live: 0, attempts_taken: 4, lessons_completed: 0, enrollments: 0, reviews: 0 }
        );
        assert_eq!(deletion_impact(&w.conn, &w.admin.id).unwrap(), teacher_only(0, 0, 0, 0, 0));
        assert_eq!(
            serde_json::to_value(deletion_impact(&w.conn, &w.teacher.id).unwrap()).unwrap(),
            serde_json::json!({"exams": 2, "attempts": 3, "courses": 1, "posts": 1, "live": 1,
                               "attempts_taken": 0, "lessons_completed": 0, "enrollments": 0, "reviews": 0}),
            "the agreed JSON shape"
        );
    }

    #[test]
    fn a_teacher_whose_exams_students_have_sat_cannot_delete_the_account() {
        let w = world();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        attempt_on(&w.conn, "a1", "e1", &w.students[0], "in_progress"); // even an unfinished attempt counts
        let e = delete_account(&w.conn, &w.teacher).unwrap_err();
        assert_eq!(e.0, StatusCode::CONFLICT);
        assert!(e.1.contains("has_attempts"), "{}", e.1);
        // nothing at all was removed: account, content, exam, attempt
        let account = format!("SELECT count(*) FROM users WHERE id = '{}'", w.teacher.id);
        assert_eq!(n(&w.conn, &account), 1);
        assert_eq!(n(&w.conn, "SELECT (SELECT count(*) FROM posts) + (SELECT count(*) FROM courses) + (SELECT count(*) FROM assessments) + (SELECT count(*) FROM attempts)"), 4);
        assert_eq!(n(&w.conn, "SELECT count(*) FROM audit_log WHERE action = 'account_deleted'"), 0);
        // the pre-check the HTTP handler runs before it even asks for the password says the same
        assert!(can_delete_account(&w.conn, &w.teacher).unwrap_err().1.contains("has_attempts"));
        // once the attempts are gone (an admin deleted/moved the exam) the account can go
        w.conn.execute("DELETE FROM attempts", []).unwrap();
        delete_account(&w.conn, &w.teacher).unwrap();
        assert_eq!(n(&w.conn, &account), 0);
        assert_eq!(n(&w.conn, "SELECT (SELECT count(*) FROM posts) + (SELECT count(*) FROM courses) + (SELECT count(*) FROM assessments)"), 0, "an exam nobody sat goes with its teacher");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM audit_log WHERE action = 'account_deleted'"), 1);
    }

    #[test]
    fn deleting_a_teacher_never_turns_their_exams_into_admin_exams() {
        let w = world();
        exam_of(&w.conn, "e-teacher", Some(&w.teacher));
        exam_of(&w.conn, "e-admin", None);
        attempt_on(&w.conn, "a-admin", "e-admin", &w.students[0], "submitted");
        delete_account(&w.conn, &w.teacher).unwrap();
        assert_eq!(n(&w.conn, "SELECT count(*) FROM assessments WHERE id = 'e-teacher'"), 0, "gone, not orphaned with a NULL teacher");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM assessments WHERE teacher_id IS NULL"), 1, "only the genuine admin exam has no teacher");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM attempts WHERE id = 'a-admin'"), 1, "an admin exam's attempts are untouched");
    }

    #[test]
    fn deleting_a_teacher_removes_the_reviews_and_reports_that_pointed_at_what_was_deleted() {
        let w = world();
        let (s0, s1, s2) = (&w.students[0], &w.students[1], &w.students[2]);
        let other = insert_test_user(&w.conn, "t2@x.com", "teacher", "active");
        w.conn.execute("INSERT INTO posts VALUES ('p2', ?1, 's1', 'article', 'منشور آخر', 'نص', 'published', NULL, 0, 0, NULL)", params![other.id]).unwrap();
        w.conn.execute("INSERT INTO courses VALUES ('c2', ?1, 's1', 'دورة أخرى', NULL, 'published', 0, 0, NULL)", params![other.id]).unwrap();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        exam_of(&w.conn, "e2", Some(&other));
        w.conn.execute(
            "INSERT INTO live_sessions(id,teacher_id,subject_id,title,starts_at,duration_min,join_url,status,created_at) VALUES ('l1',?1,'s1','بث',0,60,'https://x.example','scheduled',0), ('l2',?2,'s1','بث',0,60,'https://x.example','scheduled',0)",
            params![w.teacher.id, other.id],
        )
        .unwrap();
        // reviews: of the teacher's course, of the teacher, and (kept) of the other teacher's course and of the other teacher
        review(&w.conn, "r-course", s0, "course", "c1");
        review(&w.conn, "r-teacher", s1, "teacher", &w.teacher.id);
        review(&w.conn, "r-other-course", s0, "course", "c2");
        review(&w.conn, "r-other-teacher", s1, "teacher", &other.id);
        // reports on every kind of target: the teacher's (all go) and the other teacher's (all stay)
        for (i, (t, id)) in [("post", "p1"), ("course", "c1"), ("live", "l1"), ("assessment", "e1"), ("teacher", w.teacher.id.as_str()), ("review", "r-course"), ("review", "r-teacher")].iter().enumerate() {
            report_on(&w.conn, &format!("rep-mine-{i}"), s2, t, id);
        }
        for (i, (t, id)) in [("post", "p2"), ("course", "c2"), ("live", "l2"), ("assessment", "e2"), ("teacher", other.id.as_str()), ("review", "r-other-course"), ("review", "r-other-teacher")].iter().enumerate() {
            report_on(&w.conn, &format!("rep-other-{i}"), s2, t, id);
        }
        delete_account(&w.conn, &w.teacher).unwrap();
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reviews WHERE id IN ('r-course','r-teacher')"), 0, "reviews of the deleted course and teacher are gone");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reviews WHERE id IN ('r-other-course','r-other-teacher')"), 2, "other reviews are kept");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reports WHERE id LIKE 'rep-mine-%'"), 0, "every report about the deleted content, review and teacher is gone");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reports WHERE id LIKE 'rep-other-%'"), 7, "reports about content that still exists are untouched");
        // nothing is left pointing at a row that no longer exists
        assert_eq!(
            n(&w.conn, "SELECT count(*) FROM reports r WHERE (r.target_type = 'post' AND NOT EXISTS(SELECT 1 FROM posts WHERE id = r.target_id))
                 OR (r.target_type = 'course' AND NOT EXISTS(SELECT 1 FROM courses WHERE id = r.target_id))
                 OR (r.target_type = 'live' AND NOT EXISTS(SELECT 1 FROM live_sessions WHERE id = r.target_id))
                 OR (r.target_type = 'assessment' AND NOT EXISTS(SELECT 1 FROM assessments WHERE id = r.target_id))
                 OR (r.target_type = 'review' AND NOT EXISTS(SELECT 1 FROM reviews WHERE id = r.target_id))
                 OR (r.target_type = 'teacher' AND NOT EXISTS(SELECT 1 FROM users WHERE id = r.target_id))"),
            0
        );
    }

    /// The notice a teacher got for a review (`platform_engage::upsert_review` writes it: the quoted comment and the review's id).
    fn review_notice(conn: &Connection, to: &User, review_id: &str, comment: &str) {
        conn.execute(
            "INSERT INTO notifications(id, user_id, kind, data, link, created_at) VALUES (?1,?2,'new_review',?3,'/platform/courses/c1',1)",
            params![format!("n-{review_id}"), to.id, serde_json::json!({ "rating": 5, "target": "course", "title": "دورة", "comment": comment, "review": review_id }).to_string()],
        )
        .unwrap();
    }

    #[test]
    fn the_quoted_comment_of_a_review_goes_with_the_review_when_its_author_or_its_course_is_deleted() {
        let w = world();
        let (author, other) = (&w.students[0], &w.students[1]);
        review(&w.conn, "r-mine", author, "course", "c1");
        review(&w.conn, "r-theirs", other, "course", "c1");
        review_notice(&w.conn, &w.teacher, "r-mine", "تعليق شخصي");
        review_notice(&w.conn, &w.teacher, "r-theirs", "تعليق آخر");
        w.conn.execute("UPDATE notifications SET read_at = 5 WHERE id = 'n-r-mine'", []).unwrap(); // a READ notice keeps the text too
        // a notice from before the link existed has no review id and is left alone
        w.conn.execute("INSERT INTO notifications(id, user_id, kind, data, created_at) VALUES ('old', ?1, 'new_review', '{\"rating\":4,\"target\":\"teacher\"}', 1)", params![w.teacher.id]).unwrap();
        let left = |w: &W| -> Vec<String> {
            w.conn.prepare("SELECT id FROM notifications WHERE kind = 'new_review' ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(left(&w).len(), 3);
        // the author's account goes: their review, and what the teacher was told about it, go with it
        delete_account(&w.conn, author).unwrap();
        assert_eq!(left(&w), ["n-r-theirs", "old"]);
        // the course goes: the other student's review does, and so does the quote
        let tx = w.conn.unchecked_transaction().unwrap();
        purge_target_references(&tx, "course", "c1").unwrap();
        tx.commit().unwrap();
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reviews WHERE target_id = 'c1'"), 0);
        assert_eq!(left(&w), ["old"]);
    }

    #[test]
    fn deleting_a_student_removes_reports_about_their_own_reviews_and_nothing_else() {
        let w = world();
        let (author, other, reporter) = (&w.students[0], &w.students[1], &w.students[2]);
        review(&w.conn, "r-mine", author, "course", "c1");
        review(&w.conn, "r-theirs", other, "course", "c1");
        report_on(&w.conn, "rep-1", reporter, "review", "r-mine");
        report_on(&w.conn, "rep-2", reporter, "review", "r-theirs");
        report_on(&w.conn, "rep-3", author, "post", "p1"); // by the author: cascades with the account
        delete_account(&w.conn, author).unwrap();
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reviews WHERE id = 'r-mine'"), 0);
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reports WHERE id IN ('rep-1','rep-3')"), 0, "report about the deleted review, and the report they filed");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM reports WHERE id = 'rep-2'"), 1, "an unrelated report stays");
        assert_eq!(n(&w.conn, "SELECT (SELECT count(*) FROM posts) + (SELECT count(*) FROM courses)"), 2, "the teacher's content is not touched");
    }

    fn scratch_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("exameow-del-{}", new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn attach(conn: &Connection, dir: &std::path::Path, id: &str, owner: &User) {
        conn.execute("INSERT INTO files(id,owner_id,original_name,mime,size,created_at) VALUES (?1,?2,'ملخص.pdf','application/pdf',3,0)", params![id, owner.id]).unwrap();
        std::fs::write(dir.join(id), b"pdf").unwrap();
    }

    #[test]
    fn deleting_an_account_also_deletes_its_attachment_bytes_but_only_its_own() {
        let w = world();
        let other = insert_test_user(&w.conn, "t2@x.com", "teacher", "active");
        let dir = scratch_dir();
        attach(&w.conn, &dir, "f-mine-1", &w.teacher);
        attach(&w.conn, &dir, "f-mine-2", &w.teacher);
        attach(&w.conn, &dir, "f-other", &other);
        w.conn.execute("UPDATE posts SET file_id = 'f-mine-1' WHERE id = 'p1'", []).unwrap();
        delete_account_and_files(&w.conn, &dir, &w.teacher).unwrap();
        assert_eq!(n(&w.conn, "SELECT count(*) FROM files"), 1, "rows: only the other teacher's remains");
        assert!(!dir.join("f-mine-1").exists() && !dir.join("f-mine-2").exists(), "the bytes of the deleted account are gone");
        assert!(dir.join("f-other").exists(), "another account's attachment is untouched");
        // an attachment whose bytes are already missing does not fail the deletion
        attach(&w.conn, &dir, "f-gone", &other);
        std::fs::remove_file(dir.join("f-gone")).unwrap();
        delete_account_and_files(&w.conn, &dir, &other).unwrap();
        assert!(!dir.join("f-other").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_deletion_never_touches_the_files() {
        let w = world();
        let dir = scratch_dir();
        attach(&w.conn, &dir, "f1", &w.teacher);
        exam_of(&w.conn, "e1", Some(&w.teacher));
        attempt_on(&w.conn, "a1", "e1", &w.students[0], "submitted");
        assert!(delete_account_and_files(&w.conn, &dir, &w.teacher).is_err());
        assert!(dir.join("f1").exists());
        assert_eq!(n(&w.conn, "SELECT count(*) FROM files"), 1);
        // the last admin is refused the same way
        attach(&w.conn, &dir, "f2", &w.admin);
        assert!(delete_account_and_files(&w.conn, &dir, &w.admin).is_err());
        assert!(dir.join("f2").exists());
        // names that are not plain file names are never joined onto the directory
        remove_attachment_files(&dir, &["../escape".to_string(), "".to_string(), "a/b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn closing_or_archiving_an_exam_does_not_lift_the_account_deletion_block() {
        let w = world();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        attempt_on(&w.conn, "a1", "e1", &w.students[0], "submitted");
        for status in ["closed", "archived"] {
            w.conn.execute("UPDATE assessments SET status = ?1 WHERE id = 'e1'", params![status]).unwrap();
            let e = can_delete_account(&w.conn, &w.teacher).unwrap_err();
            assert_eq!((e.0, e.1.contains("has_attempts")), (StatusCode::CONFLICT, true), "{status}: the exam and its results would still be deleted with the account");
            assert_eq!(deletion_impact(&w.conn, &w.teacher.id).unwrap().attempts, 1, "{status}");
        }
        // only removing the exam (the admin's typed-title delete) clears it
        w.conn.execute("DELETE FROM assessments WHERE id = 'e1'", []).unwrap();
        assert!(can_delete_account(&w.conn, &w.teacher).is_ok());
    }

    #[test]
    fn a_failed_deletion_leaves_reviews_and_reports_alone() {
        let w = world();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        attempt_on(&w.conn, "a1", "e1", &w.students[0], "submitted");
        review(&w.conn, "r1", &w.students[1], "course", "c1");
        report_on(&w.conn, "rep1", &w.students[2], "course", "c1");
        assert!(delete_account(&w.conn, &w.teacher).is_err());
        assert_eq!(n(&w.conn, "SELECT (SELECT count(*) FROM reviews) + (SELECT count(*) FROM reports)"), 2, "the purge never runs for an account that stays");
    }

    #[test]
    fn audit_pagination() {
        let w = world();
        for i in 0..60 {
            audit(&w.conn, &w.admin.id, "t", "x", &i.to_string());
        }
        let page1 = list_audit(&w.conn, None).unwrap();
        assert_eq!(page1.len(), 50);
        let page2 = list_audit(&w.conn, Some(page1.last().unwrap().id)).unwrap();
        assert_eq!(page2.len(), 10);
        assert_eq!(page1[0].actor.as_deref(), Some("T"));
    }

    // ───── phase 3-2: a hidden institution leaves the search; content cleanup helper ─────

    #[test]
    fn a_hidden_institution_takes_its_subjects_courses_and_posts_out_of_the_search() {
        let w = world();
        let found = |q: &str| {
            let r = search(&w.conn, q).unwrap();
            (r.subjects.len(), r.courses.len(), r.posts.len())
        };
        assert_eq!((found("برمج"), found("بايثون")), ((1, 0, 1), (0, 1, 0)));
        w.conn.execute("UPDATE institutions SET is_active = 0 WHERE id = 'i1'", []).unwrap();
        assert_eq!((found("برمج"), found("بايثون")), ((0, 0, 0), (0, 0, 0)), "subject, course and post are all gone from the results");
        assert_eq!(search(&w.conn, "خالد").unwrap().teachers.len(), 1, "the person is still findable; their page just lists nothing hidden");
        w.conn.execute("UPDATE institutions SET is_active = 1 WHERE id = 'i1'", []).unwrap();
        assert_eq!(found("بايثون"), (0, 1, 0));
    }

    #[test]
    fn purging_the_references_of_one_item_knows_exactly_three_kinds_and_touches_only_that_item() {
        let w = world();
        let reporter = &w.students[0];
        let report = |id: &str, ttype: &str, tid: &str| {
            w.conn.execute("INSERT INTO reports(id,reporter_id,target_type,target_id,reason,status,created_at) VALUES (?1,?2,?3,?4,'سبب','open',0)", params![id, reporter.id, ttype, tid]).unwrap();
        };
        report("r1", "post", "p1");
        report("r2", "post", "other");
        report("r3", "assessment", "p1"); // same id, another kind of target: never matched
        purge_target_references(&w.conn, "post", "p1").unwrap();
        let left: Vec<String> = w.conn.prepare("SELECT id FROM reports ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(left, ["r2", "r3"]);
        for unknown in ["review", "teacher", "assessment", "", "posts"] {
            assert_eq!(purge_target_references(&w.conn, unknown, "x").unwrap_err().0, StatusCode::BAD_REQUEST, "{unknown:?}");
        }
        purge_target_references(&w.conn, "live", "nothing").unwrap(); // nothing to remove is fine
    }

    // ───── phase 3-4: `content_auto_hidden` is not `content_unpublished` ─────

    #[test]
    fn auto_hide_tells_the_owner_content_auto_hidden_for_every_kind_and_never_content_unpublished() {
        let w = world();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        w.conn.execute("INSERT INTO live_sessions VALUES ('l1', ?1, 's1', 'بث مباشر', NULL, ?2, 60, 'https://meet.example.com/x', 'scheduled', 0)", params![w.teacher.id, now_ms() + 3_600_000]).unwrap();
        w.conn.execute("INSERT INTO subject_enrollments VALUES (?1, 's1', 0)", params![w.students[3].id]).unwrap();
        for (kind, id) in [("post", "p1"), ("course", "c1"), ("live", "l1"), ("assessment", "e1")] {
            for s in &w.students[..3] {
                create_report(&w.conn, s, &rep(kind, id)).unwrap();
            }
        }
        let mine = |kind: &str| -> Vec<(String, String)> {
            w.conn
                .prepare("SELECT data, link FROM notifications WHERE user_id = ?1 AND kind = ?2 ORDER BY link")
                .unwrap()
                .query_map(params![w.teacher.id, kind], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let hidden = mine("content_auto_hidden");
        assert_eq!(hidden.len(), 4, "one per piece of content that went down: {hidden:?}");
        let links: Vec<&str> = hidden.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(links, ["/platform/assessments/e1", "/platform/courses/c1", "/platform/posts/p1", "/platform/subjects/s1"], "the link of each target is the one it always had");
        for (data, _) in &hidden {
            let d: serde_json::Value = serde_json::from_str(data).unwrap();
            assert_eq!(d.as_object().unwrap().keys().collect::<Vec<_>>(), ["title"], "just the title");
        }
        assert!(mine("content_unpublished").is_empty(), "that kind is for an admin doing it by hand");
        // the exam still closes / drafts exactly as before
        assert_eq!(status_of_exam(&w.conn, "e1").0, "draft");
        // a cancelled live session is cancelled for the people who were going to join as well - not for its teacher
        let (told, title): (i64, String) = w.conn.query_row("SELECT count(*), max(data) FROM notifications WHERE user_id = ?1 AND kind = 'live_cancelled'", params![w.students[3].id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(told, 1);
        let d: serde_json::Value = serde_json::from_str(&title).unwrap();
        assert_eq!((d["title"].as_str(), d["teacher"].as_str()), (Some("بث مباشر"), Some("د. خالد البرمجي")));
        // a cancelled session has no page of its own (`GET /live/{id}` answers 404 for it): the notice leads to the subject
        let link: String = w.conn.query_row("SELECT link FROM notifications WHERE user_id = ?1 AND kind = 'live_cancelled'", params![w.students[3].id], |r| r.get(0)).unwrap();
        assert_eq!(link, "/platform/subjects/s1");
        assert_eq!(n(&w.conn, &format!("SELECT count(*) FROM notifications WHERE user_id = '{}' AND kind = 'live_cancelled'", w.teacher.id)), 0);
    }

    // ───────── phase 4-1: the student's side of an account deletion ─────────

    #[test]
    fn deletion_impact_counts_the_students_own_attempts_lessons_enrolments_and_reviews() {
        let w = world();
        let (me, other) = (&w.students[0], &w.students[1]);
        exam_of(&w.conn, "e1", Some(&w.teacher));
        exam_of(&w.conn, "e2", None);
        for (i, status) in ["submitted", "expired", "in_progress"].iter().enumerate() {
            attempt_on(&w.conn, &format!("m{i}"), if i == 0 { "e1" } else { "e2" }, me, status);
        }
        attempt_on(&w.conn, "o1", "e1", other, "submitted");
        w.conn.execute("INSERT INTO lessons(id,course_id,position,title,video_url,video_kind,created_at) VALUES ('l1','c1',1,'د','https://x.example/v','link',0), ('l2','c1',2,'د','https://x.example/v','link',0)", []).unwrap();
        for (who, lesson) in [(me, "l1"), (me, "l2"), (other, "l1")] {
            w.conn.execute("INSERT INTO lesson_progress(student_id,lesson_id,completed_at) VALUES (?1,?2,0)", params![who.id, lesson]).unwrap();
        }
        for who in [me, other] {
            w.conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,'s1',0)", params![who.id]).unwrap();
        }
        review(&w.conn, "r1", me, "course", "c1");
        review(&w.conn, "r2", me, "teacher", &w.teacher.id);
        review(&w.conn, "r3", other, "course", "c1");
        let mine = deletion_impact(&w.conn, &me.id).unwrap();
        assert_eq!(
            mine,
            DeletionImpact { exams: 0, attempts: 0, courses: 0, posts: 0, live: 0, attempts_taken: 3, lessons_completed: 2, enrollments: 1, reviews: 2 },
            "any status counts: every one of them goes with the account"
        );
        let t = deletion_impact(&w.conn, &w.teacher.id).unwrap();
        assert_eq!((t.attempts_taken, t.lessons_completed, t.enrollments, t.reviews), (0, 0, 0, 0), "a teacher's student side is zero");
        assert_eq!((t.exams, t.attempts), (1, 2), "and the teacher's side is unchanged");
    }

    #[test]
    fn a_student_with_attempts_must_type_the_email_to_delete_the_account() {
        let w = world();
        let me = w.students[0].clone();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        attempt_on(&w.conn, "a1", "e1", &me, "expired"); // even an expired attempt is a record that would go
        let dir = scratch_dir();
        for typed in [None, Some(""), Some("   "), Some("s1@x.com"), Some("s0@x.co"), Some("other@x.com")] {
            let e = delete_account_confirmed(&w.conn, &dir, &me, typed).unwrap_err();
            assert_eq!((e.0, e.1.contains("confirm_required")), (StatusCode::CONFLICT, true), "{typed:?}");
        }
        assert_eq!(n(&w.conn, &format!("SELECT count(*) FROM users WHERE id = '{}'", me.id)), 1, "nothing was deleted");
        assert_eq!(n(&w.conn, "SELECT count(*) FROM attempts WHERE id = 'a1'"), 1);
        // trimmed and in any case
        delete_account_confirmed(&w.conn, &dir, &me, Some("  S0@X.Com \n")).unwrap();
        assert_eq!(n(&w.conn, &format!("SELECT count(*) FROM users WHERE id = '{}'", me.id)), 0);
        assert_eq!(n(&w.conn, "SELECT count(*) FROM attempts WHERE id = 'a1'"), 0);
        // without attempts nothing more is asked (a student who only enrolled, a teacher, an admin with a colleague)
        let quiet = &w.students[1];
        w.conn.execute("INSERT INTO subject_enrollments(student_id,subject_id,created_at) VALUES (?1,'s1',0)", params![quiet.id]).unwrap();
        delete_account_confirmed(&w.conn, &dir, quiet, None).unwrap();
        let t2 = insert_test_user(&w.conn, "t2@x.com", "teacher", "active");
        delete_account_confirmed(&w.conn, &dir, &t2, None).unwrap();
        // the older refusals still come first in their own right
        insert_test_user(&w.conn, "a2@x.com", "admin", "active");
        delete_account_confirmed(&w.conn, &dir, &w.admin, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The order of the checks of `DELETE /me` through the real handler: the password first (a stolen session token must
    /// not learn from `confirm_required` that the account has taken exams), then the typed e-mail.
    #[tokio::test]
    async fn the_handler_checks_the_password_before_it_asks_for_the_email() {
        use crate::platform::{hash_password, PlatformState};
        use axum::http::header;
        static CFG: std::sync::Once = std::sync::Once::new();
        CFG.call_once(|| std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join(format!("exameow-http-test-{}", new_id()))));
        let w = world();
        let me = w.students[0].clone();
        w.conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![hash_password("correct-horse-1").unwrap(), me.id]).unwrap();
        exam_of(&w.conn, "e1", Some(&w.teacher));
        attempt_on(&w.conn, "a1", "e1", &me, "submitted"); // the account has taken an exam: the e-mail is asked for
        let token = format!("tok-{}", new_id());
        w.conn
            .execute("INSERT INTO sessions(token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, 0, ?3)", params![sha256_hex(&token), me.id, now_ms() + 3_600_000])
            .unwrap();
        let state = Arc::new(AppState {
            config_store: exameow_core::config::ConfigStore::new("ExameowServerHttpTest").expect("a scratch config dir"),
            relay: crate::relay::init_db(":memory:").unwrap(),
            platform: PlatformState::for_tests(w.conn),
            admin_token: std::sync::Mutex::new("pass".into()),
            legacy: crate::legacy_guard::LegacyGuard::new(60, 2000),
            auth_failures: crate::token_limit::TokenLimiter::new(600),
        });
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        let call = |password: &str, email: Option<&str>| {
            let (state, headers) = (state.clone(), headers.clone());
            let r = DeleteReq { password: password.into(), confirm_email: email.map(str::to_string) };
            async move { delete_account_handler(State(state), headers, Json(r)).await }
        };
        let code = |e: (StatusCode, String)| (e.0, e.1.contains("wrong_password"), e.1.contains("confirm_required"));
        // wrong password: always 403 wrong_password, whether the e-mail is missing, wrong or right
        for email in [None, Some("nobody@x.com"), Some("s0@x.com")] {
            assert_eq!(code(call("not-the-password", email).await.unwrap_err()), (StatusCode::FORBIDDEN, true, false), "wrong password, e-mail {email:?}");
        }
        // right password, e-mail missing or wrong: 409 confirm_required - and nothing was deleted
        for email in [None, Some(""), Some("nobody@x.com")] {
            assert_eq!(code(call("correct-horse-1", email).await.unwrap_err()), (StatusCode::CONFLICT, false, true), "right password, e-mail {email:?}");
        }
        let conn = state.platform.conn.lock().unwrap();
        assert_eq!(n(&conn, "SELECT count(*) FROM users WHERE email = 's0@x.com'"), 1);
        assert_eq!(n(&conn, "SELECT count(*) FROM attempts WHERE id = 'a1'"), 1);
        drop(conn);
        // right password and the e-mail typed again (any case): 204, and the account and its results are gone
        assert_eq!(call("correct-horse-1", Some(" S0@X.com ")).await.unwrap(), StatusCode::NO_CONTENT);
        let conn = state.platform.conn.lock().unwrap();
        assert_eq!(n(&conn, "SELECT count(*) FROM users WHERE email = 's0@x.com'"), 0);
        assert_eq!(n(&conn, "SELECT count(*) FROM attempts WHERE id = 'a1'"), 0);
    }
}

/// Phase 4-2: only reports from accounts that have been around for a while count towards the automatic hide.
#[cfg(test)]
mod reporter_age_tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    const NOW: i64 = 1_800_000_000_000;
    const H: i64 = 3_600_000;

    struct W {
        conn: Connection,
    }

    fn world() -> W {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة',1,0);
             INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s1','i1','برمجة',1,0);",
        )
        .unwrap();
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "active");
        conn.execute("INSERT INTO teacher_subjects(teacher_id,subject_id,status,created_at,decided_at) VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO posts(id,teacher_id,subject_id,kind,title,body,status,created_at,updated_at) VALUES ('p1',?1,'s1','article','مقال','نص','published',0,0)", params![teacher.id]).unwrap();
        W { conn }
    }

    /// A student whose account is `age` old at `NOW`.
    fn student(w: &W, n: &str, age: i64) -> User {
        let u = insert_test_user(&w.conn, &format!("{n}@x.com"), "student", "active");
        w.conn.execute("UPDATE users SET created_at = ?2 WHERE id = ?1", params![u.id, NOW - age]).unwrap();
        u
    }

    fn report(w: &W, who: &User, limits: &ReportLimits) {
        let r = ReportReq { target_type: "post".into(), target_id: "p1".into(), reason: "مسيء".into() };
        super::create_report(&w.conn, who, &r, limits, NOW).unwrap();
    }

    fn status(w: &W) -> String {
        w.conn.query_row("SELECT status FROM posts WHERE id = 'p1'", [], |r| r.get(0)).unwrap()
    }

    fn open_reports(w: &W) -> i64 {
        w.conn.query_row("SELECT count(*) FROM reports WHERE target_id = 'p1' AND status = 'open'", [], |r| r.get(0)).unwrap()
    }

    fn hours(h: i64) -> ReportLimits {
        ReportLimits { min_account_age_hours: h }
    }

    #[test]
    fn reports_from_young_accounts_are_stored_but_do_not_count_towards_the_hide() {
        let w = world();
        let limits = ReportLimits::default();
        let old = [student(&w, "o1", 30 * H), student(&w, "o2", 25 * H)];
        let young = [student(&w, "y1", H), student(&w, "y2", 5 * 60_000), student(&w, "y3", 0)];
        for u in &young {
            report(&w, u, &limits);
        }
        assert_eq!((status(&w), open_reports(&w)), ("published".to_string(), 3), "three brand-new accounts cannot take a post down");
        report(&w, &old[0], &limits);
        report(&w, &old[1], &limits);
        assert_eq!((status(&w), open_reports(&w)), ("published".to_string(), 5), "two old reporters are still short of three, whatever the young ones did");
        let third = student(&w, "o3", 48 * H);
        report(&w, &third, &limits);
        assert_eq!(status(&w), "draft", "the third ESTABLISHED account tips it");
        assert_eq!(open_reports(&w), 6, "and every report is still there for the admin");
        let notified: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'content_auto_hidden'", [], |r| r.get(0)).unwrap();
        assert_eq!(notified, 1);
        // an admin sees all six
        let all = list_reports(&w.conn, Some("open")).unwrap();
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn the_boundary_is_inclusive_to_the_millisecond() {
        let limits = ReportLimits::default();
        for (age, counts) in [(24 * H, true), (24 * H - 1, false), (24 * H + 1, true)] {
            let w = world();
            let reporters = [student(&w, "a", 100 * H), student(&w, "b", 100 * H), student(&w, "edge", age)];
            for u in &reporters {
                report(&w, u, &limits);
            }
            assert_eq!(status(&w) == "draft", counts, "an account {age} ms old");
        }
    }

    #[test]
    fn zero_switches_the_age_rule_off() {
        let w = world();
        let off = hours(0);
        for n in 0..3 {
            report(&w, &student(&w, &format!("n{n}"), 0), &off);
        }
        assert_eq!(status(&w), "draft", "with 0 every account counts, even one created this instant");
    }

    #[test]
    fn the_age_is_configurable() {
        let w = world();
        let limits = hours(72);
        let reporters = [student(&w, "a", 80 * H), student(&w, "b", 73 * H), student(&w, "c", 71 * H), student(&w, "d", 30 * H)];
        for u in &reporters {
            report(&w, u, &limits);
        }
        assert_eq!(status(&w), "published", "two of them are 72 hours old, the others are not");
        report(&w, &student(&w, "e", 99 * H), &limits);
        assert_eq!(status(&w), "draft");
    }

    #[test]
    fn the_age_rule_applies_to_every_kind_of_target() {
        // a course is hidden by the same rule as a post
        let w = world();
        let teacher_id: String = w.conn.query_row("SELECT id FROM users WHERE role = 'teacher'", [], |r| r.get(0)).unwrap();
        w.conn.execute("INSERT INTO courses(id,teacher_id,subject_id,title,status,created_at,updated_at) VALUES ('c1',?1,'s1','دورة','published',0,0)", params![teacher_id]).unwrap();
        let limits = ReportLimits::default();
        let r = ReportReq { target_type: "course".into(), target_id: "c1".into(), reason: "مسيء".into() };
        for n in 0..3 {
            super::create_report(&w.conn, &student(&w, &format!("y{n}"), H), &r, &limits, NOW).unwrap();
        }
        let st = |w: &W| -> String { w.conn.query_row("SELECT status FROM courses WHERE id = 'c1'", [], |r| r.get(0)).unwrap() };
        assert_eq!(st(&w), "published");
        for n in 0..3 {
            super::create_report(&w.conn, &student(&w, &format!("o{n}"), 40 * H), &r, &limits, NOW).unwrap();
        }
        assert_eq!(st(&w), "draft");
    }

    // ───────── reporters that come of age (phase 4-2 review) ─────────

    /// Files a report as of `at`; the reporter's account is `age` old at that moment.
    fn report_at(w: &W, n: &str, age: i64, at: i64, limits: &ReportLimits) -> User {
        let u = insert_test_user(&w.conn, &format!("{n}@x.com"), "student", "active");
        w.conn.execute("UPDATE users SET created_at = ?2 WHERE id = ?1", params![u.id, at - age]).unwrap();
        let r = ReportReq { target_type: "post".into(), target_id: "p1".into(), reason: "مسيء".into() };
        super::create_report(&w.conn, &u, &r, limits, at).unwrap();
        u
    }

    fn auto_hide_notices(w: &W) -> i64 {
        w.conn.query_row("SELECT count(*) FROM notifications WHERE kind = 'content_auto_hidden'", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn reports_from_young_accounts_count_once_the_accounts_are_old_enough() {
        let w = world();
        let limits = ReportLimits::default();
        let t = now_ms();
        // a class registered an hour before it reported: three reports, none counts yet
        for n in 0..3 {
            report_at(&w, &format!("y{n}"), H, t, &limits);
        }
        assert_eq!((status(&w), open_reports(&w)), ("published".to_string(), 3));
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + H), 0, "still young");
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 23 * H - 1), 0, "one millisecond short of 24 hours old");
        assert_eq!(status(&w), "published");
        // exactly 24 hours old: all three count, and the post goes down at the next pass
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 23 * H), 1);
        assert_eq!(status(&w), "draft");
        assert_eq!(auto_hide_notices(&w), 1, "the owner is told once");
        let audit: i64 = w.conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'auto_hidden_by_reports' AND target_id = 'p1'", [], |r| r.get(0)).unwrap();
        assert_eq!(audit, 1);
        // and no later pass repeats it
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 30 * H), 0);
        assert_eq!((auto_hide_notices(&w), open_reports(&w)), (1, 3), "the reports stay open for the admin");
    }

    #[test]
    fn a_mix_of_old_and_matured_reporters_reaches_the_threshold_when_the_young_ones_mature() {
        let w = world();
        let limits = ReportLimits::default();
        let t = now_ms();
        report_at(&w, "o1", 50 * H, t, &limits);
        report_at(&w, "o2", 50 * H, t, &limits);
        report_at(&w, "y1", 2 * H, t, &limits);
        assert_eq!(status(&w), "published", "two established accounts are short of three");
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 5 * H), 0);
        assert_eq!(status(&w), "published");
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 22 * H), 1, "the third reporter is 24 hours old");
        assert_eq!(status(&w), "draft");
    }

    #[test]
    fn only_two_matured_reporters_are_still_short_of_three() {
        let w = world();
        let limits = ReportLimits::default();
        let t = now_ms();
        report_at(&w, "y1", H, t, &limits);
        report_at(&w, "y2", H, t, &limits);
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 40 * H), 0);
        assert_eq!(status(&w), "published");
    }

    #[test]
    fn the_clock_never_hides_content_again_that_its_owner_published_after_a_hide() {
        let w = world();
        let limits = ReportLimits::default();
        let t = now_ms();
        for n in 0..3 {
            report_at(&w, &format!("y{n}"), H, t, &limits);
        }
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 23 * H), 1);
        // the owner publishes it again while the reports are still open: the clock does not take it down a second time
        w.conn.execute("UPDATE posts SET status = 'published' WHERE id = 'p1'", []).unwrap();
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 50 * H), 0);
        assert_eq!(status(&w), "published");
        // reports that counted when they were filed are not the clock's business either
        let w = world();
        let t = now_ms();
        for n in 0..3 {
            report_at(&w, &format!("o{n}"), 40 * H, t, &limits);
        }
        assert_eq!(status(&w), "draft", "hidden at the third established report, as before");
        w.conn.execute("UPDATE posts SET status = 'published' WHERE id = 'p1'", []).unwrap();
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 100 * H), 0);
        assert_eq!(status(&w), "published");
        // a new established report still hides it again, exactly as it always did
        report_at(&w, "o9", 40 * H, t, &limits);
        assert_eq!(status(&w), "draft");
    }

    #[test]
    fn reports_that_an_admin_closed_do_not_count_later() {
        let w = world();
        let limits = ReportLimits::default();
        let t = now_ms();
        for n in 0..3 {
            report_at(&w, &format!("y{n}"), H, t, &limits);
        }
        w.conn.execute("UPDATE reports SET status = 'dismissed'", []).unwrap();
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 40 * H), 0);
        assert_eq!(status(&w), "published");
    }

    #[test]
    fn with_the_age_rule_off_there_is_nothing_to_mature() {
        let w = world();
        let t = now_ms();
        for n in 0..2 {
            report_at(&w, &format!("y{n}"), H, t, &hours(24));
        }
        assert_eq!(recount_matured_reports(&w.conn, &hours(0), t + 40 * H), 0);
        assert_eq!(status(&w), "published");
    }

    #[test]
    fn matured_reports_hide_every_kind_of_target() {
        let w = world();
        let teacher_id: String = w.conn.query_row("SELECT id FROM users WHERE role = 'teacher'", [], |r| r.get(0)).unwrap();
        w.conn.execute("INSERT INTO courses(id,teacher_id,subject_id,title,status,created_at,updated_at) VALUES ('c1',?1,'s1','دورة','published',0,0)", params![teacher_id]).unwrap();
        w.conn
            .execute("INSERT INTO live_sessions(id,teacher_id,subject_id,title,starts_at,duration_min,join_url,status,created_at) VALUES ('l1',?1,'s1','بث',?2,60,'https://m.example.com/x','scheduled',0)", params![teacher_id, now_ms() + 40 * H])
            .unwrap();
        let limits = ReportLimits::default();
        let t = now_ms();
        for (kind, id) in [("course", "c1"), ("live", "l1")] {
            let r = ReportReq { target_type: kind.into(), target_id: id.into(), reason: "مسيء".into() };
            for n in 0..3 {
                let u = insert_test_user(&w.conn, &format!("{kind}{n}@x.com"), "student", "active");
                w.conn.execute("UPDATE users SET created_at = ?2 WHERE id = ?1", params![u.id, t - H]).unwrap();
                super::create_report(&w.conn, &u, &r, &limits, t).unwrap();
            }
        }
        assert_eq!(recount_matured_reports(&w.conn, &limits, t + 23 * H), 2);
        let st = |table: &str, id: &str| -> String { w.conn.query_row(&format!("SELECT status FROM {table} WHERE id = ?1"), params![id], |r| r.get(0)).unwrap() };
        assert_eq!((st("courses", "c1").as_str(), st("live_sessions", "l1").as_str()), ("draft", "cancelled"));
    }

    #[test]
    fn the_setting_is_read_once_from_the_environment_and_garbage_keeps_the_default() {
        let parse = |v: Option<&str>| ReportLimits::parse(|k| {
            assert_eq!(k, "PLATFORM_REPORT_MIN_ACCOUNT_AGE_HOURS");
            v.map(String::from)
        });
        assert_eq!(parse(None), hours(24));
        assert_eq!(parse(Some("24")), hours(24));
        assert_eq!(parse(Some("0")), hours(0));
        assert_eq!(parse(Some(" 48 ")), hours(48));
        assert_eq!(parse(Some("1")), hours(1));
        for garbage in ["", "abc", "-1", "1.5", "12h", "0x10", "99999999999999", "٣"] {
            assert_eq!(parse(Some(garbage)), hours(24), "{garbage:?} must not switch the protection off by accident");
        }
        assert_eq!(ReportLimits::default(), hours(24));
        assert!(hours(24).describe().contains("24") && hours(0).describe().contains("every account"));
        // a huge value cannot overflow the arithmetic
        assert_eq!(hours(u32::MAX as i64).newest_counting_account(NOW) < NOW, true);
        assert_eq!(hours(0).newest_counting_account(NOW), i64::MAX);
    }
}
