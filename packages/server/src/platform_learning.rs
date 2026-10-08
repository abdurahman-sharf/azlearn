//! Teaching assignments (teacher <-> subject, admin-approved), student placement and
//! enrollment, following teachers, public teacher/subject pages.

use crate::platform::{audit, authenticate, bad, db_err, lock, opt_text, require_active, require_admin, require_role, Res, User};
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS teacher_subjects (
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected')),
  created_at INTEGER NOT NULL,
  decided_at INTEGER,
  PRIMARY KEY (teacher_id, subject_id)
);
CREATE INDEX IF NOT EXISTS idx_ts_subject ON teacher_subjects(subject_id, status);
CREATE TABLE IF NOT EXISTS student_placement (
  student_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  institution_id TEXT NOT NULL REFERENCES institutions(id) ON DELETE CASCADE,
  unit_id TEXT REFERENCES org_units(id) ON DELETE SET NULL
);
CREATE TABLE IF NOT EXISTS subject_enrollments (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  subject_id TEXT NOT NULL REFERENCES subjects(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, subject_id)
);
CREATE INDEX IF NOT EXISTS idx_enroll_subject ON subject_enrollments(subject_id);
CREATE TABLE IF NOT EXISTS teacher_follows (
  student_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  teacher_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (student_id, teacher_id)
);
CREATE INDEX IF NOT EXISTS idx_follow_teacher ON teacher_follows(teacher_id);
";

// ───────── profile ─────────

#[derive(Deserialize, Default)]
pub struct ProfilePatch {
    full_name: Option<String>,
    bio: Option<String>,
    /// `school`, `institute` or `university`; anything else is refused. There is no way to clear it here.
    institution_type: Option<String>,
}

/// The kinds of institution a person can say they belong to (same list as `institutions.type`).
pub(crate) fn valid_institution_type(raw: &str) -> Res<&str> {
    match raw {
        t @ ("school" | "institute" | "university") => Ok(t),
        _ => Err(bad("invalid_type")),
    }
}

/// Edits the caller's own profile row (`user.id` comes from the session). Every field is validated before the
/// first write, so a bad value never leaves a half-applied change.
fn update_profile(conn: &Connection, user: &User, p: &ProfilePatch) -> Res<()> {
    let name = match &p.full_name {
        Some(n) => {
            let t = n.trim();
            if t.is_empty() || t.chars().count() > 120 {
                return Err(bad("invalid_name"));
            }
            Some(t.to_string())
        }
        None => None,
    };
    let bio = if p.bio.is_some() { Some(opt_text(&p.bio, 1000, "bio_too_long")?) } else { None };
    let inst = match &p.institution_type {
        Some(t) => Some(valid_institution_type(t)?),
        None => None,
    };
    if let Some(n) = name {
        conn.execute("UPDATE users SET full_name = ?1 WHERE id = ?2", params![n, user.id]).map_err(db_err)?;
    }
    if let Some(b) = bio {
        conn.execute("UPDATE users SET bio = ?1 WHERE id = ?2", params![b, user.id]).map_err(db_err)?;
    }
    if let Some(t) = inst {
        conn.execute("UPDATE users SET institution_type = ?1 WHERE id = ?2", params![t, user.id]).map_err(db_err)?;
    }
    Ok(())
}

pub async fn update_profile_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(p): Json<ProfilePatch>,
) -> Res<StatusCode> {
    let user = require_active(&state, &headers)?;
    update_profile(&*lock(&state)?, &user, &p)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── teaching assignments ─────────

#[derive(Serialize, Debug)]
pub struct Teaching {
    teacher_id: String,
    teacher_name: String,
    subject_id: String,
    subject_name: String,
    institution_id: String,
    institution_name: String,
    status: String,
}

/// The shared assignment query; `extra` adds columns after the standard seven (e.g. `, u.status`).
fn teaching_select(extra: &str) -> String {
    format!(
        "SELECT ts.teacher_id, u.full_name, ts.subject_id, s.name_ar, i.id, i.name_ar, ts.status{extra}
     FROM teacher_subjects ts
     JOIN users u ON u.id = ts.teacher_id
     JOIN subjects s ON s.id = ts.subject_id
     JOIN institutions i ON i.id = s.institution_id"
    )
}

/// An assignment row as the admin sees it: the standard fields plus the state of the teacher's *account*, so a
/// request from a teacher who is still waiting for approval is recognisable.
#[derive(Serialize, Debug)]
pub struct AdminTeaching {
    #[serde(flatten)]
    base: Teaching,
    teacher_status: String,
}

fn map_teaching(r: &rusqlite::Row) -> rusqlite::Result<Teaching> {
    Ok(Teaching {
        teacher_id: r.get(0)?,
        teacher_name: r.get(1)?,
        subject_id: r.get(2)?,
        subject_name: r.get(3)?,
        institution_id: r.get(4)?,
        institution_name: r.get(5)?,
        status: r.get(6)?,
    })
}

fn my_teaching(conn: &Connection, teacher_id: &str) -> Res<Vec<Teaching>> {
    conn.prepare(&format!("{} WHERE ts.teacher_id = ?1 ORDER BY i.name_ar, s.name_ar", teaching_select("")))
        .map_err(db_err)?
        .query_map(params![teacher_id], map_teaching)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

pub async fn my_teaching_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Vec<Teaching>>> {
    let user = may_request_teaching(authenticate(&state, &headers)?)?;
    my_teaching(&*lock(&state)?, &user.id).map(Json)
}

#[derive(Deserialize)]
pub struct SubjectRef {
    subject_id: String,
}

/// How many requests a teacher may have waiting for the admin at once. Accounts are open to self-registration and a
/// teacher account may ask for subjects while it is itself still unapproved, so without a ceiling one throw-away
/// account could queue a request for every subject and bury the genuine ones.
pub(crate) const MAX_OPEN_TEACHING_REQUESTS: i64 = 20;

/// A teacher asks to teach a subject; it stays `pending` until an admin approves.
fn request_teaching(conn: &Connection, teacher: &User, subject_id: &str) -> Res<()> {
    let active: Option<bool> = conn
        .query_row(
            "SELECT s.is_active AND i.is_active FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE s.id = ?1",
            params![subject_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if active != Some(true) {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT status FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2",
            params![teacher.id, subject_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if matches!(existing.as_deref(), Some("pending") | Some("approved")) {
        return Err(err(StatusCode::CONFLICT, "already_requested"));
    }
    let open: i64 = conn
        .query_row("SELECT count(*) FROM teacher_subjects WHERE teacher_id = ?1 AND status = 'pending'", params![teacher.id], |r| r.get(0))
        .map_err(db_err)?;
    if open >= MAX_OPEN_TEACHING_REQUESTS {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too_many_pending_requests"));
    }
    match existing.as_deref() {
        Some(_) => {
            conn.execute(
                "UPDATE teacher_subjects SET status = 'pending', created_at = ?3, decided_at = NULL WHERE teacher_id = ?1 AND subject_id = ?2",
                params![teacher.id, subject_id, now_ms()],
            )
            .map_err(db_err)?;
        }
        None => {
            conn.execute(
                "INSERT INTO teacher_subjects(teacher_id, subject_id, status, created_at) VALUES (?1, ?2, 'pending', ?3)",
                params![teacher.id, subject_id, now_ms()],
            )
            .map_err(db_err)?;
        }
    }
    Ok(())
}

pub async fn request_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SubjectRef>,
) -> Res<StatusCode> {
    let user = may_request_teaching(authenticate(&state, &headers)?)?;
    request_teaching(&*lock(&state)?, &user, &req.subject_id)?;
    Ok(StatusCode::CREATED)
}

/// Who may use the "ask to teach a subject" flow: a teacher whose account is active **or still waiting for approval**
/// (so the request can already be in the admin's queue when the account is approved). Rejected and suspended teachers
/// — and everyone who is not a teacher — are refused. The flow is exactly four routes, so a pending teacher can
/// actually complete it: `POST /teaching` (ask), `GET /teaching` (see the requests), `DELETE /teaching/{id}`
/// (withdraw one) and — to find a subject id at all — the catalogue reads, see [`may_browse_catalog`]. That is all a
/// pending account gets: the requested content stays invisible until the account is active *and* the assignment
/// approved (`visible()`), and every other teacher route still goes through `require_role`, which demands an active
/// account.
pub(crate) fn may_request_teaching(user: User) -> Res<User> {
    if user.role == "teacher" && (user.status == "active" || user.status == "pending") {
        Ok(user)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

/// Who may read the institution list and an institution's structure (`GET /institutions[/{id}/structure]`): any
/// active account, plus a teacher still waiting for approval — they need the catalogue to pick the subjects they
/// ask for ([`may_request_teaching`]). Pending students and rejected/suspended accounts of any role stay out.
pub(crate) fn may_browse_catalog(user: &User) -> bool {
    user.status == "active" || (user.role == "teacher" && user.status == "pending")
}

pub async fn drop_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(subject_id): Path<String>,
) -> Res<StatusCode> {
    let user = may_request_teaching(authenticate(&state, &headers)?)?;
    lock(&state)?
        .execute("DELETE FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2", params![user.id, subject_id])
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct TeachingFilter {
    status: Option<String>,
}

pub async fn admin_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(f): Query<TeachingFilter>,
) -> Res<Json<Vec<AdminTeaching>>> {
    require_admin(&state, &headers)?;
    admin_teaching(&*lock(&state)?, f.status.as_deref()).map(Json)
}

fn admin_teaching(conn: &Connection, status: Option<&str>) -> Res<Vec<AdminTeaching>> {
    // Requests from active accounts come first: those from accounts still awaiting approval can wait for the
    // account decision and must not push real requests off the 200-row page.
    let sql = format!(
        "{} WHERE (?1 IS NULL OR ts.status = ?1) ORDER BY (u.status = 'active') DESC, ts.created_at DESC LIMIT 200",
        teaching_select(", u.status")
    );
    conn.prepare(&sql)
        .map_err(db_err)?
        .query_map(params![status], |r| Ok(AdminTeaching { base: map_teaching(r)?, teacher_status: r.get(7)? }))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

#[derive(Deserialize)]
pub struct TeachingDecision {
    teacher_id: String,
    subject_id: String,
    status: String,
}

fn decide_teaching(conn: &Connection, admin: &User, d: &TeachingDecision) -> Res<()> {
    if !["approved", "rejected", "pending"].contains(&d.status.as_str()) {
        return Err(bad("invalid_status"));
    }
    let n = conn
        .execute(
            "UPDATE teacher_subjects SET status = ?3, decided_at = ?4 WHERE teacher_id = ?1 AND subject_id = ?2",
            params![d.teacher_id, d.subject_id, d.status, now_ms()],
        )
        .map_err(db_err)?;
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    audit(conn, &admin.id, &d.teacher_id, "teaching_decided", &format!("{} -> {}", d.subject_id, d.status));
    if d.status != "pending" {
        let subject: String = conn
            .query_row("SELECT name_ar FROM subjects WHERE id = ?1", params![d.subject_id], |r| r.get(0))
            .unwrap_or_default();
        crate::platform_engage::notify(
            conn,
            &d.teacher_id,
            &format!("teaching_{}", d.status),
            serde_json::json!({ "subject": subject }),
            &format!("/platform/subjects/{}", d.subject_id),
        );
    }
    Ok(())
}

pub async fn admin_decide_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(d): Json<TeachingDecision>,
) -> Res<StatusCode> {
    let admin = require_admin(&state, &headers)?;
    decide_teaching(&*lock(&state)?, &admin, &d)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── student placement / enrollment ─────────

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Placement {
    institution_id: String,
    unit_id: Option<String>,
}

fn set_placement(conn: &Connection, student: &User, p: &Placement) -> Res<()> {
    let ok: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM institutions WHERE id = ?1 AND is_active = 1)", params![p.institution_id], |r| r.get(0))
        .map_err(db_err)?;
    if !ok {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    if let Some(uid) = &p.unit_id {
        let same: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM org_units WHERE id = ?1 AND institution_id = ?2)",
                params![uid, p.institution_id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if !same {
            return Err(bad("unit_other_institution"));
        }
    }
    conn.execute(
        "INSERT INTO student_placement(student_id, institution_id, unit_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(student_id) DO UPDATE SET institution_id = excluded.institution_id, unit_id = excluded.unit_id",
        params![student.id, p.institution_id, p.unit_id],
    )
    .map_err(db_err)?;
    Ok(())
}

pub async fn get_placement_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Option<Placement>>> {
    let user = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    conn.query_row(
        "SELECT institution_id, unit_id FROM student_placement WHERE student_id = ?1",
        params![user.id],
        |r| Ok(Placement { institution_id: r.get(0)?, unit_id: r.get(1)? }),
    )
    .optional()
    .map_err(db_err)
    .map(Json)
}

pub async fn set_placement_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(p): Json<Placement>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    set_placement(&*lock(&state)?, &user, &p)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Debug)]
pub struct EnrolledSubject {
    subject_id: String,
    subject_name: String,
    institution_id: String,
    institution_name: String,
}

fn my_enrollments(conn: &Connection, student_id: &str) -> Res<Vec<EnrolledSubject>> {
    conn.prepare(
        "SELECT s.id, s.name_ar, i.id, i.name_ar FROM subject_enrollments e
         JOIN subjects s ON s.id = e.subject_id JOIN institutions i ON i.id = s.institution_id
         WHERE e.student_id = ?1 ORDER BY e.created_at DESC",
    )
    .map_err(db_err)?
    .query_map(params![student_id], |r| {
        Ok(EnrolledSubject { subject_id: r.get(0)?, subject_name: r.get(1)?, institution_id: r.get(2)?, institution_name: r.get(3)? })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

pub async fn my_enrollments_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Vec<EnrolledSubject>>> {
    let user = require_role(&state, &headers, "student")?;
    my_enrollments(&*lock(&state)?, &user.id).map(Json)
}

fn enroll(conn: &Connection, student: &User, subject_id: &str) -> Res<()> {
    let active: Option<bool> = conn
        .query_row(
            "SELECT s.is_active AND i.is_active FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE s.id = ?1",
            params![subject_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if active != Some(true) {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO subject_enrollments(student_id, subject_id, created_at) VALUES (?1, ?2, ?3)",
        params![student.id, subject_id, now_ms()],
    )
    .map_err(db_err)?;
    Ok(())
}

pub async fn enroll_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SubjectRef>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    enroll(&*lock(&state)?, &user, &req.subject_id)?;
    Ok(StatusCode::CREATED)
}

pub async fn unenroll_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(subject_id): Path<String>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    lock(&state)?
        .execute("DELETE FROM subject_enrollments WHERE student_id = ?1 AND subject_id = ?2", params![user.id, subject_id])
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── follows ─────────

fn follow(conn: &Connection, student: &User, teacher_id: &str) -> Res<()> {
    let is_teacher: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1 AND role = 'teacher' AND status = 'active')",
            params![teacher_id],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if !is_teacher {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO teacher_follows(student_id, teacher_id, created_at) VALUES (?1, ?2, ?3)",
        params![student.id, teacher_id, now_ms()],
    )
    .map_err(db_err)?;
    Ok(())
}

pub async fn follow_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    follow(&*lock(&state)?, &user, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unfollow_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "student")?;
    lock(&state)?
        .execute("DELETE FROM teacher_follows WHERE student_id = ?1 AND teacher_id = ?2", params![user.id, id])
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ───────── public pages ─────────

#[derive(Serialize, Debug)]
pub struct TeacherCard {
    id: String,
    full_name: String,
    bio: Option<String>,
    subject_count: i64,
}

#[derive(Deserialize)]
pub struct TeacherQuery {
    q: Option<String>,
    subject_id: Option<String>,
}

fn list_teachers(conn: &Connection, q: &TeacherQuery) -> Res<Vec<TeacherCard>> {
    let like = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_").to_lowercase())
    });
    conn.prepare(
        "SELECT u.id, u.full_name, u.bio,
                (SELECT count(*) FROM teacher_subjects t WHERE t.teacher_id = u.id AND t.status = 'approved')
         FROM users u
         WHERE u.role = 'teacher' AND u.status = 'active'
           AND (?1 IS NULL OR lower(u.full_name) LIKE ?1 ESCAPE '\\')
           AND (?2 IS NULL OR EXISTS(SELECT 1 FROM teacher_subjects t WHERE t.teacher_id = u.id AND t.subject_id = ?2 AND t.status = 'approved'))
         ORDER BY u.full_name LIMIT 100",
    )
    .map_err(db_err)?
    .query_map(params![like, q.subject_id], |r| {
        Ok(TeacherCard { id: r.get(0)?, full_name: r.get(1)?, bio: r.get(2)?, subject_count: r.get(3)? })
    })
    .map_err(db_err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(db_err)
}

pub async fn list_teachers_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TeacherQuery>,
) -> Res<Json<Vec<TeacherCard>>> {
    require_active(&state, &headers)?;
    list_teachers(&*lock(&state)?, &q).map(Json)
}

#[derive(Serialize)]
pub struct TeacherPage {
    id: String,
    full_name: String,
    bio: Option<String>,
    followers: i64,
    following: bool,
    subjects: Vec<Teaching>,
}

pub async fn teacher_page_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<Json<TeacherPage>> {
    let viewer = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    let (full_name, bio): (String, Option<String>) = conn
        .query_row(
            "SELECT full_name, bio FROM users WHERE id = ?1 AND role = 'teacher' AND status = 'active'",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    let followers: i64 = conn
        .query_row("SELECT count(*) FROM teacher_follows WHERE teacher_id = ?1", params![id], |r| r.get(0))
        .map_err(db_err)?;
    let following: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM teacher_follows WHERE teacher_id = ?1 AND student_id = ?2)",
            params![id, viewer.id],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    let subjects = my_teaching(&conn, &id)?.into_iter().filter(|t| t.status == "approved").collect();
    Ok(Json(TeacherPage { id, full_name, bio, followers, following, subjects }))
}

#[derive(Serialize)]
pub struct SubjectPage {
    id: String,
    name_ar: String,
    name_en: Option<String>,
    institution_id: String,
    institution_name: String,
    path: Vec<String>,
    enrolled: bool,
    enrolled_count: i64,
    teachers: Vec<TeacherCard>,
}

pub async fn subject_page_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<Json<SubjectPage>> {
    let viewer = require_active(&state, &headers)?;
    let conn = lock(&state)?;
    let (name_ar, name_en, institution_id, institution_name, unit_id, active): (String, Option<String>, String, String, Option<String>, bool) = conn
        .query_row(
            "SELECT s.name_ar, s.name_en, i.id, i.name_ar, s.unit_id, s.is_active AND i.is_active
             FROM subjects s JOIN institutions i ON i.id = s.institution_id WHERE s.id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;
    if !active && viewer.role != "admin" {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    // Names of the unit chain, root first (e.g. department > level > term).
    let mut path = Vec::new();
    let mut cur = unit_id;
    while let Some(uid) = cur {
        let (n, parent): (String, Option<String>) = conn
            .query_row("SELECT name_ar, parent_id FROM org_units WHERE id = ?1", params![uid], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db_err)?;
        path.push(n);
        cur = parent;
        if path.len() > 8 {
            break;
        }
    }
    path.reverse();
    let enrolled: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM subject_enrollments WHERE student_id = ?1 AND subject_id = ?2)", params![viewer.id, id], |r| r.get(0))
        .map_err(db_err)?;
    let enrolled_count: i64 = conn
        .query_row("SELECT count(*) FROM subject_enrollments WHERE subject_id = ?1", params![id], |r| r.get(0))
        .map_err(db_err)?;
    let teachers = list_teachers(&conn, &TeacherQuery { q: None, subject_id: Some(id.clone()) })?;
    Ok(Json(SubjectPage { id, name_ar, name_en, institution_id, institution_name, path, enrolled, enrolled_count, teachers }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    fn seed(conn: &Connection) -> (String, String) {
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','ج',1,0)", []).unwrap();
        conn.execute("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i2','school','م',1,0)", []).unwrap();
        conn.execute("INSERT INTO org_units(id,institution_id,kind,name_ar,created_at) VALUES ('u1','i1','level','L1',0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,unit_id,name_ar,is_active,created_at) VALUES ('s1','i1','u1','برمجة',1,0)", []).unwrap();
        conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES ('s2','i2','رياضيات',0,0)", []).unwrap();
        ("s1".into(), "s2".into())
    }

    #[test]
    fn teaching_requires_approval_and_flows() {
        let conn = create_test_db();
        let (s1, s2) = seed(&conn);
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        assert_eq!(request_teaching(&conn, &t, &s2).unwrap_err().0, StatusCode::NOT_FOUND, "inactive subject");
        assert_eq!(request_teaching(&conn, &t, "nope").unwrap_err().0, StatusCode::NOT_FOUND);
        request_teaching(&conn, &t, &s1).unwrap();
        assert_eq!(request_teaching(&conn, &t, &s1).unwrap_err().0, StatusCode::CONFLICT);
        // pending teachers do not show up for the subject
        let q = TeacherQuery { q: None, subject_id: Some(s1.clone()) };
        assert_eq!(list_teachers(&conn, &q).unwrap().len(), 0);
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: t.id.clone(), subject_id: s1.clone(), status: "approved".into() }).unwrap();
        assert_eq!(list_teachers(&conn, &q).unwrap().len(), 1);
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: t.id.clone(), subject_id: s1.clone(), status: "rejected".into() }).unwrap();
        assert_eq!(list_teachers(&conn, &q).unwrap().len(), 0);
        request_teaching(&conn, &t, &s1).unwrap(); // re-request after rejection
        assert_eq!(my_teaching(&conn, &t.id).unwrap()[0].status, "pending");
        let bad_status = TeachingDecision { teacher_id: t.id.clone(), subject_id: s1.clone(), status: "x".into() };
        assert_eq!(decide_teaching(&conn, &admin, &bad_status).unwrap_err().0, StatusCode::BAD_REQUEST);
        let missing = TeachingDecision { teacher_id: "no".into(), subject_id: s1, status: "approved".into() };
        assert_eq!(decide_teaching(&conn, &admin, &missing).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn placement_validates_unit_belongs_to_institution() {
        let conn = create_test_db();
        seed(&conn);
        let s = insert_test_user(&conn, "s@x.com", "student", "active");
        let p = |i: &str, u: Option<&str>| Placement { institution_id: i.into(), unit_id: u.map(Into::into) };
        set_placement(&conn, &s, &p("i1", Some("u1"))).unwrap();
        assert_eq!(set_placement(&conn, &s, &p("i2", Some("u1"))).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(set_placement(&conn, &s, &p("zzz", None)).unwrap_err().0, StatusCode::NOT_FOUND);
        set_placement(&conn, &s, &p("i2", None)).unwrap(); // update in place
        let n: i64 = conn.query_row("SELECT count(*) FROM student_placement", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn enroll_and_follow_rules() {
        let conn = create_test_db();
        let (s1, s2) = seed(&conn);
        let stu = insert_test_user(&conn, "s@x.com", "student", "active");
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let pending_t = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        enroll(&conn, &stu, &s1).unwrap();
        enroll(&conn, &stu, &s1).unwrap(); // idempotent
        assert_eq!(enroll(&conn, &stu, &s2).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(my_enrollments(&conn, &stu.id).unwrap().len(), 1);
        follow(&conn, &stu, &t.id).unwrap();
        assert_eq!(follow(&conn, &stu, &pending_t.id).unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(follow(&conn, &stu, &stu.id).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn profile_update_validates() {
        let conn = create_test_db();
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        update_profile(&conn, &t, &ProfilePatch { full_name: Some(" د. أحمد ".into()), bio: Some("خبير".into()), ..Default::default() }).unwrap();
        let card = &list_teachers(&conn, &TeacherQuery { q: Some("أحمد".into()), subject_id: None }).unwrap()[0];
        assert_eq!((card.full_name.as_str(), card.bio.as_deref()), ("د. أحمد", Some("خبير")));
        assert_eq!(update_profile(&conn, &t, &ProfilePatch { full_name: Some("  ".into()), ..Default::default() }).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_profile(&conn, &t, &ProfilePatch { bio: Some("x".repeat(1001)), ..Default::default() }).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    // ───── phase 3-1: kind of institution in the profile ─────

    fn inst_of(conn: &Connection, u: &User) -> Option<String> {
        conn.query_row("SELECT institution_type FROM users WHERE id = ?1", params![u.id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn the_profile_accepts_only_known_institution_types_and_only_changes_the_callers_row() {
        let conn = create_test_db();
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let bystander = insert_test_user(&conn, "b@x.com", "teacher", "active");
        let set = |v: &str| ProfilePatch { institution_type: Some(v.into()), ..Default::default() };
        for ok in ["school", "institute", "university"] {
            update_profile(&conn, &t, &set(ok)).unwrap();
            assert_eq!(inst_of(&conn, &t).as_deref(), Some(ok));
        }
        for bad_value in ["college", "", " school", "SCHOOL", "university ", "<script>"] {
            let e = update_profile(&conn, &t, &set(bad_value)).unwrap_err();
            assert_eq!((e.0, e.1.contains("invalid_type")), (StatusCode::BAD_REQUEST, true), "{bad_value:?}");
        }
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("university"), "refused values change nothing");
        assert_eq!(inst_of(&conn, &bystander), None, "another user's row is never touched");
        // omitted / null leaves it alone
        update_profile(&conn, &t, &ProfilePatch { bio: Some("نبذة".into()), ..Default::default() }).unwrap();
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("university"));
        let from_json: ProfilePatch = serde_json::from_value(serde_json::json!({"institution_type": null})).unwrap();
        update_profile(&conn, &t, &from_json).unwrap();
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("university"));
        let from_json: ProfilePatch = serde_json::from_value(serde_json::json!({"institution_type": "school"})).unwrap();
        update_profile(&conn, &t, &from_json).unwrap();
        assert_eq!(inst_of(&conn, &t).as_deref(), Some("school"));
    }

    #[test]
    fn a_bad_field_leaves_the_whole_profile_patch_unapplied() {
        let conn = create_test_db();
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        update_profile(&conn, &t, &ProfilePatch { bio: Some("قديمة".into()), institution_type: Some("school".into()), ..Default::default() }).unwrap();
        let name = |c: &Connection| -> String { c.query_row("SELECT full_name FROM users WHERE id = ?1", params![t.id], |r| r.get(0)).unwrap() };
        let bio = |c: &Connection| -> Option<String> { c.query_row("SELECT bio FROM users WHERE id = ?1", params![t.id], |r| r.get(0)).unwrap() };
        for patch in [
            ProfilePatch { full_name: Some("اسم جديد".into()), institution_type: Some("bogus".into()), ..Default::default() },
            ProfilePatch { full_name: Some("اسم جديد".into()), bio: Some("x".repeat(1001)), ..Default::default() },
            ProfilePatch { bio: Some("جديدة".into()), institution_type: Some("bogus".into()), ..Default::default() },
        ] {
            assert!(update_profile(&conn, &t, &patch).is_err());
            assert_eq!((name(&conn).as_str(), bio(&conn).as_deref(), inst_of(&conn, &t).as_deref()), ("T", Some("قديمة"), Some("school")), "nothing half-applied");
        }
    }

    // ───── phase 3-1: a teacher waiting for approval can already ask for subjects ─────

    fn reload(conn: &Connection, u: &User) -> User {
        let status: String = conn.query_row("SELECT status FROM users WHERE id = ?1", params![u.id], |r| r.get(0)).unwrap();
        User { status, ..u.clone() }
    }

    fn visible_posts(conn: &Connection) -> i64 {
        conn.query_row(
            &format!(
                "SELECT count(*) FROM posts p JOIN users u ON u.id = p.teacher_id JOIN subjects s ON s.id = p.subject_id WHERE {}",
                crate::platform_content::visible("p", "published")
            ),
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn only_active_and_pending_teachers_may_request_a_subject() {
        let conn = create_test_db();
        let who = |role: &str, status: &str| insert_test_user(&conn, &format!("{role}-{status}@x.com"), role, status);
        for (role, status, allowed) in [
            ("teacher", "active", true),
            ("teacher", "pending", true),
            ("teacher", "rejected", false),
            ("teacher", "suspended", false),
            ("student", "active", false),
            ("student", "pending", false),
            ("admin", "active", false),
        ] {
            let r = may_request_teaching(who(role, status));
            assert_eq!(r.is_ok(), allowed, "{role}/{status}");
            if !allowed {
                assert_eq!(r.unwrap_err().0, StatusCode::FORBIDDEN);
            }
        }
    }

    #[test]
    fn a_pending_teacher_can_browse_the_catalogue_and_manage_requests_and_nobody_else_gains_anything() {
        let conn = create_test_db();
        let who = |role: &str, status: &str| insert_test_user(&conn, &format!("{role}-{status}@x.com"), role, status);
        // (role, status, may browse institutions/structure, may ask for / list / withdraw subject requests)
        let matrix = [
            ("admin", "active", true, false),
            ("student", "active", true, false),
            ("teacher", "active", true, true),
            ("teacher", "pending", true, true), // the point of the phase: a pending teacher can find a subject and ask for it
            ("student", "pending", false, false),
            ("teacher", "rejected", false, false),
            ("teacher", "suspended", false, false),
            ("student", "suspended", false, false),
        ];
        for (role, status, browse, teach) in matrix {
            let u = who(role, status);
            assert_eq!(may_browse_catalog(&u), browse, "browse {role}/{status}");
            assert_eq!(may_request_teaching(u).is_ok(), teach, "teaching flow {role}/{status}");
        }
        // everything else a pending teacher might try still needs an ACTIVE account
        let pending = insert_test_user(&conn, "the-pending-one@x.com", "teacher", "pending");
        for denied in [
            crate::platform::ensure_role(pending.clone(), "teacher").map(|_| ()),
            crate::platform::ensure_active(pending.clone()).map(|_| ()),
            crate::platform::ensure_role(pending.clone(), "admin").map(|_| ()),
        ] {
            assert_eq!(denied.unwrap_err().0, StatusCode::FORBIDDEN);
        }
        // and the flow itself works end to end on the data layer: ask, see it, withdraw it
        let (s1, _) = seed(&conn);
        request_teaching(&conn, &pending, &s1).unwrap();
        assert_eq!(my_teaching(&conn, &pending.id).unwrap().len(), 1);
        conn.execute("DELETE FROM teacher_subjects WHERE teacher_id = ?1 AND subject_id = ?2", params![pending.id, s1]).unwrap();
        assert!(my_teaching(&conn, &pending.id).unwrap().is_empty());
    }

    #[test]
    fn a_pending_teacher_can_request_a_subject_but_gets_nothing_else_until_approved() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let student = insert_test_user(&conn, "s@x.com", "student", "active");
        enroll(&conn, &student, &s1).unwrap();

        // the request goes through and lands in the admin's queue
        let me = may_request_teaching(pending.clone()).unwrap();
        request_teaching(&conn, &me, &s1).unwrap();
        assert_eq!(request_teaching(&conn, &me, &s1).unwrap_err().0, StatusCode::CONFLICT, "still one request per subject");

        // …but everything else a teacher does keeps demanding an active account (the handlers' `require_role` guard)
        assert_eq!(crate::platform::ensure_role(pending.clone(), "teacher").unwrap_err().0, StatusCode::FORBIDDEN, "create posts/courses/live/exams, read my teaching, …");
        assert_eq!(crate::platform::ensure_active(pending.clone()).unwrap_err().0, StatusCode::FORBIDDEN);

        // approving the ASSIGNMENT does not activate the ACCOUNT
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: pending.id.clone(), subject_id: s1.clone(), status: "approved".into() }).unwrap();
        let after = reload(&conn, &pending);
        assert_eq!(after.status, "pending", "the account is still waiting");
        assert_eq!(crate::platform::ensure_role(after, "teacher").unwrap_err().0, StatusCode::FORBIDDEN);

        // content of a pending teacher stays invisible even with an approved assignment: posts and exams alike
        conn.execute("INSERT INTO posts VALUES ('p1', ?1, 's1', 'article', 'منشور', 'نص', 'published', NULL, 0, 0)", params![pending.id]).unwrap();
        conn.execute(
            "INSERT INTO assessments(id,teacher_id,subject_id,title,questions,question_count,total_points,status,created_at,updated_at) VALUES ('e1',?1,'s1','امتحان','[]',1,1,'published',0,0)",
            params![pending.id],
        )
        .unwrap();
        let available = |c: &Connection| crate::platform_exams::list_available(c, &student.id).unwrap().len();
        assert_eq!((visible_posts(&conn), available(&conn)), (0, 0), "account pending, assignment approved: invisible");

        // active account but assignment not (or no longer) approved: invisible
        conn.execute("UPDATE users SET status = 'active' WHERE id = ?1", params![pending.id]).unwrap();
        assert_eq!((visible_posts(&conn), available(&conn)), (1, 1), "both conditions hold: visible");
        for assignment in ["pending", "rejected"] {
            conn.execute("UPDATE teacher_subjects SET status = ?1 WHERE teacher_id = ?2", params![assignment, pending.id]).unwrap();
            assert_eq!((visible_posts(&conn), available(&conn)), (0, 0), "assignment {assignment}: invisible");
        }
        conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1", params![pending.id]).unwrap();
        conn.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", params![pending.id]).unwrap();
        assert_eq!((visible_posts(&conn), available(&conn)), (0, 0), "suspended account: invisible again");
    }

    #[test]
    fn the_admin_queue_shows_the_state_of_the_teachers_account_next_to_each_request() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let active = insert_test_user(&conn, "t@x.com", "teacher", "active");
        request_teaching(&conn, &pending, &s1).unwrap();
        request_teaching(&conn, &active, &s1).unwrap();
        let rows = admin_teaching(&conn, None).unwrap();
        assert_eq!(rows.len(), 2);
        let status_of = |id: &str| rows.iter().find(|r| r.base.teacher_id == id).map(|r| r.teacher_status.clone()).unwrap();
        assert_eq!((status_of(&pending.id).as_str(), status_of(&active.id).as_str()), ("pending", "active"));
        // the JSON keeps every field the page already used and adds teacher_status
        let json = serde_json::to_value(rows.iter().find(|r| r.base.teacher_id == pending.id).unwrap()).unwrap();
        for key in ["teacher_id", "teacher_name", "subject_id", "subject_name", "institution_id", "institution_name", "status", "teacher_status"] {
            assert!(json.get(key).is_some(), "{key} in {json}");
        }
        assert_eq!((json["status"].as_str(), json["teacher_status"].as_str()), (Some("pending"), Some("pending")), "assignment status and account status are different fields");
        // the status filter still filters assignments
        conn.execute("UPDATE teacher_subjects SET status = 'approved' WHERE teacher_id = ?1", params![active.id]).unwrap();
        assert_eq!(admin_teaching(&conn, Some("approved")).unwrap().len(), 1);
        assert_eq!(admin_teaching(&conn, Some("pending")).unwrap().len(), 1);
        // an account the admin later rejects keeps showing its (rejected) account state
        conn.execute("UPDATE users SET status = 'rejected' WHERE id = ?1", params![pending.id]).unwrap();
        assert_eq!(admin_teaching(&conn, Some("pending")).unwrap()[0].teacher_status, "rejected");
    }

    fn extra_subjects(conn: &Connection, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| {
                let id = format!("x{i}");
                conn.execute("INSERT INTO subjects(id,institution_id,name_ar,is_active,created_at) VALUES (?1,'i1','مادة',1,0)", params![id]).unwrap();
                id
            })
            .collect()
    }

    #[test]
    fn a_teacher_can_only_have_a_limited_number_of_requests_waiting() {
        let conn = create_test_db();
        seed(&conn);
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let other = insert_test_user(&conn, "o@x.com", "teacher", "active");
        let subjects = extra_subjects(&conn, MAX_OPEN_TEACHING_REQUESTS as usize + 2);
        let (first_batch, rest) = subjects.split_at(MAX_OPEN_TEACHING_REQUESTS as usize);
        for s in first_batch {
            request_teaching(&conn, &pending, s).unwrap();
        }
        let waiting = |c: &Connection, t: &User| -> i64 { c.query_row("SELECT count(*) FROM teacher_subjects WHERE teacher_id = ?1 AND status = 'pending'", params![t.id], |r| r.get(0)).unwrap() };
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS);

        // the next one is refused with 429 and nothing is written
        let e = request_teaching(&conn, &pending, &rest[0]).unwrap_err();
        assert_eq!((e.0, e.1.contains("too_many_pending_requests")), (StatusCode::TOO_MANY_REQUESTS, true));
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS);
        // asking again for one that is already waiting is still reported as that, not as the cap
        assert_eq!(request_teaching(&conn, &pending, &first_batch[0]).unwrap_err().0, StatusCode::CONFLICT);
        // the cap belongs to the teacher, not to the platform
        request_teaching(&conn, &other, &rest[0]).unwrap();

        // a rejected request that is retried would be a new waiting one, so it is capped too
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: pending.id.clone(), subject_id: first_batch[0].clone(), status: "rejected".into() }).unwrap();
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS - 1);
        request_teaching(&conn, &pending, &rest[0]).unwrap(); // a slot was freed by the decision
        assert_eq!(request_teaching(&conn, &pending, &first_batch[0]).unwrap_err().0, StatusCode::TOO_MANY_REQUESTS, "retrying the rejected one needs a free slot");
        // approving frees a slot as well (an approved assignment is not "waiting")
        decide_teaching(&conn, &admin, &TeachingDecision { teacher_id: pending.id.clone(), subject_id: first_batch[1].clone(), status: "approved".into() }).unwrap();
        request_teaching(&conn, &pending, &first_batch[0]).unwrap();
        assert_eq!(waiting(&conn, &pending), MAX_OPEN_TEACHING_REQUESTS);
    }

    #[test]
    fn the_admin_queue_lists_requests_of_active_accounts_before_those_of_unapproved_ones() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let others = extra_subjects(&conn, 1);
        let pending = insert_test_user(&conn, "p@x.com", "teacher", "pending");
        let active = insert_test_user(&conn, "t@x.com", "teacher", "active");
        request_teaching(&conn, &active, &s1).unwrap();
        request_teaching(&conn, &pending, &s1).unwrap();
        request_teaching(&conn, &pending, &others[0]).unwrap();
        // the unapproved account's requests are the newest ones…
        conn.execute("UPDATE teacher_subjects SET created_at = 1 WHERE teacher_id = ?1", params![active.id]).unwrap();
        conn.execute("UPDATE teacher_subjects SET created_at = 9000000000000 WHERE teacher_id = ?1", params![pending.id]).unwrap();
        let rows = admin_teaching(&conn, Some("pending")).unwrap();
        let order: Vec<&str> = rows.iter().map(|r| r.teacher_status.as_str()).collect();
        // …and still come last, so they can never push a real teacher's request off the page
        assert_eq!(order, ["active", "pending", "pending"]);
    }

    #[test]
    fn deleting_a_subject_cascades_assignments() {
        let conn = create_test_db();
        let (s1, _) = seed(&conn);
        let t = insert_test_user(&conn, "t@x.com", "teacher", "active");
        let stu = insert_test_user(&conn, "s@x.com", "student", "active");
        request_teaching(&conn, &t, &s1).unwrap();
        enroll(&conn, &stu, &s1).unwrap();
        conn.execute("DELETE FROM institutions WHERE id = 'i1'", []).unwrap();
        let n: i64 = conn.query_row("SELECT (SELECT count(*) FROM teacher_subjects)+(SELECT count(*) FROM subject_enrollments)", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }
}
