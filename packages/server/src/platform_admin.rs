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
const DEFAULT_USERS_PAGE: i64 = MAX_USERS_LISTED; // unchanged default for old clients; the UI pages explicitly

// ───────── users ─────────

#[derive(Deserialize)]
pub struct UserFilter {
    status: Option<String>,
    role: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

fn list_users(conn: &Connection, f: &UserFilter) -> Res<Vec<User>> {
    let q = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_").to_lowercase())
    });
    let limit = f.limit.unwrap_or(DEFAULT_USERS_PAGE).clamp(1, MAX_USERS_LISTED);
    let offset = f.offset.unwrap_or(0).clamp(0, 1_000_000);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {USER_COLS} FROM users
             WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR role = ?2)
               AND (?3 IS NULL OR lower(email) LIKE ?3 ESCAPE '\\' OR lower(full_name) LIKE ?3 ESCAPE '\\')
             ORDER BY created_at DESC, id LIMIT ?4 OFFSET ?5"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![f.status, f.role, q, limit, offset], row_to_user)
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

#[derive(Deserialize, Default)]
pub struct UserPatch {
    role: Option<String>,
    status: Option<String>,
    status_reason: Option<String>,
    full_name: Option<String>,
    password: Option<String>,
    /// `school` | `institute` | `university` (phase 3-1: an admin can correct the kind of institution a user named).
    institution_type: Option<String>,
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
    let institution_type = match &patch.institution_type {
        Some(t) => Some(crate::platform_learning::valid_institution_type(t)?.to_string()),
        None => target.institution_type.clone(),
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
        "UPDATE users SET role = ?1, status = ?2, status_reason = ?3, full_name = ?4, institution_type = ?5 WHERE id = ?6",
        params![role, status, reason, full_name, institution_type, id],
    )
    .map_err(db_err)?;
    if let Some(h) = password_hash {
        conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![h, id]).map_err(db_err)?;
    }
    // Force re-login whenever access or credentials change: the role changes, the password is reset, or the account
    // *moves* to a non-active status. An edit that leaves a pending/rejected account where it is (a corrected name or
    // institution type) must not sign the person out — a pending teacher's waiting page relies on its session.
    if role != target.role || password_hash.is_some() || (status != target.status && status != "active") {
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
    if institution_type != target.institution_type {
        audit(
            conn,
            &actor.id,
            id,
            "user_institution_type_changed",
            &format!("{} -> {}", target.institution_type.as_deref().unwrap_or("-"), institution_type.as_deref().unwrap_or("-")),
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

/// Any active signed-in user (and a teacher still awaiting approval, who has to find subjects to ask for) may browse
/// institutions; admins also see inactive ones.
pub async fn list_institutions_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(f): Query<TypeFilter>,
) -> Res<Json<Vec<Institution>>> {
    let user = authenticate(&state, &headers)?;
    if !crate::platform_learning::may_browse_catalog(&user) {
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
    if !crate::platform_learning::may_browse_catalog(&user) {
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
        let ok = update_user(&conn, &admin, &teacher.id, &UserPatch { status: Some("active".into()), ..Default::default() }, None).unwrap();
        assert_eq!(ok.status, "active");
        let sessions = || -> i64 { conn.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0)).unwrap() };
        assert_eq!(sessions(), 1, "approval keeps the teacher signed in");
        let rej = update_user(&conn, &admin, &teacher.id, &UserPatch { status: Some("rejected".into()), status_reason: Some("وثائق ناقصة".into()), ..Default::default() }, None).unwrap();
        assert_eq!(rej.status_reason.as_deref(), Some("وثائق ناقصة"));
        assert_eq!(sessions(), 0, "rejection signs the user out");
        let audit_n: i64 = conn.query_row("SELECT count(*) FROM audit_log", [], |r| r.get(0)).unwrap();
        assert_eq!(audit_n, 2);
    }

    #[test]
    fn admin_cannot_change_own_role_or_status_and_invalid_values_rejected() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let own = |role: Option<&str>, status: Option<&str>| UserPatch { role: role.map(Into::into), status: status.map(Into::into), ..Default::default() };
        assert_eq!(update_user(&conn, &admin, &admin.id, &own(Some("student"), None), None).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_user(&conn, &admin, &admin.id, &own(None, Some("suspended")), None).unwrap_err().0, StatusCode::BAD_REQUEST);
        let other = insert_test_user(&conn, "o@x.com", "student", "active");
        assert_eq!(update_user(&conn, &admin, &other.id, &own(Some("god"), None), None).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(update_user(&conn, &admin, "nope", &own(None, None), None).unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn admin_can_correct_a_users_institution_type_with_validation_and_an_audit_row() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "pending");
        let inst = |c: &Connection| -> Option<String> { c.query_row("SELECT institution_type FROM users WHERE id = ?1", params![teacher.id], |r| r.get(0)).unwrap() };
        let audits = |c: &Connection| -> i64 { c.query_row("SELECT count(*) FROM audit_log WHERE action = 'user_institution_type_changed'", [], |r| r.get(0)).unwrap() };
        let set = |v: &str| UserPatch { institution_type: Some(v.into()), ..Default::default() };

        let u = update_user(&conn, &admin, &teacher.id, &set("institute"), None).unwrap();
        assert_eq!((u.institution_type.as_deref(), inst(&conn).as_deref(), audits(&conn)), (Some("institute"), Some("institute"), 1));
        let detail: String = conn.query_row("SELECT detail FROM audit_log WHERE action = 'user_institution_type_changed'", [], |r| r.get(0)).unwrap();
        assert_eq!(detail, "- -> institute");
        // the same value again is not a change; a real change is audited with both values
        update_user(&conn, &admin, &teacher.id, &set("institute"), None).unwrap();
        assert_eq!(audits(&conn), 1);
        update_user(&conn, &admin, &teacher.id, &set("university"), None).unwrap();
        let last: String = conn.query_row("SELECT detail FROM audit_log WHERE action = 'user_institution_type_changed' ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!((audits(&conn), last.as_str()), (2, "institute -> university"));
        // invalid values are refused and change nothing — not even the other fields of the same patch
        for bad_value in ["college", "", "University"] {
            let patch = UserPatch { institution_type: Some(bad_value.into()), full_name: Some("اسم آخر".into()), status: Some("active".into()), ..Default::default() };
            let e = update_user(&conn, &admin, &teacher.id, &patch, None).unwrap_err();
            assert_eq!((e.0, e.1.contains("invalid_type")), (StatusCode::BAD_REQUEST, true), "{bad_value:?}");
        }
        let (name, status): (String, String) = conn.query_row("SELECT full_name, status FROM users WHERE id = ?1", params![teacher.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((name.as_str(), status.as_str(), inst(&conn).as_deref(), audits(&conn)), ("T", "pending", Some("university"), 2));
        // a patch without the field leaves it as it was
        update_user(&conn, &admin, &teacher.id, &UserPatch { full_name: Some("معلم".into()), ..Default::default() }, None).unwrap();
        assert_eq!((inst(&conn).as_deref(), audits(&conn)), (Some("university"), 2));
    }

    #[test]
    fn editing_an_account_that_stays_pending_does_not_sign_it_out_but_real_access_changes_do() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let teacher = insert_test_user(&conn, "t@x.com", "teacher", "pending");
        let sessions = |c: &Connection| -> i64 { c.query_row("SELECT count(*) FROM sessions WHERE user_id = ?1", params![teacher.id], |r| r.get(0)).unwrap() };
        let sign_in = |c: &Connection| c.execute("INSERT OR REPLACE INTO sessions VALUES ('h', ?1, 0, 9999999999999)", params![teacher.id]).unwrap();
        sign_in(&conn);

        // a corrected institution type / name / reason-less re-save of the same status keeps the waiting page alive
        update_user(&conn, &admin, &teacher.id, &UserPatch { institution_type: Some("school".into()), ..Default::default() }, None).unwrap();
        update_user(&conn, &admin, &teacher.id, &UserPatch { full_name: Some("معلم جديد".into()), ..Default::default() }, None).unwrap();
        update_user(&conn, &admin, &teacher.id, &UserPatch { status: Some("pending".into()), ..Default::default() }, None).unwrap();
        assert_eq!(sessions(&conn), 1, "pending stays pending: still signed in");

        // credentials, role and a move to a non-active status all still force a new sign-in
        update_user(&conn, &admin, &teacher.id, &UserPatch::default(), Some("new-hash")).unwrap();
        assert_eq!(sessions(&conn), 0, "password reset");
        sign_in(&conn);
        update_user(&conn, &admin, &teacher.id, &UserPatch { role: Some("student".into()), ..Default::default() }, None).unwrap();
        assert_eq!(sessions(&conn), 0, "role change");
        sign_in(&conn);
        update_user(&conn, &admin, &teacher.id, &UserPatch { status: Some("suspended".into()), ..Default::default() }, None).unwrap();
        assert_eq!(sessions(&conn), 0, "moved to suspended");
        // an edit that leaves the (already non-active) status alone is not an access change either
        sign_in(&conn);
        update_user(&conn, &admin, &teacher.id, &UserPatch { full_name: Some("اسم".into()), ..Default::default() }, None).unwrap();
        assert_eq!(sessions(&conn), 1, "an edit that changes no access leaves a session alone");
    }

    #[test]
    fn user_search_escapes_wildcards() {
        let conn = create_test_db();
        insert_test_user(&conn, "a_b@x.com", "student", "active");
        insert_test_user(&conn, "axb@x.com", "student", "active");
        let f = |q: &str| UserFilter { status: None, role: None, q: Some(q.into()), limit: None, offset: None };
        assert_eq!(list_users(&conn, &f("a_b")).unwrap().len(), 1);
        assert_eq!(list_users(&conn, &f("%")).unwrap().len(), 0);
    }

    #[test]
    fn user_listing_pages_without_gaps_or_repeats() {
        let conn = create_test_db();
        for i in 0..7 {
            insert_test_user(&conn, &format!("u{i}@x.com"), if i % 2 == 0 { "student" } else { "teacher" }, "active");
        }
        let page = |limit, offset, role: Option<&str>| {
            let f = UserFilter { status: None, role: role.map(Into::into), q: None, limit: Some(limit), offset: Some(offset) };
            list_users(&conn, &f).unwrap().into_iter().map(|u| u.id).collect::<Vec<_>>()
        };
        let all = page(200, 0, None);
        assert_eq!(all.len(), 7);
        let stitched: Vec<_> = [page(3, 0, None), page(3, 3, None), page(3, 6, None)].concat();
        assert_eq!(stitched, all, "pages are contiguous and stable even with equal created_at");
        assert!(page(3, 7, None).is_empty());
        assert_eq!(page(50, 0, Some("teacher")).len(), 3);
        assert_eq!(page(0, -5, None).len(), 1, "limit is clamped to at least 1 and offset to at least 0");
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
