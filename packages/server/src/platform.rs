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
/// The window of the sign-in failure counters ([`AuthLimits`]).
const LOGIN_WINDOW_MS: i64 = 600_000;
const REGISTER_WINDOW_MS: i64 = 3_600_000;

/// Sign-in and registration limits (phase 4-1), read once at start ([`AuthLimits::from_env`]); `0` switches one off.
///
/// Only **failed** sign-ins count (a wrong password or an unknown e-mail): a whole class signing in from one school
/// address never runs into a limit, and a correct password is never refused because of somebody else's mistakes unless
/// a counter is already full. The per-e-mail counter is kept per e-mail **and client address**, so a stranger on another
/// network cannot lock a student out; a looser per-e-mail counter across all addresses stays as abuse protection.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthLimits {
    /// `PLATFORM_LOGIN_FAILS_PER_10MIN_PER_IP` (default 30): failed sign-ins per client address.
    pub login_fails_per_ip: i64,
    /// `PLATFORM_LOGIN_FAILS_PER_10MIN_PER_EMAIL_IP` (default 10): failed sign-ins per e-mail from one address.
    pub login_fails_per_email_ip: i64,
    /// `PLATFORM_LOGIN_FAILS_PER_10MIN_PER_EMAIL` (default 50): failed sign-ins per e-mail from all addresses.
    pub login_fails_per_email: i64,
    /// `PLATFORM_REGISTER_PER_HOUR_PER_IP` (default 60): registration requests per client address and hour.
    pub register_per_ip: i64,
}

impl Default for AuthLimits {
    fn default() -> Self {
        AuthLimits { login_fails_per_ip: 30, login_fails_per_email_ip: 10, login_fails_per_email: 50, register_per_ip: 60 }
    }
}

impl AuthLimits {
    pub fn from_env() -> Self {
        Self::parse(|k| std::env::var(k).ok())
    }

    /// Each value: a whole number `>= 0` (`0` = off); anything else keeps the default rather than switching a
    /// protection off by accident.
    pub fn parse(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = AuthLimits::default();
        let n = |k: &str, default: i64| get(k).and_then(|v| v.trim().parse::<u32>().ok()).map_or(default, i64::from);
        AuthLimits {
            login_fails_per_ip: n("PLATFORM_LOGIN_FAILS_PER_10MIN_PER_IP", d.login_fails_per_ip),
            login_fails_per_email_ip: n("PLATFORM_LOGIN_FAILS_PER_10MIN_PER_EMAIL_IP", d.login_fails_per_email_ip),
            login_fails_per_email: n("PLATFORM_LOGIN_FAILS_PER_10MIN_PER_EMAIL", d.login_fails_per_email),
            register_per_ip: n("PLATFORM_REGISTER_PER_HOUR_PER_IP", d.register_per_ip),
        }
    }

    pub fn describe(&self) -> String {
        let v = |n: i64| if n == 0 { "off".to_string() } else { n.to_string() };
        format!(
            "failed sign-ins per 10 min: {} per address, {} per e-mail+address, {} per e-mail; registrations per hour per address: {}",
            v(self.login_fails_per_ip),
            v(self.login_fails_per_email_ip),
            v(self.login_fails_per_email),
            v(self.register_per_ip)
        )
    }
}

pub struct PlatformState {
    pub conn: Mutex<Connection>,
    /// Directory for uploaded attachments (`PLATFORM_FILES_DIR`).
    pub files_dir: std::path::PathBuf,
    /// Encrypts secrets stored in the `settings` table (AI key, SMTP password); read from phase 1-3.
    #[allow(dead_code)]
    pub crypto: crate::platform_settings::Crypto,
    /// Where the database, key, attachments and backups live (phase 1-9).
    pub layout: crate::platform_backup::Layout,
    pub backup: crate::platform_backup::BackupConfig,
    /// Set while a backup is being written: only one at a time.
    pub backup_busy: std::sync::atomic::AtomicBool,
    pub started_at: i64,
    /// Short-lived memo of the public landing-page counters (phase 2-1).
    pub public_stats: crate::platform_stats::StatsCache,
    /// Sign-in / registration limits (phase 4-1).
    pub limits: AuthLimits,
    /// How old a reporter's account must be for its report to count towards the automatic hide (phase 4-2).
    pub report_limits: crate::platform_ops::ReportLimits,
}

impl PlatformState {
    /// A state over an in-memory connection with its own scratch directories (tests only).
    #[cfg(test)]
    pub fn for_tests(conn: Connection) -> Self {
        let root = std::env::temp_dir().join(format!("exameow-test-{}", new_id()));
        let layout = crate::platform_backup::Layout { db_path: root.join("platform.db"), files_dir: root.join("files"), key_file: root.join("platform.key"), backup_dir: root.join("backups") };
        PlatformState {
            conn: Mutex::new(conn),
            files_dir: layout.files_dir.clone(),
            crypto: crate::platform_settings::Crypto::for_tests(),
            layout,
            backup: crate::platform_backup::BackupConfig::default(),
            backup_busy: std::sync::atomic::AtomicBool::new(false),
            started_at: 0,
            public_stats: Default::default(),
            limits: AuthLimits::default(),
            report_limits: Default::default(),
        }
    }
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
    // Every connection that gets the schema (the production open path and the test databases alike) gets the SQL
    // functions the queries use.
    register_sql_functions(conn)?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    for extra in [crate::platform_learning::SCHEMA, crate::platform_content::SCHEMA, crate::platform_engage::SCHEMA, crate::platform_exams::SCHEMA, crate::platform_ops::SCHEMA, crate::platform_settings::SCHEMA, crate::platform_ai::SCHEMA, crate::platform_bank::SCHEMA, crate::platform_reminders::SCHEMA, crate::platform_backup::SCHEMA, crate::platform_bookmarks::SCHEMA] {
        conn.execute_batch(extra).map_err(|e| e.to_string())?;
    }
    crate::platform_exams::migrate(conn)?;
    // `exam_reminders` learned a third kind (phase 4-2); an older table is rebuilt with the wider CHECK
    crate::platform_reminders::migrate(conn)?;
    // when a post / course was first announced (phase 3-4)
    crate::platform_content::migrate(conn)?;
    // Columns added after the first release.
    add_column_if_missing(conn, "users", "bio", "TEXT")?;
    add_column_if_missing(conn, "users", "consented_at", "INTEGER")?;
    // The admin's optional note on a teaching decision (phase 3-2); shown to the teacher only.
    add_column_if_missing(conn, "teacher_subjects", "reason", "TEXT")?;
    Ok(())
}

pub fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    Ok(conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>())
        .map_err(|e| e.to_string())?
        .iter()
        .any(|c| c == column))
}

pub fn add_column_if_missing(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<(), String> {
    if !column_exists(conn, table, column)? {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn init_db(layout: &crate::platform_backup::Layout) -> Result<PlatformState, String> {
    let files_dir = &layout.files_dir;
    std::fs::create_dir_all(files_dir).map_err(|e| format!("cannot create {}: {e}", files_dir.display()))?;
    let crypto = crate::platform_settings::Crypto::from_key_file(&layout.key_file)?;
    let conn = Connection::open(&layout.db_path).map_err(|e| e.to_string())?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    apply_schema(&conn)?;
    Ok(PlatformState {
        conn: Mutex::new(conn),
        files_dir: files_dir.clone(),
        crypto,
        layout: layout.clone(),
        backup: crate::platform_backup::BackupConfig::from_env(),
        backup_busy: std::sync::atomic::AtomicBool::new(false),
        started_at: now_ms(),
        public_stats: Default::default(),
        limits: AuthLimits::from_env(),
        report_limits: crate::platform_ops::ReportLimits::from_env(),
    })
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

pub fn verify_password(password: &str, phc: &str) -> bool {
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
pub fn rate_limit(conn: &Connection, key: &str, window_ms: i64, max: i64) -> Result<(), Err> {
    let count = bump(conn, key, now_ms() / window_ms, window_ms)?;
    if count > max {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    }
    Ok(())
}

/// One more hit on `key` in window `bucket`; returns the new count.
fn bump(conn: &Connection, key: &str, bucket: i64, window_ms: i64) -> Result<i64, Err> {
    let expires = (bucket + 1) * window_ms + window_ms;
    conn.query_row(
        "INSERT INTO rate_limits(key, bucket, count, expires_at) VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(key, bucket) DO UPDATE SET count = count + 1 RETURNING count",
        params![key, bucket, expires],
        |r| r.get(0),
    )
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))
}

/// The failure counters one sign-in attempt holds while its password is checked (see [`hold_login_attempt`]).
#[derive(Debug, Default)]
pub(crate) struct LoginHold {
    /// `(key, bucket)` of each counter that was bumped.
    held: Vec<(String, i64)>,
}

impl LoginHold {
    /// The sign-in succeeded: it gives its place back, so a correct password never uses up a counter.
    pub(crate) fn release(self, conn: &Connection) {
        for (key, bucket) in self.held {
            let _ = conn.execute("UPDATE rate_limits SET count = count - 1 WHERE key = ?1 AND bucket = ?2 AND count > 0", params![key, bucket]);
        }
    }
}

/// The three sign-in failure counters of a request: per client address, per e-mail from that address, per e-mail from
/// anywhere. The e-mail is hashed: whatever was typed into the field (sometimes a password) is never stored.
fn login_keys(limits: &AuthLimits, ip: &str, email: &str) -> [(String, i64); 3] {
    let e = sha256_hex(email);
    [
        (format!("login-fail-ip:{ip}"), limits.login_fails_per_ip),
        (format!("login-fail-email-ip:{e}:{ip}"), limits.login_fails_per_email_ip),
        (format!("login-fail-email:{e}"), limits.login_fails_per_email),
    ]
}

/// Takes a place on every sign-in failure counter BEFORE the password is checked (all under the database lock, so a
/// burst of parallel guesses cannot all slip past a full counter), or answers 429 `rate_limited` when one is full (that
/// request takes nothing). A failed attempt keeps its places; a successful one hands them back ([`LoginHold::release`]),
/// so only failures are ever counted. A counter whose limit is 0 is off.
pub(crate) fn hold_login_attempt(conn: &Connection, limits: &AuthLimits, ip: &str, email: &str, now: i64) -> Result<LoginHold, Err> {
    let bucket = now / LOGIN_WINDOW_MS;
    let mut hold = LoginHold::default();
    for (key, max) in login_keys(limits, ip, email) {
        if max <= 0 {
            continue;
        }
        let count = match bump(conn, &key, bucket, LOGIN_WINDOW_MS) {
            Ok(c) => c,
            Err(e) => {
                hold.release(conn);
                return Err(e);
            }
        };
        hold.held.push((key, bucket));
        if count > max {
            hold.release(conn);
            return Err(err(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
        }
    }
    Ok(hold)
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
    /// Must be `true`: acceptance of the terms/privacy policy (recorded with a timestamp).
    pub consent: Option<bool>,
}

/// Validates the request and inserts the user. The role is whitelisted here:
/// anything other than `teacher` becomes `student`; teachers start `pending`.
fn insert_user(conn: &Connection, req: &RegisterReq, password_hash: &str) -> Result<User, Err> {
    if req.consent != Some(true) {
        return Err(err(StatusCode::BAD_REQUEST, "consent_required"));
    }
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
        "INSERT INTO users(id, email, password_hash, full_name, role, institution_type, status, created_at, consented_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
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

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

pub(crate) fn user_for_token(conn: &Connection, token: &str) -> Option<User> {
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

/// An already-authenticated user whose account is active (not pending/rejected/suspended).
/// The pure half of [`require_active`], so the rule can be tested without a request.
pub fn ensure_active(user: User) -> Res<User> {
    if user.status == "active" {
        Ok(user)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

/// An already-authenticated, active user holding `role` (the pure half of [`require_role`]).
pub fn ensure_role(user: User, role: &str) -> Res<User> {
    let user = ensure_active(user)?;
    if user.role == role {
        Ok(user)
    } else {
        Err(err(StatusCode::FORBIDDEN, "forbidden"))
    }
}

/// Signed-in user whose account is active (not pending/rejected/suspended).
pub fn require_active(state: &AppState, headers: &HeaderMap) -> Res<User> {
    ensure_active(authenticate(state, headers)?)
}

pub fn require_role(state: &AppState, headers: &HeaderMap, role: &str) -> Res<User> {
    ensure_role(authenticate(state, headers)?, role)
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

/// SQL condition for the alias `s` (subjects): the subject itself is switched on.
pub const SUBJECT_ACTIVE: &str = "s.is_active = 1";

/// SQL condition for the alias `s` (subjects): the subject's *institution* is switched on. Hiding an institution
/// hides everything below it, so every "may students see / may a teacher publish into this subject?" predicate
/// carries this next to [`SUBJECT_ACTIVE`]. A correlated `EXISTS` instead of a join, so no extra alias is needed.
pub const INSTITUTION_ACTIVE: &str = "EXISTS(SELECT 1 FROM institutions inst WHERE inst.id = s.institution_id AND inst.is_active = 1)";

/// `%…%` pattern for a case-insensitive `LIKE … ESCAPE '\'`: `%`, `_` and `\` in the search text are literal.
pub fn like_pattern(raw: &str) -> String {
    format!("%{}%", raw.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_").to_lowercase())
}

/// The `LIKE … ESCAPE '\'` pattern for an Arabic-folded search: the text is folded with [`ar_fold`] (so the pattern and the
/// column — `ar_fold(col) LIKE ?` — are compared in the same form) and `%`, `_` and `\` in it are literal.
pub fn fold_like(raw: &str) -> String {
    format!("%{}%", ar_fold(raw).replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

/// **The one Arabic folding for searches** (and the SQL function `ar_fold`): the folding the answer checker and the question
/// bank already use (`platform_exams::norm_text` — lowercase, no diacritics / tatweel, every alef form to a plain alef, ى to ي)
/// with runs of whitespace collapsed (`platform_bank::stem_norm`). ة and ه are deliberately NOT unified, as in grading.
pub fn ar_fold(s: &str) -> String {
    crate::platform_bank::stem_norm(s)
}

/// Registers the SQL scalar functions the platform queries use. Idempotent (a second call replaces the first).
///
/// * `ar_fold(text)` — [`ar_fold`] in SQL; `NULL` stays `NULL`, a number is folded as its text, a blob is `NULL`.
pub fn register_sql_functions(conn: &Connection) -> Result<(), String> {
    use rusqlite::functions::FunctionFlags;
    use rusqlite::types::ValueRef;
    conn.create_scalar_function("ar_fold", 1, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, |ctx| {
        Ok(match ctx.get_raw(0) {
            ValueRef::Text(bytes) => Some(ar_fold(&String::from_utf8_lossy(bytes))),
            ValueRef::Integer(i) => Some(i.to_string()),
            ValueRef::Real(f) => Some(f.to_string()),
            ValueRef::Null | ValueRef::Blob(_) => None,
        })
    })
    .map_err(|e| e.to_string())
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
    let per_ip = state.platform.limits.register_per_ip;
    if per_ip > 0 {
        let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
        rate_limit(&conn, &format!("reg:{}", client_ip(&headers)), REGISTER_WINDOW_MS, per_ip)?;
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
    let ip = client_ip(&headers);
    let (hold, found): (LoginHold, Option<(User, String)>) = {
        let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
        // only failures count: the attempt takes a place now and gives it back below if the password is right
        let hold = hold_login_attempt(&conn, &state.platform.limits, &ip, &email, now_ms())?;
        let found = conn.query_row(
            &format!("SELECT {USER_COLS}, password_hash FROM users WHERE email = ?1"),
            params![email],
            |r| Ok((row_to_user(r)?, r.get::<_, String>(7)?)),
        )
        .optional();
        match found {
            Ok(f) => (hold, f),
            Err(_) => {
                // our failure, not the caller's: nothing is counted
                hold.release(&conn);
                return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"));
            }
        }
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
        // a wrong password or an unknown e-mail: the places taken above stay counted
        _ => return Err(err(StatusCode::UNAUTHORIZED, "invalid_credentials")),
    };

    let conn = state.platform.conn.lock().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"))?;
    hold.release(&conn);
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
            consent: Some(true),
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
    fn registration_requires_recorded_consent() {
        let conn = db();
        for consent in [None, Some(false)] {
            let mut r = req("c@x.com", "student");
            r.consent = consent;
            assert_eq!(insert_user(&conn, &r, "h").unwrap_err().1.contains("consent_required"), true, "{consent:?}");
        }
        let n: i64 = conn.query_row("SELECT count(*) FROM users", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "no account is created without consent");
        let before = now_ms();
        insert_user(&conn, &req("c@x.com", "student"), "h").unwrap();
        let at: i64 = conn.query_row("SELECT consented_at FROM users WHERE email = 'c@x.com'", [], |r| r.get(0)).unwrap();
        assert!(at >= before && at <= now_ms(), "consent timestamp recorded");
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
        let state = PlatformState::for_tests(db());
        assert!(bootstrap_admin(&state, "Root@x.com", "password123").unwrap());
        assert!(!bootstrap_admin(&state, "other@x.com", "password123").unwrap());
        assert!(bootstrap_admin(&PlatformState::for_tests(db()), "r@x.com", "short").is_err());
    }

    // ───────── phase 4-1: sign-in / registration limits ─────────

    /// The start of a 10-minute window, so a test never straddles two.
    const T0: i64 = 1_800_000_000_000;

    fn hold(conn: &Connection, l: &AuthLimits, ip: &str, email: &str) -> Result<LoginHold, Err> {
        hold_login_attempt(conn, l, ip, email, T0)
    }

    fn fail(conn: &Connection, l: &AuthLimits, ip: &str, email: &str) {
        hold(conn, l, ip, email).expect("a failure below the limit is let through"); // kept: it failed
    }

    fn limited(r: Result<LoginHold, Err>) -> bool {
        matches!(r, Err((StatusCode::TOO_MANY_REQUESTS, ref m)) if m.contains("rate_limited"))
    }

    #[test]
    fn limits_come_from_the_environment_with_safe_defaults_and_zero_means_off() {
        assert_eq!(AuthLimits::parse(|_| None), AuthLimits { login_fails_per_ip: 30, login_fails_per_email_ip: 10, login_fails_per_email: 50, register_per_ip: 60 });
        assert_eq!(AuthLimits::default(), AuthLimits::parse(|_| None));
        let env = |k: &str| -> Option<String> {
            match k {
                "PLATFORM_LOGIN_FAILS_PER_10MIN_PER_IP" => Some(" 5 ".into()),
                "PLATFORM_LOGIN_FAILS_PER_10MIN_PER_EMAIL_IP" => Some("0".into()),
                "PLATFORM_LOGIN_FAILS_PER_10MIN_PER_EMAIL" => Some("-3".into()),
                "PLATFORM_REGISTER_PER_HOUR_PER_IP" => Some("lots".into()),
                _ => None,
            }
        };
        assert_eq!(
            AuthLimits::parse(env),
            AuthLimits { login_fails_per_ip: 5, login_fails_per_email_ip: 0, login_fails_per_email: 50, register_per_ip: 60 },
            "unparseable or negative keeps the default"
        );
        assert!(AuthLimits::parse(env).describe().contains("off"));
    }

    #[test]
    fn a_successful_sign_in_never_uses_up_a_counter_and_a_failure_does() {
        let conn = db();
        let l = AuthLimits { login_fails_per_ip: 2, login_fails_per_email_ip: 2, login_fails_per_email: 2, register_per_ip: 1 };
        for _ in 0..25 {
            hold(&conn, &l, "1.1.1.1", "a@x.com").unwrap().release(&conn);
        }
        fail(&conn, &l, "1.1.1.1", "a@x.com");
        fail(&conn, &l, "1.1.1.1", "a@x.com");
        assert!(limited(hold(&conn, &l, "1.1.1.1", "a@x.com")), "the third failure in the window is refused");
        // a refused request takes nothing: the counters stay exactly full
        let counts: Vec<i64> = conn.prepare("SELECT count FROM rate_limits ORDER BY key").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(counts, vec![2, 2, 2]);
        // the next window starts afresh
        assert!(hold_login_attempt(&conn, &l, "1.1.1.1", "a@x.com", T0 + LOGIN_WINDOW_MS).is_ok());
    }

    #[test]
    fn the_email_counter_is_kept_per_address_so_a_stranger_cannot_lock_a_student_out() {
        let conn = db();
        let l = AuthLimits::default();
        for _ in 0..10 {
            fail(&conn, &l, "203.0.113.9", "student@x.com");
        }
        assert!(limited(hold(&conn, &l, "203.0.113.9", "student@x.com")), "the stranger is stopped for that e-mail");
        let at_school = hold(&conn, &l, "198.51.100.7", "student@x.com").expect("the student, from the school's address, still gets in");
        at_school.release(&conn);
        // the stranger's address is not blocked for other e-mails until its own (looser) counter is full
        hold(&conn, &l, "203.0.113.9", "someone@x.com").unwrap().release(&conn);
    }

    #[test]
    fn the_per_address_and_the_cross_address_caps_still_hold() {
        let conn = db();
        let l = AuthLimits::default();
        // one address guessing many e-mails: 30 failures
        for i in 0..30 {
            fail(&conn, &l, "192.0.2.1", &format!("u{i}@x.com"));
        }
        assert!(limited(hold(&conn, &l, "192.0.2.1", "fresh@x.com")));
        // one e-mail guessed from many addresses: 50 failures, then even a new address is refused
        for i in 0..50 {
            fail(&conn, &l, &format!("10.0.{}.{}", i / 200, i % 200), "victim@x.com");
        }
        assert!(limited(hold(&conn, &l, "10.9.9.9", "victim@x.com")));
        assert!(hold(&conn, &l, "10.9.9.9", "else@x.com").is_ok());
    }

    #[test]
    fn a_zero_limit_switches_that_counter_off() {
        let conn = db();
        let off = AuthLimits { login_fails_per_ip: 0, login_fails_per_email_ip: 0, login_fails_per_email: 0, register_per_ip: 0 };
        for _ in 0..200 {
            fail(&conn, &off, "1.1.1.1", "a@x.com");
        }
        let rows: i64 = conn.query_row("SELECT count(*) FROM rate_limits", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0, "nothing is even counted");
        let only_email_ip = AuthLimits { login_fails_per_email_ip: 3, ..off };
        for _ in 0..3 {
            fail(&conn, &only_email_ip, "1.1.1.1", "a@x.com");
        }
        assert!(limited(hold(&conn, &only_email_ip, "1.1.1.1", "a@x.com")));
        assert!(hold(&conn, &only_email_ip, "1.1.1.2", "a@x.com").is_ok());
    }

    #[test]
    fn the_typed_email_is_never_stored_in_the_counters() {
        let conn = db();
        fail(&conn, &AuthLimits::default(), "1.1.1.1", "my-secret-password");
        let keys: Vec<String> = conn.prepare("SELECT key FROM rate_limits").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.iter().all(|k| !k.contains("my-secret-password")), "{keys:?}");
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request};
    use axum::routing::post;
    use axum::Router;
    use tower::ServiceExt;

    fn make_app(limits: AuthLimits) -> (Router, Arc<AppState>) {
        static CFG: std::sync::Once = std::sync::Once::new();
        CFG.call_once(|| std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join(format!("exameow-http-test-{}", new_id()))));
        let mut platform = PlatformState::for_tests(create_test_db());
        platform.limits = limits;
        let state = Arc::new(AppState {
            config_store: exameow_core::config::ConfigStore::new("ExameowServerHttpTest").expect("a scratch config dir"),
            relay: crate::relay::init_db(":memory:").unwrap(),
            platform,
            admin_token: std::sync::Mutex::new("pass".into()),
            legacy: crate::legacy_guard::LegacyGuard::new(60, 2000),
            auth_failures: crate::token_limit::TokenLimiter::new(600),
        });
        let router = Router::new()
            .route("/api/platform/login", post(login_handler))
            .route("/api/platform/register", post(register_handler))
            .with_state(state.clone());
        (router, state)
    }

    async fn send(app: &Router, uri: &str, ip: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(crate::client_ip::HEADER, ip)
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn only_failed_sign_ins_count_and_another_address_is_not_locked_out() {
        let (app, state) = make_app(AuthLimits { login_fails_per_ip: 0, login_fails_per_email_ip: 2, login_fails_per_email: 0, register_per_ip: 0 });
        {
            let conn = state.platform.conn.lock().unwrap();
            let hash = hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO users(id,email,password_hash,full_name,role,status,created_at) VALUES ('u1','s@x.com',?1,'S','student','active',0)",
                params![hash],
            )
            .unwrap();
        }
        let good = serde_json::json!({"email": "S@x.com", "password": "password123"});
        let wrong = serde_json::json!({"email": "s@x.com", "password": "wrong-password"});
        for _ in 0..3 {
            let (st, v) = send(&app, "/api/platform/login", "1.1.1.1", good.clone()).await;
            assert_eq!(st, StatusCode::OK, "{v}");
            assert!(v["token"].is_string());
        }
        for _ in 0..2 {
            assert_eq!(send(&app, "/api/platform/login", "1.1.1.1", wrong.clone()).await.0, StatusCode::UNAUTHORIZED);
        }
        let (st, v) = send(&app, "/api/platform/login", "1.1.1.1", good.clone()).await;
        assert_eq!((st, v["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")), "that address used up its failures for this e-mail");
        let (st, _) = send(&app, "/api/platform/login", "2.2.2.2", good.clone()).await;
        assert_eq!(st, StatusCode::OK, "the student from another address is not locked out");
        // an unknown e-mail is a failure too
        let ghost = serde_json::json!({"email": "ghost@x.com", "password": "whatever-123"});
        for _ in 0..2 {
            assert_eq!(send(&app, "/api/platform/login", "3.3.3.3", ghost.clone()).await.0, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(send(&app, "/api/platform/login", "3.3.3.3", ghost).await.0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn registrations_per_address_follow_the_configured_limit() {
        let reg = |email: &str| serde_json::json!({"email": email, "password": "password123", "full_name": "طالب", "role": "student", "consent": true});
        let (app, _) = make_app(AuthLimits { register_per_ip: 1, ..AuthLimits::default() });
        assert_eq!(send(&app, "/api/platform/register", "1.1.1.1", reg("a@x.com")).await.0, StatusCode::CREATED);
        let (st, v) = send(&app, "/api/platform/register", "1.1.1.1", reg("b@x.com")).await;
        assert_eq!((st, v["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
        assert_eq!(send(&app, "/api/platform/register", "1.1.1.2", reg("b@x.com")).await.0, StatusCode::CREATED, "per address");
        let (app, _) = make_app(AuthLimits { register_per_ip: 0, ..AuthLimits::default() });
        for email in ["c@x.com", "d@x.com"] {
            assert_eq!(send(&app, "/api/platform/register", "1.1.1.1", reg(email)).await.0, StatusCode::CREATED, "0 = off");
        }
    }
}

/// Phase 4-2: the Arabic folding that searches use, as a SQL function on every platform connection.
#[cfg(test)]
mod ar_fold_tests {
    use super::*;

    fn fold_in_sql(conn: &Connection, text: &str) -> Option<String> {
        conn.query_row("SELECT ar_fold(?1)", params![text], |r| r.get(0)).unwrap()
    }

    #[test]
    fn it_is_the_same_folding_the_answer_checker_and_the_question_bank_use() {
        for s in ["أَحْمَد", "إسلام", "آمن", "ٱلكتاب", "مصطفى", "مـــد", "  كلمة   في\tسطر\n", "ABC def", "ÀÉ", "", "   ", "١٢٣", "نص مُشَكَّل جِدًّا", "ة ه"] {
            assert_eq!(ar_fold(s), crate::platform_bank::stem_norm(s), "{s:?}");
            assert_eq!(ar_fold(s), crate::platform_exams::norm_text(s).split_whitespace().collect::<Vec<_>>().join(" "), "{s:?}");
        }
        assert_eq!(ar_fold("أَحْمَد  ـ الإبراهيمي"), "احمد الابراهيمي");
        assert_eq!(ar_fold("مصطفى"), "مصطفي", "ى = ي");
        assert_eq!(ar_fold("مدرسة"), "مدرسة", "ة is not folded into ه (as in grading)");
        assert_ne!(ar_fold("مدرسة"), ar_fold("مدرسه"));
        assert_eq!(ar_fold("HeLLo"), "hello");
    }

    #[test]
    fn it_is_registered_on_a_test_database() {
        let conn = create_test_db();
        assert_eq!(fold_in_sql(&conn, "أَحْمَد  ـ الإبراهيمي").as_deref(), Some("احمد الابراهيمي"));
        // NULL stays NULL, numbers fold as their text, a blob is NULL
        assert_eq!(fold_in_sql(&conn, "x").as_deref(), Some("x"));
        let null: Option<String> = conn.query_row("SELECT ar_fold(NULL)", [], |r| r.get(0)).unwrap();
        assert_eq!(null, None);
        let n: Option<String> = conn.query_row("SELECT ar_fold(42)", [], |r| r.get(0)).unwrap();
        assert_eq!(n.as_deref(), Some("42"));
        let blob: Option<String> = conn.query_row("SELECT ar_fold(x'0102')", [], |r| r.get(0)).unwrap();
        assert_eq!(blob, None);
        // it works inside a query over a table, with LIKE and ESCAPE like the searches use it
        conn.execute_batch("INSERT INTO institutions(id,type,name_ar,is_active,created_at) VALUES ('i1','university','جامعة الإمام',1,0);").unwrap();
        let found: i64 = conn.query_row("SELECT count(*) FROM institutions WHERE ar_fold(name_ar) LIKE ?1 ESCAPE '\\'", params![fold_like("الامام")], |r| r.get(0)).unwrap();
        assert_eq!(found, 1);
    }

    #[test]
    fn it_is_registered_on_the_production_open_path_too() {
        let root = std::env::temp_dir().join(format!("exameow-arfold-{}", new_id()));
        let layout = crate::platform_backup::Layout { db_path: root.join("platform.db"), files_dir: root.join("files"), key_file: root.join("platform.key"), backup_dir: root.join("backups") };
        std::fs::create_dir_all(&root).unwrap();
        let state = init_db(&layout).expect("the production open path");
        {
            let conn = state.conn.lock().unwrap();
            assert_eq!(fold_in_sql(&conn, "إحصاء").as_deref(), Some("احصاء"));
        }
        // the file works again after a restart (a new connection to the same file registers it again)
        drop(state);
        let again = init_db(&layout).unwrap();
        assert_eq!(fold_in_sql(&again.conn.lock().unwrap(), "آمنة").as_deref(), Some("امنة"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn registering_twice_is_harmless_and_a_plain_connection_does_not_have_it() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(conn.query_row("SELECT ar_fold('x')", [], |r| r.get::<_, String>(0)).is_err(), "a bare connection has no such function: every opener must go through apply_schema");
        register_sql_functions(&conn).unwrap();
        register_sql_functions(&conn).unwrap();
        assert_eq!(fold_in_sql(&conn, "أ").as_deref(), Some("ا"));
        apply_schema(&conn).unwrap();
        apply_schema(&conn).unwrap();
        assert_eq!(fold_in_sql(&conn, "ى").as_deref(), Some("ي"));
    }

    #[test]
    fn the_like_pattern_is_folded_and_wildcards_stay_literal() {
        assert_eq!(fold_like("أَحمد"), "%احمد%");
        assert_eq!(fold_like("100%"), "%100\\%%");
        assert_eq!(fold_like("a_b"), "%a\\_b%");
        assert_eq!(fold_like("a\\b"), "%a\\\\b%");
        assert_eq!(fold_like("  Two   Words "), "%two words%");
    }
}
