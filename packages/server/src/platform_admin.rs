//! Platform administration: user approval/editing and the institutions tree.
//! Every handler authenticates server-side; admin routes require an active `admin`.

use crate::platform::{audit, authenticate, bad, db_err, hash_password, lock, new_id, opt_text, require_admin, row_to_user, text, Res, User, USER_COLS};
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

const ROLES: [&str; 5] = ["admin", "institution_admin", "moderator", "teacher", "student"];
const STATUSES: [&str; 4] = ["active", "pending", "rejected", "suspended"];
const TYPES: [&str; 3] = ["school", "institute", "university"];
const MAX_USERS_LISTED: i64 = 200;

// ───────── users ─────────

#[derive(Deserialize)]
pub struct UserFilter {
    status: Option<String>,
    role: Option<String>,
    q: Option<String>,
}

fn list_users(conn: &Connection, f: &UserFilter) -> Res<Vec<User>> {
    let q = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_").to_lowercase())
    });
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {USER_COLS} FROM users
             WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR role = ?2)
               AND (?3 IS NULL OR lower(email) LIKE ?3 ESCAPE '\\' OR lower(full_name) LIKE ?3 ESCAPE '\\')
             ORDER BY created_at DESC LIMIT {MAX_USERS_LISTED}"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![f.status, f.role, q], row_to_user)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

pub async fn list_users_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(f): Query<UserFilter>,
) -> Res<Json<Vec<User>>> {
    require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    list_users(&conn, &f).map(Json)
}

#[derive(Deserialize)]
pub struct UserPatch {
    role: Option<String>,
    status: Option<String>,
    status_reason: Option<String>,
    full_name: Option<String>,
    password: Option<String>,
}

/// Applies an admin edit. `password_hash` is the already-hashed `patch.password`, if any.
fn update_user(conn: &Connection, actor: &User, id: &str, patch: &UserPatch, password_hash: Option<&str>) -> Res<User> {
    let target = conn
        .query_row(&format!("SELECT {USER_COLS} FROM users WHERE id = ?1"), params![id], row_to_user)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))?;

    let role = patch.role.clone().unwrap_or_else(|| target.role.clone());
    let status = patch.status.clone().unwrap_or_else(|| target.status.clone());
    if !ROLES.contains(&role.as_str()) {
        return Err(bad("invalid_role"));
    }
    if !STATUSES.contains(&status.as_str()) {
        return Err(bad("invalid_status"));
    }
    // An admin cannot lock themselves out or change their own privileges.
    if target.id == actor.id && (role != target.role || status != target.status) {
        return Err(bad("cannot_modify_self"));
    }
    let full_name = match &patch.full_name {
        Some(n) => {
            let t = n.trim();
            if t.is_empty() || t.chars().count() > 120 {
                return Err(bad("invalid_name"));
            }
            t.to_string()
        }
        None => target.full_name.clone(),
    };
    let reason = if status == "rejected" || status == "suspended" {
        let r = patch.status_reason.clone().or(target.status_reason.clone()).unwrap_or_default();
        let r = r.trim().to_string();
        if r.chars().count() > 500 {
            return Err(bad("reason_too_long"));
        }
        (!r.is_empty()).then_some(r)
    } else {
        None
    };

    conn.execute(
        "UPDATE users SET role = ?1, status = ?2, status_reason = ?3, full_name = ?4 WHERE id = ?5",
        params![role, status, reason, full_name, id],
    )
    .map_err(db_err)?;
    if let Some(h) = password_hash {
        conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![h, id]).map_err(db_err)?;
    }
    // Force re-login whenever access or credentials change.
    if status != "active" || role != target.role || password_hash.is_some() {
        conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![id]).map_err(db_err)?;
    }
    if role != target.role || status != target.status {
        audit(
            conn,
            &actor.id,
            id,
            "user_role_status_changed",
            &format!("{}/{} -> {}/{}", target.role, target.status, role, status),
        );
    }
    if password_hash.is_some() {
        audit(conn, &actor.id, id, "user_password_reset", "");
    }
    if status != target.status && ["active", "rejected", "suspended"].contains(&status.as_str()) {
        crate::platform_engage::notify(conn, id, &format!("account_{status}"), serde_json::json!({ "reason": reason }), "/platform");
    }
    conn.query_row(&format!("SELECT {USER_COLS} FROM users WHERE id = ?1"), params![id], row_to_user)
        .map_err(db_err)
}

pub async fn update_user_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(patch): Json<UserPatch>,
) -> Res<Json<User>> {
    let actor = require_admin(&state, &headers)?;
    let hash = match &patch.password {
        Some(p) => {
            if p.chars().count() < 8 || p.chars().count() > 128 {
                return Err(bad("invalid_password"));
            }
            let p = p.clone();
            Some(
                tokio::task::spawn_blocking(move || hash_password(&p))
                    .await
                    .map_err(db_err)??,
            )
        }
        None => None,
    };
    let conn = lock(&state)?;
    update_user(&conn, &actor, &id, &patch, hash.as_deref()).map(Json)
}

// ───────── institutions tree ─────────

#[derive(Serialize, Debug)]
pub struct Institution {
    id: String,
    #[serde(rename = "type")]
    kind_type: String,
    name_ar: String,
    name_en: Option<String>,
    city: Option<String>,
    is_active: bool,
}

#[derive(Serialize, Debug)]
pub struct Unit {
    id: String,
    institution_id: String,
    parent_id: Option<String>,
    kind: String,
    name_ar: String,
    name_en: Option<String>,
    sort_order: i64,
    is_active: bool,
}

#[derive(Serialize, Debug)]
pub struct Subject {
    id: String,
    institution_id: String,
    unit_id: Option<String>,
    name_ar: String,
    name_en: Option<String>,
    is_active: bool,
}

fn row_institution(r: &rusqlite::Row) -> rusqlite::Result<Institution> {
    Ok(Institution {
        id: r.get(0)?,
        kind_type: r.get(1)?,
        name_ar: r.get(2)?,
        name_en: r.get(3)?,
        city: r.get(4)?,
        is_active: r.get(5)?,
    })
}

const INST_COLS: &str = "id, type, name_ar, name_en, city, is_active";

fn get_institution(conn: &Connection, id: &str) -> Res<Institution> {
    conn.query_row(&format!("SELECT {INST_COLS} FROM institutions WHERE id = ?1"), params![id], row_institution)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

#[derive(Deserialize)]
pub struct TypeFilter {
    #[serde(rename = "type")]
    kind_type: Option<String>,
}

/// Any active signed-in user may browse institutions; admins also see inactive ones.
pub async fn list_institutions_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(f): Query<TypeFilter>,
) -> Res<Json<Vec<Institution>>> {
    let user = authenticate(&state, &headers)?;
    if user.status != "active" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let all = user.role == "admin";
    let conn = lock(&state)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {INST_COLS} FROM institutions WHERE (?1 IS NULL OR type = ?1) AND (?2 OR is_active = 1) ORDER BY name_ar"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![f.kind_type, all], row_institution)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(Json(rows))
}

#[derive(Serialize, Debug)]
pub struct Structure {
    institution: Institution,
    units: Vec<Unit>,
    subjects: Vec<Subject>,
}

pub async fn structure_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<Json<Structure>> {
    let user = authenticate(&state, &headers)?;
    if user.status != "active" {
        return Err(err(StatusCode::FORBIDDEN, "forbidden"));
    }
    let all = user.role == "admin";
    let conn = lock(&state)?;
    let institution = get_institution(&conn, &id)?;
    if !institution.is_active && !all {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    let units = conn
        .prepare(
            "SELECT id, institution_id, parent_id, kind, name_ar, name_en, sort_order, is_active FROM org_units
             WHERE institution_id = ?1 AND (?2 OR is_active = 1) ORDER BY sort_order, created_at",
        )
        .map_err(db_err)?
        .query_map(params![id, all], |r| {
            Ok(Unit {
                id: r.get(0)?,
                institution_id: r.get(1)?,
                parent_id: r.get(2)?,
                kind: r.get(3)?,
                name_ar: r.get(4)?,
                name_en: r.get(5)?,
                sort_order: r.get(6)?,
                is_active: r.get(7)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let subjects = conn
        .prepare(
            "SELECT id, institution_id, unit_id, name_ar, name_en, is_active FROM subjects
             WHERE institution_id = ?1 AND (?2 OR is_active = 1) ORDER BY name_ar",
        )
        .map_err(db_err)?
        .query_map(params![id, all], |r| {
            Ok(Subject {
                id: r.get(0)?,
                institution_id: r.get(1)?,
                unit_id: r.get(2)?,
                name_ar: r.get(3)?,
                name_en: r.get(4)?,
                is_active: r.get(5)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(Json(Structure { institution, units, subjects }))
}

#[derive(Deserialize)]
pub struct InstitutionReq {
    #[serde(rename = "type")]
    kind_type: Option<String>,
    name_ar: Option<String>,
    name_en: Option<String>,
    city: Option<String>,
    is_active: Option<bool>,
}

fn create_institution(conn: &Connection, actor: &User, req: &InstitutionReq) -> Res<Institution> {
    let t = req.kind_type.as_deref().unwrap_or("");
    if !TYPES.contains(&t) {
        return Err(bad("invalid_type"));
    }
    let name_ar = text(req.name_ar.as_deref().unwrap_or(""), 200, "invalid_name")?;
    let name_en = opt_text(&req.name_en, 200, "name_too_long")?;
    let city = opt_text(&req.city, 200, "name_too_long")?;
    let id = new_id();
    conn.execute(
        "INSERT INTO institutions(id, type, name_ar, name_en, city, is_active, created_at) VALUES (?1,?2,?3,?4,?5,1,?6)",
        params![id, t, name_ar, name_en, city, now_ms()],
    )
    .map_err(db_err)?;
    audit(conn, &actor.id, &id, "institution_created", &name_ar);
    get_institution(conn, &id)
}

fn update_institution(conn: &Connection, actor: &User, id: &str, req: &InstitutionReq) -> Res<Institution> {
    let cur = get_institution(conn, id)?;
    let name_ar = match &req.name_ar {
        Some(n) => text(n, 200, "invalid_name")?,
        None => cur.name_ar,
    };
    let name_en = if req.name_en.is_some() { opt_text(&req.name_en, 200, "name_too_long")? } else { cur.name_en };
    let city = if req.city.is_some() { opt_text(&req.city, 200, "name_too_long")? } else { cur.city };
    let active = req.is_active.unwrap_or(cur.is_active);
    conn.execute(
        "UPDATE institutions SET name_ar=?1, name_en=?2, city=?3, is_active=?4 WHERE id=?5",
        params![name_ar, name_en, city, active, id],
    )
    .map_err(db_err)?;
    audit(conn, &actor.id, id, "institution_updated", &name_ar);
    get_institution(conn, id)
}

pub async fn create_institution_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<InstitutionReq>,
) -> Res<(StatusCode, Json<Institution>)> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    create_institution(&conn, &actor, &req).map(|i| (StatusCode::CREATED, Json(i)))
}

pub async fn update_institution_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<InstitutionReq>,
) -> Res<Json<Institution>> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    update_institution(&conn, &actor, &id, &req).map(Json)
}

fn delete_row(conn: &Connection, actor: &User, table: &str, action: &str, id: &str) -> Res<StatusCode> {
    // `table` is always a literal from this module, never user input.
    let n = conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), params![id]).map_err(db_err)?;
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "not_found"));
    }
    audit(conn, &actor.id, id, action, "");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_institution_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<StatusCode> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    delete_row(&conn, &actor, "institutions", "institution_deleted", &id)
}

// ───────── units (department > level > year > term) ─────────

fn kind_rank(kind: &str) -> Option<u8> {
    match kind {
        "department" => Some(0),
        "level" => Some(1),
        "year" => Some(2),
        "term" => Some(3),
        _ => None,
    }
}

#[derive(Deserialize)]
pub struct UnitReq {
    institution_id: Option<String>,
    parent_id: Option<String>,
    kind: Option<String>,
    name_ar: Option<String>,
    name_en: Option<String>,
    sort_order: Option<i64>,
    is_active: Option<bool>,
}

fn get_unit(conn: &Connection, id: &str) -> Res<Unit> {
    conn.query_row(
        "SELECT id, institution_id, parent_id, kind, name_ar, name_en, sort_order, is_active FROM org_units WHERE id = ?1",
        params![id],
        |r| {
            Ok(Unit {
                id: r.get(0)?,
                institution_id: r.get(1)?,
                parent_id: r.get(2)?,
                kind: r.get(3)?,
                name_ar: r.get(4)?,
                name_en: r.get(5)?,
                sort_order: r.get(6)?,
                is_active: r.get(7)?,
            })
        },
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn create_unit(conn: &Connection, actor: &User, req: &UnitReq) -> Res<Unit> {
    let inst = get_institution(conn, req.institution_id.as_deref().unwrap_or(""))?;
    let kind = req.kind.as_deref().unwrap_or("");
    let rank = kind_rank(kind).ok_or_else(|| bad("invalid_kind"))?;
    // Schools have no departments; levels/years/terms only.
    if inst.kind_type == "school" && kind == "department" {
        return Err(bad("invalid_kind_for_school"));
    }
    if let Some(pid) = req.parent_id.as_deref() {
        let parent = get_unit(conn, pid)?;
        if parent.institution_id != inst.id {
            return Err(bad("parent_other_institution"));
        }
        if kind_rank(&parent.kind).unwrap_or(u8::MAX) >= rank {
            return Err(bad("invalid_kind_under_parent"));
        }
    }
    let name_ar = text(req.name_ar.as_deref().unwrap_or(""), 200, "invalid_name")?;
    let name_en = opt_text(&req.name_en, 200, "name_too_long")?;
    let id = new_id();
    conn.execute(
        "INSERT INTO org_units(id, institution_id, parent_id, kind, name_ar, name_en, sort_order, is_active, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8)",
        params![id, inst.id, req.parent_id, kind, name_ar, name_en, req.sort_order.unwrap_or(0), now_ms()],
    )
    .map_err(db_err)?;
    audit(conn, &actor.id, &id, "unit_created", &format!("{kind}: {name_ar}"));
    get_unit(conn, &id)
}

fn update_unit(conn: &Connection, actor: &User, id: &str, req: &UnitReq) -> Res<Unit> {
    let cur = get_unit(conn, id)?;
    let name_ar = match &req.name_ar {
        Some(n) => text(n, 200, "invalid_name")?,
        None => cur.name_ar,
    };
    let name_en = if req.name_en.is_some() { opt_text(&req.name_en, 200, "name_too_long")? } else { cur.name_en };
    conn.execute(
        "UPDATE org_units SET name_ar=?1, name_en=?2, sort_order=?3, is_active=?4 WHERE id=?5",
        params![name_ar, name_en, req.sort_order.unwrap_or(cur.sort_order), req.is_active.unwrap_or(cur.is_active), id],
    )
    .map_err(db_err)?;
    audit(conn, &actor.id, id, "unit_updated", &name_ar);
    get_unit(conn, id)
}

pub async fn create_unit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<UnitReq>,
) -> Res<(StatusCode, Json<Unit>)> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    create_unit(&conn, &actor, &req).map(|u| (StatusCode::CREATED, Json(u)))
}

pub async fn update_unit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UnitReq>,
) -> Res<Json<Unit>> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    update_unit(&conn, &actor, &id, &req).map(Json)
}

pub async fn delete_unit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<StatusCode> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    delete_row(&conn, &actor, "org_units", "unit_deleted", &id)
}

// ───────── subjects ─────────

#[derive(Deserialize)]
pub struct SubjectReq {
    institution_id: Option<String>,
    unit_id: Option<String>,
    name_ar: Option<String>,
    name_en: Option<String>,
    is_active: Option<bool>,
}

fn get_subject(conn: &Connection, id: &str) -> Res<Subject> {
    conn.query_row(
        "SELECT id, institution_id, unit_id, name_ar, name_en, is_active FROM subjects WHERE id = ?1",
        params![id],
        |r| {
            Ok(Subject {
                id: r.get(0)?,
                institution_id: r.get(1)?,
                unit_id: r.get(2)?,
                name_ar: r.get(3)?,
                name_en: r.get(4)?,
                is_active: r.get(5)?,
            })
        },
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found"))
}

fn create_subject(conn: &Connection, actor: &User, req: &SubjectReq) -> Res<Subject> {
    let inst = get_institution(conn, req.institution_id.as_deref().unwrap_or(""))?;
    if let Some(uid) = req.unit_id.as_deref() {
        if get_unit(conn, uid)?.institution_id != inst.id {
            return Err(bad("unit_other_institution"));
        }
    }
    let name_ar = text(req.name_ar.as_deref().unwrap_or(""), 200, "invalid_name")?;
    let name_en = opt_text(&req.name_en, 200, "name_too_long")?;
    let id = new_id();
    conn.execute(
        "INSERT INTO subjects(id, institution_id, unit_id, name_ar, name_en, is_active, created_at) VALUES (?1,?2,?3,?4,?5,1,?6)",
        params![id, inst.id, req.unit_id, name_ar, name_en, now_ms()],
    )
    .map_err(db_err)?;
    audit(conn, &actor.id, &id, "subject_created", &name_ar);
    get_subject(conn, &id)
}

fn update_subject(conn: &Connection, actor: &User, id: &str, req: &SubjectReq) -> Res<Subject> {
    let cur = get_subject(conn, id)?;
    let name_ar = match &req.name_ar {
        Some(n) => text(n, 200, "invalid_name")?,
        None => cur.name_ar,
    };
    let name_en = if req.name_en.is_some() { opt_text(&req.name_en, 200, "name_too_long")? } else { cur.name_en };
    conn.execute(
        "UPDATE subjects SET name_ar=?1, name_en=?2, is_active=?3 WHERE id=?4",
        params![name_ar, name_en, req.is_active.unwrap_or(cur.is_active), id],
    )
    .map_err(db_err)?;
    audit(conn, &actor.id, id, "subject_updated", &name_ar);
    get_subject(conn, id)
}

pub async fn create_subject_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SubjectReq>,
) -> Res<(StatusCode, Json<Subject>)> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    create_subject(&conn, &actor, &req).map(|s| (StatusCode::CREATED, Json(s)))
}

pub async fn update_subject_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SubjectReq>,
) -> Res<Json<Subject>> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    update_subject(&conn, &actor, &id, &req).map(Json)
}

pub async fn delete_subject_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<StatusCode> {
    let actor = require_admin(&state, &headers)?;
    let conn = lock(&state)?;
    delete_row(&conn, &actor, "subjects", "subject_deleted", &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    fn inst(conn: &Connection, admin: &User, t: &str) -> Institution {
        create_institution(
            conn,
            admin,
            &InstitutionReq { kind_type: Some(t.into()), name_ar: Some("جامعة".into()), name_en: None, city: None, is_active: None },
        )
        .unwrap()
    }

    fn unit(inst_id: &str, parent: Option<&str>, kind: &str) -> UnitReq {
        UnitReq {
            institution_id: Some(inst_id.into()),
            parent_id: parent.map(Into::into),
            kind: Some(kind.into()),
            name_ar: Some("س".into()),
            name_en: None,
            sort_order: None,
            is_active: None,
        }
    }

    #[test]
    fn approving_teacher_revokes_nothing_but_rejecting_kills_sessions() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "pending");
        conn.execute("INSERT INTO sessions VALUES ('h', ?1, 0, 9999999999999)", params![teacher.id]).unwrap();
        let ok = update_user(&conn, &admin, &teacher.id, &UserPatch { role: None, status: Some("active".into()), status_reason: None, full_name: None, password: None }, None).unwrap();
        assert_eq!(ok.status, "active");
        let sessions = || -> i64 { conn.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0)).unwrap() };
        assert_eq!(sessions(), 1, "approval keeps the teacher signed in");
        let rej = update_user(&conn, &admin, &teacher.id, &UserPatch { role: None, status: Some("rejected".into()), status_reason: Some("وثائق ناقصة".into()), full_name: None, password: None }, None).unwrap();
        assert_eq!(rej.status_reason.as_deref(), Some("وثائق ناقصة"));
        assert_eq!(sessions(), 0, "rejection signs the user out");
        let audit_n: i64 = conn.query_row("SELECT count(*) FROM audit_log", [], |r| r.get(0)).unwrap();
        assert_eq!(audit_n, 2);
    }

    #[test]
    fn admin_cannot_change_own_role_or_status_and_invalid_values_rejected() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let own = |role: Option<&str>, status: Option<&str>| UserPatch { role: role.map(Into::into), status: status.map(Into::into), status_reason: None, full_name: None, password: None };
        assert_eq!(update_user(&conn, &admin, &admin.id, &own(Some("student"), None), None).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_user(&conn, &admin, &admin.id, &own(None, Some("suspended")), None).unwrap_err().0, StatusCode::BAD_REQUEST);
        let other = insert_test_user(&conn, "o@x.com", "student", "active");
        assert_eq!(update_user(&conn, &admin, &other.id, &own(Some("god"), None), None).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_user(&conn, &admin, "nope", &own(None, None), None).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn user_search_escapes_wildcards() {
        let conn = create_test_db();
        insert_test_user(&conn, "a_b@x.com", "student", "active");
        insert_test_user(&conn, "axb@x.com", "student", "active");
        let f = |q: &str| UserFilter { status: None, role: None, q: Some(q.into()) };
        assert_eq!(list_users(&conn, &f("a_b")).unwrap().len(), 1);
        assert_eq!(list_users(&conn, &f("%")).unwrap().len(), 0);
    }

    #[test]
    fn unit_hierarchy_rules() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let uni = inst(&conn, &admin, "university");
        let school = inst(&conn, &admin, "school");
        assert_eq!(create_unit(&conn, &admin, &unit(&school.id, None, "department")).unwrap_err().0, StatusCode::BAD_REQUEST);
        let dept = create_unit(&conn, &admin, &unit(&uni.id, None, "department")).unwrap();
        let level = create_unit(&conn, &admin, &unit(&uni.id, Some(&dept.id), "level")).unwrap();
        assert_eq!(create_unit(&conn, &admin, &unit(&uni.id, Some(&level.id), "department")).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create_unit(&conn, &admin, &unit(&uni.id, Some(&level.id), "level")).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create_unit(&conn, &admin, &unit(&school.id, Some(&level.id), "year")).unwrap_err().0, StatusCode::BAD_REQUEST);
        create_unit(&conn, &admin, &unit(&uni.id, Some(&level.id), "term")).unwrap();
    }

    #[test]
    fn subjects_must_match_institution_and_delete_cascades() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let a = inst(&conn, &admin, "institute");
        let b = inst(&conn, &admin, "institute");
        let u = create_unit(&conn, &admin, &unit(&a.id, None, "level")).unwrap();
        let req = |i: &str, u: Option<&str>| SubjectReq { institution_id: Some(i.into()), unit_id: u.map(Into::into), name_ar: Some("رياضيات".into()), name_en: None, is_active: None };
        assert_eq!(create_subject(&conn, &admin, &req(&b.id, Some(&u.id))).unwrap_err().0, StatusCode::BAD_REQUEST);
        create_subject(&conn, &admin, &req(&a.id, Some(&u.id))).unwrap();
        delete_row(&conn, &admin, "institutions", "institution_deleted", &a.id).unwrap();
        let n: i64 = conn.query_row("SELECT (SELECT count(*) FROM subjects)+(SELECT count(*) FROM org_units)", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        assert_eq!(delete_row(&conn, &admin, "institutions", "x", &a.id).unwrap_err().0, StatusCode::NOT_FOUND);
    }
}
