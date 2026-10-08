//! Operations: content reports (with auto-hide), admin stats, audit viewer, unified search,
//! and account self-service (change password, delete account).

use crate::platform::{audit, bad, db_err, hash_password, lock, new_id, rate_limit, require_active, require_admin, text, verify_password, Res, User};
use crate::platform_content::visible;
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
        "assessment" => one("SELECT title, teacher_id FROM assessments WHERE id = ?1", &|r| {
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

fn create_report(conn: &Connection, reporter: &User, r: &ReportReq) -> Res<()> {
    let reason = text(&r.reason, 500, "invalid_reason")?;
    let target = target_info(conn, &r.target_type, &r.target_id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if target.owner_id == reporter.id {
        return Err(bad("cannot_report_own"));
    }
    rate_limit(conn, &format!("report:{}", reporter.id), 86_400_000, REPORTS_PER_DAY)?;
    conn.execute(
        "INSERT INTO reports(id, reporter_id, target_type, target_id, reason, status, created_at) VALUES (?1,?2,?3,?4,?5,'open',?6)",
        params![new_id(), reporter.id, r.target_type, r.target_id, reason, now_ms()],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => err(StatusCode::CONFLICT, "already_reported"),
        _ => db_err(e),
    })?;
    auto_hide_if_needed(conn, &r.target_type, &r.target_id, &target)?;
    Ok(())
}

/// ≥ N distinct reporters on open reports hide published content until an admin reviews it.
fn auto_hide_if_needed(conn: &Connection, ttype: &str, id: &str, target: &Target) -> Res<()> {
    let n: i64 = conn
        .query_row(
            "SELECT count(DISTINCT reporter_id) FROM reports WHERE target_type = ?1 AND target_id = ?2 AND status = 'open'",
            params![ttype, id],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if n < AUTO_HIDE_THRESHOLD {
        return Ok(());
    }
    let changed = match ttype {
        "post" => conn.execute("UPDATE posts SET status = 'draft' WHERE id = ?1 AND status = 'published'", params![id]),
        "course" => conn.execute("UPDATE courses SET status = 'draft' WHERE id = ?1 AND status = 'published'", params![id]),
        "assessment" => conn.execute("UPDATE assessments SET status = 'draft' WHERE id = ?1 AND status = 'published'", params![id]),
        "live" => conn.execute("UPDATE live_sessions SET status = 'cancelled' WHERE id = ?1 AND status = 'scheduled'", params![id]),
        _ => Ok(0),
    }
    .map_err(db_err)?;
    if changed > 0 {
        audit(conn, "", id, "auto_hidden_by_reports", ttype);
        crate::platform_engage::notify(conn, &target.owner_id, "content_unpublished", json!({ "title": target.title }), &target.link);
    }
    Ok(())
}

pub async fn report_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<ReportReq>) -> Res<StatusCode> {
    let u = require_active(&s, &h)?;
    create_report(&*lock(&s)?, &u, &r)?;
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
        pending_teaching: count(conn, "SELECT count(*) FROM teacher_subjects WHERE status = 'pending'")?,
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

// ───────── search ─────────

#[derive(Serialize, Debug)]
pub struct Hit {
    id: String,
    title: String,
    subtitle: String,
    link: String,
}

#[derive(Serialize, Debug)]
pub struct SearchResults {
    subjects: Vec<Hit>,
    teachers: Vec<Hit>,
    courses: Vec<Hit>,
    posts: Vec<Hit>,
}

fn like(q: &str) -> String {
    format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_").to_lowercase())
}

fn hits(conn: &Connection, sql: &str, pat: &str, f: &dyn Fn(&rusqlite::Row) -> rusqlite::Result<Hit>) -> Res<Vec<Hit>> {
    conn.prepare(sql)
        .map_err(db_err)?
        .query_map(params![pat], |r| f(r))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

fn search(conn: &Connection, q: &str) -> Res<SearchResults> {
    let q = q.trim();
    if q.chars().count() < 2 || q.chars().count() > 60 {
        return Err(bad("invalid_query"));
    }
    let pat = like(q);
    let subjects = hits(
        conn,
        "SELECT s.id, s.name_ar, i.name_ar FROM subjects s JOIN institutions i ON i.id = s.institution_id
         WHERE s.is_active = 1 AND i.is_active = 1 AND (lower(s.name_ar) LIKE ?1 ESCAPE '\\' OR lower(COALESCE(s.name_en,'')) LIKE ?1 ESCAPE '\\')
         ORDER BY s.name_ar LIMIT 8",
        &pat,
        &|r| Ok(Hit { id: r.get(0)?, title: r.get(1)?, subtitle: r.get(2)?, link: format!("/platform/subjects/{}", r.get::<_, String>(0)?) }),
    )?;
    let teachers = hits(
        conn,
        "SELECT id, full_name, COALESCE(substr(bio, 1, 80), '') FROM users
         WHERE role = 'teacher' AND status = 'active' AND lower(full_name) LIKE ?1 ESCAPE '\\' ORDER BY full_name LIMIT 8",
        &pat,
        &|r| Ok(Hit { id: r.get(0)?, title: r.get(1)?, subtitle: r.get(2)?, link: format!("/platform/teachers/{}", r.get::<_, String>(0)?) }),
    )?;
    let courses = hits(
        conn,
        &format!(
            "SELECT c.id, c.title, s.name_ar FROM courses c JOIN users u ON u.id = c.teacher_id JOIN subjects s ON s.id = c.subject_id
             WHERE {} AND lower(c.title) LIKE ?1 ESCAPE '\\' ORDER BY c.updated_at DESC LIMIT 8",
            visible("c", "published")
        ),
        &pat,
        &|r| Ok(Hit { id: r.get(0)?, title: r.get(1)?, subtitle: r.get(2)?, link: format!("/platform/courses/{}", r.get::<_, String>(0)?) }),
    )?;
    let posts = hits(
        conn,
        &format!(
            "SELECT p.id, p.title, s.name_ar FROM posts p JOIN users u ON u.id = p.teacher_id JOIN subjects s ON s.id = p.subject_id
             WHERE {} AND lower(p.title) LIKE ?1 ESCAPE '\\' ORDER BY p.updated_at DESC LIMIT 8",
            visible("p", "published")
        ),
        &pat,
        &|r| Ok(Hit { id: r.get(0)?, title: r.get(1)?, subtitle: r.get(2)?, link: format!("/platform/posts/{}", r.get::<_, String>(0)?) }),
    )?;
    Ok(SearchResults { subjects, teachers, courses, posts })
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

pub async fn search_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Query(q): Query<SearchQuery>) -> Res<Json<SearchResults>> {
    require_active(&s, &h)?;
    search(&*lock(&s)?, q.q.as_deref().unwrap_or("")).map(Json)
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
}

/// The last active admin cannot delete themselves (would lock the platform).
fn can_delete_account(conn: &Connection, user: &User) -> Res<()> {
    if user.role == "admin" {
        let others: i64 = conn
            .query_row("SELECT count(*) FROM users WHERE role = 'admin' AND status = 'active' AND id <> ?1", params![user.id], |r| r.get(0))
            .map_err(db_err)?;
        if others == 0 {
            return Err(err(StatusCode::CONFLICT, "last_admin"));
        }
    }
    Ok(())
}

pub async fn delete_account_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<DeleteReq>) -> Res<StatusCode> {
    let user = require_active(&s, &h)?;
    let phc = {
        let conn = lock(&s)?;
        rate_limit(&conn, &format!("pw:{}", user.id), 600_000, 10)?;
        can_delete_account(&conn, &user)?;
        stored_hash(&conn, &user.id)?
    };
    let ok = tokio::task::spawn_blocking(move || verify_password(&r.password, &phc)).await.map_err(db_err)?;
    if !ok {
        return Err(err(StatusCode::FORBIDDEN, "wrong_password"));
    }
    let conn = lock(&s)?;
    // Cascades remove the user's content, sessions, progress, reviews and notifications.
    conn.execute("DELETE FROM users WHERE id = ?1", params![user.id]).map_err(db_err)?;
    audit(&conn, "", &user.id, "account_deleted", &user.role);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

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
        conn.execute("INSERT INTO teacher_subjects VALUES (?1,'s1','approved',0,0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO posts VALUES ('p1', ?1, 's1', 'article', 'مقال عن البرمجة', 'نص', 'published', NULL, 0, 0)", params![teacher.id]).unwrap();
        conn.execute("INSERT INTO courses VALUES ('c1', ?1, 's1', 'دورة بايثون', NULL, 'published', 0, 0)", params![teacher.id]).unwrap();
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
        let n: i64 = w.conn.query_row("SELECT count(*) FROM notifications WHERE user_id = ?1 AND kind = 'content_unpublished'", params![w.teacher.id], |r| r.get(0)).unwrap();
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
        w.conn.execute("INSERT INTO teacher_subjects VALUES (?1,'s1','pending',0,NULL)", params![w.students[0].id]).unwrap();
        let s = stats(&w.conn, 1).unwrap();
        assert_eq!((s.pending_teachers, s.pending_teaching, s.subjects), (1, 1, 1));
        assert_eq!(s.users["student"], 4);
        assert_eq!(s.content["posts"], 1);
        assert_eq!(s.institutions["university"], 1);
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
}
