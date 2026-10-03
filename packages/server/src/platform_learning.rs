//! Teaching assignments (teacher <-> subject, admin-approved), student placement and
//! enrollment, following teachers, public teacher/subject pages.

use crate::platform::{audit, bad, db_err, lock, opt_text, require_active, require_admin, require_role, Res, User};
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

#[derive(Deserialize)]
pub struct ProfilePatch {
    full_name: Option<String>,
    bio: Option<String>,
}

fn update_profile(conn: &Connection, user: &User, p: &ProfilePatch) -> Res<()> {
    if let Some(n) = &p.full_name {
        let t = n.trim();
        if t.is_empty() || t.chars().count() > 120 {
            return Err(bad("invalid_name"));
        }
        conn.execute("UPDATE users SET full_name = ?1 WHERE id = ?2", params![t, user.id]).map_err(db_err)?;
    }
    if p.bio.is_some() {
        let bio = opt_text(&p.bio, 1000, "bio_too_long")?;
        conn.execute("UPDATE users SET bio = ?1 WHERE id = ?2", params![bio, user.id]).map_err(db_err)?;
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

const TEACHING_SELECT: &str = "SELECT ts.teacher_id, u.full_name, ts.subject_id, s.name_ar, i.id, i.name_ar, ts.status
     FROM teacher_subjects ts
     JOIN users u ON u.id = ts.teacher_id
     JOIN subjects s ON s.id = ts.subject_id
     JOIN institutions i ON i.id = s.institution_id";

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
    conn.prepare(&format!("{TEACHING_SELECT} WHERE ts.teacher_id = ?1 ORDER BY i.name_ar, s.name_ar"))
        .map_err(db_err)?
        .query_map(params![teacher_id], map_teaching)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)
}

pub async fn my_teaching_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Res<Json<Vec<Teaching>>> {
    let user = require_role(&state, &headers, "teacher")?;
    my_teaching(&*lock(&state)?, &user.id).map(Json)
}

#[derive(Deserialize)]
pub struct SubjectRef {
    subject_id: String,
}

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
    match existing.as_deref() {
        Some("pending") | Some("approved") => return Err(err(StatusCode::CONFLICT, "already_requested")),
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
    let user = require_role(&state, &headers, "teacher")?;
    request_teaching(&*lock(&state)?, &user, &req.subject_id)?;
    Ok(StatusCode::CREATED)
}

pub async fn drop_teaching_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(subject_id): Path<String>,
) -> Res<StatusCode> {
    let user = require_role(&state, &headers, "teacher")?;
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
) -> Res<Json<Vec<Teaching>>> {
    require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    let rows = conn
        .prepare(&format!("{TEACHING_SELECT} WHERE (?1 IS NULL OR ts.status = ?1) ORDER BY ts.created_at DESC LIMIT 200"))
        .map_err(db_err)?
        .query_map(params![f.status], map_teaching)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(Json(rows))
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
        update_profile(&conn, &t, &ProfilePatch { full_name: Some(" د. أحمد ".into()), bio: Some("خبير".into()) }).unwrap();
        let card = &list_teachers(&conn, &TeacherQuery { q: Some("أحمد".into()), subject_id: None }).unwrap()[0];
        assert_eq!((card.full_name.as_str(), card.bio.as_deref()), ("د. أحمد", Some("خبير")));
        assert_eq!(update_profile(&conn, &t, &ProfilePatch { full_name: Some("  ".into()), bio: None }).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_profile(&conn, &t, &ProfilePatch { full_name: None, bio: Some("x".repeat(1001)) }).unwrap_err().0, StatusCode::BAD_REQUEST);
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
