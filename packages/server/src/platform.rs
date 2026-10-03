//! Learning-platform accounts (admin / teacher / student), fully self-hosted.
//!
//! Data lives in its own SQLite file (`PLATFORM_DB_PATH`), separate from the exam relay DB,
//! because the relay purges its data after 7 days and accounts must be permanent.
//! Roles/status are only ever decided server-side; the client cannot choose them.

use crate::relay::{client_ip, err, now_ms, sha256_hex, Err};
use crate::routes::AppState;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{extract::State, http::{HeaderMap, StatusCode}, Json};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};

const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 128;
const REGISTER_PER_HOUR_PER_IP: i64 = 10;
const LOGIN_PER_10MIN_PER_IP: i64 = 30;
const LOGIN_PER_10MIN_PER_EMAIL: i64 = 10;

pub struct PlatformState {
    pub conn: Mutex<Connection>,
    /// Directory for uploaded attachments (`PLATFORM_FILES_DIR`).
    pub files_dir: std::path::PathBuf,
}

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  full_name TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('admin','institution_admin','moderator','teacher','student')),
  institution_type TEXT CHECK (institution_type IN ('school','institute','university')),
  status TEXT NOT NULL CHECK (status IN ('active','pending','rejected','suspended')),
  status_reason TEXT,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE TABLE IF NOT EXISTS rate_limits (
  key TEXT NOT NULL,
  bucket INTEGER NOT NULL,
  count INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (key, bucket)
);
CREATE TABLE IF NOT EXISTS institutions (
  id TEXT PRIMARY KEY,
  type TEXT NOT NULL CHECK (type IN ('school','institute','university')),
  name_ar TEXT NOT NULL,
  name_en TEXT,
  city TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS org_units (
  id TEXT PRIMARY KEY,
  institution_id TEXT NOT NULL REFERENCES institutions(id) ON DELETE CASCADE,
  parent_id TEXT REFERENCES org_units(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('department','level','year','term')),
  name_ar TEXT NOT NULL,
  name_en TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_org_units_inst ON org_units(institution_id, parent_id);
CREATE TABLE IF NOT EXISTS subjects (
  id TEXT PRIMARY KEY,
  institution_id TEXT NOT NULL REFERENCES institutions(id) ON DELETE CASCADE,
  unit_id TEXT REFERENCES org_units(id) ON DELETE SET NULL,
  name_ar TEXT NOT NULL,
  name_en TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_subjects_inst ON subjects(institution_id, unit_id);
CREATE TABLE IF NOT EXISTS audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  actor_id TEXT,
  target_id TEXT,
  action TEXT NOT NULL,
  detail TEXT,
  created_at INTEGER NOT NULL
);
";

/// Applies the core schema plus every feature module's schema (all idempotent).
pub fn apply_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    for extra in [crate::platform_learning::SCHEMA, crate::platform_content::SCHEMA] {
        conn.execute_batch(extra).map_err(|e| e.to_string())?;
    }
    // Columns added after the first release.
    add_column_if_missing(conn, "users", "bio", "TEXT")?;
    Ok(())
}

fn add_column_if_missing(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<(), String> {
    let exists: bool = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>())
        .map_err(|e| e.to_string())?
        .iter()
        .any(|c| c == column);
    if !exists {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn init_db(path: &str, files_dir: &str) -> Result<PlatformState, String> {
    std::fs::create_dir_all(files_dir).map_err(|e| format!("cannot create {files_dir}: {e}"))?;
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    apply_schema(&conn)?;
    Ok(PlatformState { conn: Mutex::new(conn), files_dir: files_dir.into() })
}

pub fn cleanup_expired(state: &PlatformState) {
    let now = now_ms();
    if let Ok(conn) = state.conn.lock() {
        let _ = conn.execute("DELETE FROM sessions WHERE expires_at < ?1", params![now]);
        let _ = conn.execute("DELETE FROM rate_limits WHERE expires_at < ?1", params![now]);
    }
}

// ───────── users & passwords ─────────

#[derive(Serialize, Clone, Debug)]
pub struct User {
    pub id: String,
    pub email: String,
    pub full_name: String,
    pub role: String,
    pub institution_type: Option<String>,
    pub status: String,
    pub status_reason: Option<String>,
}

pub fn hash_password(password: &str) -> Result<String, Err> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "hash_failed"))
}

fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// Verified against when the email is unknown, so response time does not reveal which emails exist.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password("dummy-password-for-timing").unwrap_or_default())
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn gen_session_token() -> String {
    let bytes: [u8; 32] = rand::thread_rng().gen();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn normalize_email(raw: &str) -> Option<String> {
    let e = raw.trim().to_lowercase();
    let (local, domain) = e.split_once('@')?;
    let ok = e.len() <= 254
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !e.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(e)
}

/// Fixed-window counter. Returns 429 once `max` hits are exceeded in the window.
fn rate_limit(conn: &Connection, key: &str, window_ms: i64, max: i64) -> Result<(), Err> {
    let bucket = now_ms() / window_ms;
    let expires = (bucket + 1) * window_ms + window_ms;
    let count: i64 = conn
        .query_row(
            "INSERT INTO rate_limits(key, bucket, count, expires_at) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(key, bucket) DO UPDATE SET count = count + 1 RETURNING count",
            params![key, bucket, expires],
            |r| r.get(0),
        )
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
    if count > max {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    }
    Ok(())
}

pub const USER_COLS: &str = "id, email, full_name, role, institution_type, status, status_reason";

pub fn row_to_user(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        email: r.get(1)?,
        full_name: r.get(2)?,
        role: r.get(3)?,
        institution_type: r.get(4)?,
        status: r.get(5)?,
        status_reason: r.get(6)?,
    })
}

#[derive(Deserialize)]
pub struct RegisterReq {
    pub email: String,
    pub password: String,
    pub full_name: String,
    pub role: String,
    pub institution_type: Option<String>,
}

/// Validates the request and inserts the user. The role is whitelisted here:
/// anything other than `teacher` becomes `student`; teachers start `pending`.
fn insert_user(conn: &Connection, req: &RegisterReq, password_hash: &str) -> Result<User, Err> {
    let email = normalize_email(&req.email).ok_or_else(|| err(StatusCode::BAD_REQUEST, "invalid_email"))?;
    let full_name = req.full_name.trim();
    if full_name.is_empty() || full_name.chars().count() > 120 {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_name"));
    }
    let inst = match req.institution_type.as_deref() {
        Some(t @ ("school" | "institute" | "university")) => Some(t.to_string()),
        _ => None,
    };
    let (role, status) = if req.role == "teacher" { ("teacher", "pending") } else { ("student", "active") };
    let id = new_id();
    conn.execute(
        "INSERT INTO users(id, email, password_hash, full_name, role, institution_type, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, email, password_hash, full_name, role, inst, status, now_ms()],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
            err(StatusCode::CONFLICT, "email_taken")
        }
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"),
    })?;
    Ok(User {
        id,
        email,
        full_name: full_name.to_string(),
        role: role.to_string(),
        institution_type: inst,
        status: status.to_string(),
        status_reason: None,
    })
}

fn create_session(conn: &Connection, user_id: &str) -> Result<String, Err> {
    let token = gen_session_token();
    let now = now_ms();
    conn.execute(
        "INSERT INTO sessions(token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![sha256_hex(&token), user_id, now, now + SESSION_TTL_MS],
    )
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
    Ok(token)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

fn user_for_token(conn: &Connection, token: &str) -> Option<User> {
    conn.query_row(
        &format!(
            "SELECT {USER_COLS} FROM users WHERE id = (SELECT user_id FROM sessions WHERE token_hash = ?1 AND expires_at > ?2)"
        ),
        params![sha256_hex(token), now_ms()],
        row_to_user,
    )
    .optional()
    .ok()
    .flatten()
}

/// Resolves the signed-in user from the `Authorization: Bearer` header.
pub fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<User, Err> {
    let token = bearer(headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
    user_for_token(&conn, token).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "unauthorized"))
}

/// Creates the first admin from env vars when no admin exists yet (no default credentials).
pub fn bootstrap_admin(state: &PlatformState, email: &str, password: &str) -> Result<bool, String> {
    let email = normalize_email(email).ok_or("PLATFORM_ADMIN_EMAIL is not a valid email")?;
    if password.chars().count() < MIN_PASSWORD {
        return Err(format!("PLATFORM_ADMIN_PASSWORD must be at least {MIN_PASSWORD} characters"));
    }
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM users WHERE role = 'admin')", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if exists {
        return Ok(false);
    }
    let hash = hash_password(password).map_err(|e| e.1)?;
    conn.execute(
        "INSERT INTO users(id, email, password_hash, full_name, role, status, created_at)
         VALUES (?1, ?2, ?3, 'Admin', 'admin', 'active', ?4)",
        params![new_id(), email, hash, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}


// ───────── shared helpers for platform modules ─────────

pub type Res<T> = Result<T, Err>;

pub fn db_err<E>(_: E) -> Err {
    err(StatusCode::INTERNAL_SERVER_ERROR, "db_error")
}

pub fn bad(code: &str) -> Err {
    err(StatusCode::BAD_REQUEST, code)
}

pub fn lock(state: &AppState) -> Res<std::sync::MutexGuard<'_, Connection>> {
    state.platform.conn.lock().map_err(db_err)
}

/// Signed-in user whose account is active (not pending/rejected/suspended).
pub fn require_active(state: &AppState, headers: &HeaderMap) -> Res<User> {
    let user = authenticate(state, headers)?;
    if user.status == "active" {
        Ok(user)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

pub fn require_role(state: &AppState, headers: &HeaderMap, role: &str) -> Res<User> {
    let user = require_active(state, headers)?;
    if user.role == role {
        Ok(user)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

pub fn require_admin(state: &AppState, headers: &HeaderMap) -> Res<User> {
    require_role(state, headers, "admin")
}

/// Trimmed, non-empty, at most `max` characters.
pub fn text(raw: &str, max: usize, code: &str) -> Res<String> {
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > max {
        Err(bad(code))
    } else {
        Ok(t.to_string())
    }
}

/// Optional text: empty/absent -> None; too long -> error.
pub fn opt_text(raw: &Option<String>, max: usize, code: &str) -> Res<Option<String>> {
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) if t.chars().count() <= max => Ok(Some(t.to_string())),
        _ => Err(bad(code)),
    }
}

pub fn audit(conn: &Connection, actor: &str, target: &str, action: &str, detail: &str) {
    let _ = conn.execute(
        "INSERT INTO audit_log(actor_id, target_id, action, detail, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![actor, target, action, detail, now_ms()],
    );
}

// ───────── HTTP handlers ─────────

pub async fn register_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RegisterReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), Err> {
    if req.password.chars().count() < MIN_PASSWORD || req.password.chars().count() > MAX_PASSWORD {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_password"));
    }
    {
        let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
        rate_limit(&conn, &format!("reg:{}", client_ip(&headers)), 3_600_000, REGISTER_PER_HOUR_PER_IP)?;
    }
    let pw = req.password.clone();
    let hash = tokio::task::spawn_blocking(move || hash_password(&pw))
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "hash_failed"))??;
    let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
    let user = insert_user(&conn, &req, &hash)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "status": user.status }))))
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
}

pub async fn login_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Result<Json<serde_json::Value>, Err> {
    let email = normalize_email(&req.email).unwrap_or_default();
    let found: Option<(User, String)> = {
        let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
        rate_limit(&conn, &format!("login-ip:{}", client_ip(&headers)), 600_000, LOGIN_PER_10MIN_PER_IP)?;
        rate_limit(&conn, &format!("login-email:{email}"), 600_000, LOGIN_PER_10MIN_PER_EMAIL)?;
        conn.query_row(
            &format!("SELECT {USER_COLS}, password_hash FROM users WHERE email = ?1"),
            params![email],
            |r| Ok((row_to_user(r)?, r.get::<_, String>(7)?)),
        )
        .optional()
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?
    };

    let (user, phc) = match found {
        Some((u, h)) => (Some(u), h),
        None => (None, dummy_hash().to_string()),
    };
    let password = req.password;
    let ok = tokio::task::spawn_blocking(move || verify_password(&password, &phc))
        .await
        .unwrap_or(false);
    let user = match (ok, user) {
        (true, Some(u)) => u,
        _ => return Err(err(StatusCode::UNAUTHORIZED, "invalid_credentials")),
    };

    let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
    let token = create_session(&conn, &user.id)?;
    Ok(Json(serde_json::json!({ "token": token, "user": user })))
}

pub async fn me_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result<Json<User>, Err> {
    authenticate(&state, &headers).map(Json)
}

pub async fn logout_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> StatusCode {
    if let (Some(token), Ok(conn)) = (bearer(&headers), state.platform.conn.lock()) {
        let _ = conn.execute("DELETE FROM sessions WHERE token_hash = ?1", params![sha256_hex(token)]);
    }
    StatusCode::NO_CONTENT
}

#[cfg(test)]
pub fn create_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    apply_schema(&conn).unwrap();
    conn
}

#[cfg(test)]
pub fn insert_test_user(conn: &Connection, email: &str, role: &str, status: &str) -> User {
    let id = new_id();
    conn.execute(
        "INSERT INTO users(id, email, password_hash, full_name, role, status, created_at) VALUES (?1, ?2, 'h', 'T', ?3, ?4, 0)",
        params![id, email, role, status],
    )
    .unwrap();
    User { id, email: email.into(), full_name: "T".into(), role: role.into(), institution_type: None, status: status.into(), status_reason: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        create_test_db()
    }

    fn req(email: &str, role: &str) -> RegisterReq {
        RegisterReq {
            email: email.into(),
            password: "password123".into(),
            full_name: "Test".into(),
            role: role.into(),
            institution_type: Some("university".into()),
        }
    }

    #[test]
    fn role_is_whitelisted_and_teacher_starts_pending() {
        let conn = db();
        let admin_attempt = insert_user(&conn, &req("a@x.com", "admin"), "h").unwrap();
        assert_eq!((admin_attempt.role.as_str(), admin_attempt.status.as_str()), ("student", "active"));
        let teacher = insert_user(&conn, &req("t@x.com", "teacher"), "h").unwrap();
        assert_eq!((teacher.role.as_str(), teacher.status.as_str()), ("teacher", "pending"));
    }

    #[test]
    fn rejects_bad_input_and_duplicates() {
        let conn = db();
        assert_eq!(insert_user(&conn, &req("not-an-email", "student"), "h").unwrap_err().0, StatusCode::BAD_REQUEST);
        let mut r = req("ok@x.com", "student");
        r.institution_type = Some("bogus".into());
        assert!(insert_user(&conn, &r, "h").unwrap().institution_type.is_none());
        assert_eq!(insert_user(&conn, &req("OK@x.com", "student"), "h").unwrap_err().0, StatusCode::CONFLICT);
    }

    #[test]
    fn password_hash_roundtrip() {
        let h = hash_password("password123").unwrap();
        assert!(verify_password("password123", &h));
        assert!(!verify_password("wrong-password", &h));
        assert!(!verify_password("x", "not-a-phc-string"));
    }

    #[test]
    fn sessions_authenticate_and_expire() {
        let conn = db();
        let u = insert_user(&conn, &req("s@x.com", "student"), "h").unwrap();
        let token = create_session(&conn, &u.id).unwrap();
        assert_eq!(user_for_token(&conn, &token).unwrap().id, u.id);
        assert!(user_for_token(&conn, "bogus").is_none());
        conn.execute("UPDATE sessions SET expires_at = 0", []).unwrap();
        assert!(user_for_token(&conn, &token).is_none());
    }

    #[test]
    fn rate_limit_blocks_after_max() {
        let conn = db();
        for _ in 0..3 {
            rate_limit(&conn, "k", 60_000, 3).unwrap();
        }
        assert_eq!(rate_limit(&conn, "k", 60_000, 3).unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
        rate_limit(&conn, "other", 60_000, 3).unwrap();
    }

    #[test]
    fn bootstrap_creates_admin_once() {
        let state = PlatformState { conn: Mutex::new(db()), files_dir: std::env::temp_dir() };
        assert!(bootstrap_admin(&state, "Root@x.com", "password123").unwrap());
        assert!(!bootstrap_admin(&state, "other@x.com", "password123").unwrap());
        assert!(bootstrap_admin(&PlatformState { conn: Mutex::new(db()), files_dir: std::env::temp_dir() }, "r@x.com", "short").is_err());
    }
}
