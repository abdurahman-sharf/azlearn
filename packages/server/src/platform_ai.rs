//! Platform AI: the admin-managed provider configuration (endpoint / model / API key), daily usage
//! caps and the cap-enforced generation endpoint.
//!
//! * The API key is stored encrypted in `settings` (`ai.key`) and is never returned to any client:
//!   responses only say whether one is saved. `AI_*` environment variables remain the fallback when
//!   nothing is saved in the settings page.
//! * Caps count generation *requests* per UTC day, for the whole platform and per admin. A request that
//!   fails at the provider is refunded; one refused by a cap is never sent.

use crate::platform::{audit, bad, db_err, lock, rate_limit, require_admin, Res};
use crate::platform_settings as settings;
use crate::relay::{err, now_ms};
use crate::routes::AppState;
use axum::{
    extract::{DefaultBodyLimit, Multipart, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use exameow_core::ai::AIClient;
use exameow_core::exam::{generate_exam, ExamParams, Question};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS ai_usage (
  day INTEGER NOT NULL,                 -- UTC days since the Unix epoch
  user_id TEXT NOT NULL,                -- no FK: usage history outlives an account
  kind TEXT NOT NULL CHECK (kind IN ('generate','grade')),
  calls INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, user_id, kind)
);
";

const K_ENDPOINT: &str = "ai.endpoint";
const K_MODEL: &str = "ai.model";
const K_KEY: &str = "ai.key";
const K_CAP_PLATFORM: &str = "ai.cap_platform";
const K_CAP_ADMIN: &str = "ai.cap_admin";
const K_TEACHERS_EXAMS: &str = "teachers.can_create_exams";

pub const DEFAULT_CAP_PLATFORM: i64 = 200;
pub const DEFAULT_CAP_ADMIN: i64 = 50;
const MAX_CAP: i64 = 100_000;
const DAY_MS: i64 = 86_400_000;
const MAX_UPLOAD_BYTES: usize = 15 * 1024 * 1024;

pub fn upload_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 512 * 1024)
}

/// The `AI_*` environment fallback (injected so tests do not depend on the process environment).
#[derive(Clone, Default)]
pub struct Env {
    pub endpoint: String,
    pub key: String,
    pub model: String,
}

impl Env {
    pub fn from_process() -> Self {
        let get = |k: &str| std::env::var(k).unwrap_or_default();
        Env { endpoint: get("AI_ENDPOINT"), key: get("AI_API_KEY"), model: get("AI_MODEL") }
    }
}

#[derive(Clone)]
pub struct AiConfig {
    pub endpoint: String,
    pub model: String,
    pub key: String,
    /// "db" (saved in the settings page), "env" (environment variables) or "none".
    pub source: &'static str,
}

fn day_of(now: i64) -> i64 {
    now.div_euclid(DAY_MS)
}

fn int_setting(conn: &Connection, crypto: &settings::Crypto, key: &str, default: i64) -> Res<i64> {
    Ok(settings::get(conn, crypto, key)?.and_then(|v| v.parse().ok()).unwrap_or(default))
}

/// Saved settings win field by field; anything not saved falls back to the environment.
pub fn effective_config(conn: &Connection, crypto: &settings::Crypto, env: &Env) -> Res<AiConfig> {
    let saved_key = settings::get(conn, crypto, K_KEY)?.filter(|k| !k.is_empty());
    let pick = |saved: Option<String>, fallback: &str| saved.filter(|v| !v.is_empty()).unwrap_or_else(|| fallback.to_string());
    let endpoint = pick(settings::get(conn, crypto, K_ENDPOINT)?, &env.endpoint);
    let model = pick(settings::get(conn, crypto, K_MODEL)?, &env.model);
    let source = if saved_key.is_some() {
        "db"
    } else if !env.key.is_empty() {
        "env"
    } else {
        "none"
    };
    let key = saved_key.unwrap_or_else(|| env.key.clone());
    Ok(AiConfig { endpoint, model, key, source })
}

// ───────── settings (read / write) ─────────

#[derive(Serialize, Debug)]
pub struct AiView {
    /// What the admin saved (empty = not saved, the environment applies).
    endpoint: String,
    model: String,
    /// The key itself is never sent; only whether one is saved.
    key_saved: bool,
    source: &'static str,
    effective_endpoint: String,
    effective_model: String,
}

#[derive(Serialize, Debug)]
pub struct SettingsView {
    ai: AiView,
    cap_platform: i64,
    cap_admin: i64,
    teachers_can_create_exams: bool,
}

pub fn read_settings(conn: &Connection, crypto: &settings::Crypto, env: &Env) -> Res<SettingsView> {
    let eff = effective_config(conn, crypto, env)?;
    Ok(SettingsView {
        ai: AiView {
            endpoint: settings::get(conn, crypto, K_ENDPOINT)?.unwrap_or_default(),
            model: settings::get(conn, crypto, K_MODEL)?.unwrap_or_default(),
            key_saved: settings::is_set(conn, K_KEY)?,
            source: eff.source,
            effective_endpoint: eff.endpoint,
            effective_model: eff.model,
        },
        cap_platform: int_setting(conn, crypto, K_CAP_PLATFORM, DEFAULT_CAP_PLATFORM)?,
        cap_admin: int_setting(conn, crypto, K_CAP_ADMIN, DEFAULT_CAP_ADMIN)?,
        teachers_can_create_exams: settings::get(conn, crypto, K_TEACHERS_EXAMS)?.as_deref() == Some("1"),
    })
}

#[derive(Deserialize, Default)]
pub struct SettingsReq {
    /// For string fields `Some("")` clears the saved value (the environment fallback applies again).
    endpoint: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    cap_platform: Option<i64>,
    cap_admin: Option<i64>,
    teachers_can_create_exams: Option<bool>,
}

fn valid_endpoint(raw: &str) -> Res<String> {
    let t = raw.trim();
    if t.chars().count() > 300 {
        return Err(bad("invalid_endpoint"));
    }
    let url = url::Url::parse(t).map_err(|_| bad("invalid_endpoint"))?;
    // http is allowed on purpose: self-hosted gateways (Ollama, vLLM) are usually plain http on a LAN.
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err(bad("invalid_endpoint"));
    }
    Ok(t.trim_end_matches('/').to_string())
}

fn valid_model(raw: &str) -> Res<String> {
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > 100 || t.chars().any(char::is_control) {
        return Err(bad("invalid_model"));
    }
    Ok(t.to_string())
}

fn valid_key(raw: &str) -> Res<String> {
    let t = raw.trim();
    if t.chars().count() < 8 || t.chars().count() > 500 || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad("invalid_api_key"));
    }
    Ok(t.to_string())
}

fn valid_cap(n: i64) -> Res<i64> {
    if (0..=MAX_CAP).contains(&n) {
        Ok(n)
    } else {
        Err(bad("invalid_cap"))
    }
}

/// Change of one saved value: `Some(Some(v))` set, `Some(None)` clear, `None` untouched.
type Change = Option<Option<String>>;

fn text_change(raw: &Option<String>, check: fn(&str) -> Res<String>) -> Res<Change> {
    match raw.as_deref().map(str::trim) {
        None => Ok(None),
        Some("") => Ok(Some(None)),
        Some(v) => Ok(Some(Some(check(v)?))),
    }
}

pub fn apply_settings(conn: &Connection, crypto: &settings::Crypto, admin_id: &str, r: &SettingsReq) -> Res<()> {
    // Validate everything first so a bad field leaves nothing half-applied.
    let endpoint = text_change(&r.endpoint, valid_endpoint)?;
    let model = text_change(&r.model, valid_model)?;
    let key = text_change(&r.api_key, valid_key)?;
    let cap_platform = r.cap_platform.map(valid_cap).transpose()?;
    let cap_admin = r.cap_admin.map(valid_cap).transpose()?;

    conn.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
    let result = (|| -> Res<()> {
        let mut changed: Vec<&str> = vec![];
        for (k, change, secret) in [(K_ENDPOINT, endpoint, false), (K_MODEL, model, false), (K_KEY, key, true)] {
            match change {
                Some(Some(v)) => settings::set(conn, crypto, k, &v, secret, Some(admin_id))?,
                Some(None) => settings::delete(conn, k)?,
                None => continue,
            }
            changed.push(k);
        }
        for (k, n) in [(K_CAP_PLATFORM, cap_platform), (K_CAP_ADMIN, cap_admin)] {
            if let Some(n) = n {
                settings::set(conn, crypto, k, &n.to_string(), false, Some(admin_id))?;
                changed.push(k);
            }
        }
        if let Some(b) = r.teachers_can_create_exams {
            settings::set(conn, crypto, K_TEACHERS_EXAMS, if b { "1" } else { "0" }, false, Some(admin_id))?;
            changed.push(K_TEACHERS_EXAMS);
        }
        // Names only: the audit log must never contain a secret.
        for k in changed {
            audit(conn, admin_id, k, "setting_changed", k);
        }
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT").map_err(db_err),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

// ───────── caps & usage ─────────

fn cap_error(code: &str, now: i64) -> crate::relay::Err {
    (
        StatusCode::TOO_MANY_REQUESTS,
        serde_json::json!({ "error": code, "resets_at": (day_of(now) + 1) * DAY_MS }).to_string(),
    )
}

fn calls_today(conn: &Connection, day: i64, kind: &str, user: Option<&str>) -> Res<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(calls), 0) FROM ai_usage WHERE day = ?1 AND kind = ?2 AND (?3 IS NULL OR user_id = ?3)",
        params![day, kind, user],
        |r| r.get(0),
    )
    .map_err(db_err)
}

/// Reserves one call against both caps, or fails with 429 (code + reset time). Check and increment
/// happen under the connection mutex the handlers hold, so concurrent requests cannot overshoot.
pub fn consume(conn: &Connection, crypto: &settings::Crypto, user_id: &str, kind: &str, now: i64) -> Res<()> {
    let day = day_of(now);
    let cap_platform = int_setting(conn, crypto, K_CAP_PLATFORM, DEFAULT_CAP_PLATFORM)?;
    let cap_admin = int_setting(conn, crypto, K_CAP_ADMIN, DEFAULT_CAP_ADMIN)?;
    if calls_today(conn, day, kind, None)? >= cap_platform {
        return Err(cap_error("ai_cap_platform", now));
    }
    if calls_today(conn, day, kind, Some(user_id))? >= cap_admin {
        return Err(cap_error("ai_cap_admin", now));
    }
    conn.execute(
        "INSERT INTO ai_usage(day, user_id, kind, calls) VALUES (?1, ?2, ?3, 1)
         ON CONFLICT(day, user_id, kind) DO UPDATE SET calls = calls + 1",
        params![day, user_id, kind],
    )
    .map_err(db_err)?;
    Ok(())
}

/// Gives a reserved call back (the provider failed, so the admin should not lose quota).
pub fn release(conn: &Connection, user_id: &str, kind: &str, now: i64) {
    let _ = conn.execute(
        "UPDATE ai_usage SET calls = calls - 1 WHERE day = ?1 AND user_id = ?2 AND kind = ?3 AND calls > 0",
        params![day_of(now), user_id, kind],
    );
}

#[derive(Serialize, Debug)]
pub struct UsageDay {
    day: i64,
    calls: i64,
}
#[derive(Serialize, Debug)]
pub struct UsageAdmin {
    user_id: String,
    name: String,
    calls: i64,
}
#[derive(Serialize, Debug)]
pub struct Usage {
    today_platform: i64,
    today_mine: i64,
    cap_platform: i64,
    cap_admin: i64,
    resets_at: i64,
    /// Newest first, last 14 days that have any usage.
    days: Vec<UsageDay>,
    admins_today: Vec<UsageAdmin>,
}

pub fn usage(conn: &Connection, crypto: &settings::Crypto, me: &str, now: i64) -> Res<Usage> {
    let day = day_of(now);
    let days = conn
        .prepare("SELECT day, SUM(calls) FROM ai_usage WHERE kind = 'generate' AND day > ?1 GROUP BY day HAVING SUM(calls) > 0 ORDER BY day DESC")
        .map_err(db_err)?
        .query_map(params![day - 14], |r| Ok(UsageDay { day: r.get(0)?, calls: r.get(1)? }))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let admins_today = conn
        .prepare(
            "SELECT u.user_id, COALESCE(n.full_name, ''), u.calls FROM ai_usage u LEFT JOIN users n ON n.id = u.user_id
             WHERE u.day = ?1 AND u.kind = 'generate' AND u.calls > 0 ORDER BY u.calls DESC, u.user_id LIMIT 50",
        )
        .map_err(db_err)?
        .query_map(params![day], |r| Ok(UsageAdmin { user_id: r.get(0)?, name: r.get(1)?, calls: r.get(2)? }))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(Usage {
        today_platform: calls_today(conn, day, "generate", None)?,
        today_mine: calls_today(conn, day, "generate", Some(me))?,
        cap_platform: int_setting(conn, crypto, K_CAP_PLATFORM, DEFAULT_CAP_PLATFORM)?,
        cap_admin: int_setting(conn, crypto, K_CAP_ADMIN, DEFAULT_CAP_ADMIN)?,
        resets_at: (day + 1) * DAY_MS,
        days,
        admins_today,
    })
}

// ───────── handlers ─────────

pub async fn get_settings_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<SettingsView>> {
    require_admin(&s, &h)?;
    read_settings(&*lock(&s)?, &s.platform.crypto, &Env::from_process()).map(Json)
}

pub async fn put_settings_handler(State(s): State<Arc<AppState>>, h: HeaderMap, Json(r): Json<SettingsReq>) -> Res<Json<SettingsView>> {
    let admin = require_admin(&s, &h)?;
    let conn = lock(&s)?;
    apply_settings(&conn, &s.platform.crypto, &admin.id, &r)?;
    read_settings(&conn, &s.platform.crypto, &Env::from_process()).map(Json)
}

pub async fn usage_handler(State(s): State<Arc<AppState>>, h: HeaderMap) -> Res<Json<Usage>> {
    let admin = require_admin(&s, &h)?;
    usage(&*lock(&s)?, &s.platform.crypto, &admin.id, now_ms()).map(Json)
}

#[derive(Deserialize, Default)]
pub struct TestReq {
    /// Optional unsaved values to try; whatever is omitted comes from the effective configuration.
    endpoint: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
}

#[derive(Serialize)]
pub struct TestResult {
    ok: bool,
    ms: u128,
    /// Machine code the UI translates; `detail` is a short provider message with the key removed.
    code: &'static str,
    detail: String,
}

/// Provider errors can echo request details; strip the key and keep it short.
fn scrub(msg: &str, key: &str) -> String {
    let m = if key.is_empty() { msg.to_string() } else { msg.replace(key, "***") };
    m.chars().take(200).collect()
}

pub async fn test_handler(State(s): State<Arc<AppState>>, h: HeaderMap, body: Option<Json<TestReq>>) -> Res<Json<TestResult>> {
    let admin = require_admin(&s, &h)?;
    let req = body.map(|b| b.0).unwrap_or_default();
    let mut cfg = {
        let conn = lock(&s)?;
        rate_limit(&conn, &format!("ai-test:{}", admin.id), 60_000, 10)?;
        effective_config(&conn, &s.platform.crypto, &Env::from_process())?
    };
    if let Some(e) = req.endpoint.as_deref().filter(|v| !v.trim().is_empty()) {
        cfg.endpoint = valid_endpoint(e)?;
    }
    if let Some(k) = req.api_key.as_deref().filter(|v| !v.trim().is_empty()) {
        cfg.key = valid_key(k)?;
    }
    if let Some(m) = req.model.as_deref().filter(|v| !v.trim().is_empty()) {
        cfg.model = valid_model(m)?;
    }
    if cfg.endpoint.is_empty() || cfg.key.is_empty() {
        return Ok(Json(TestResult { ok: false, ms: 0, code: "ai_not_configured", detail: String::new() }));
    }
    let client = AIClient::new(&cfg.endpoint, &cfg.key);
    let started = std::time::Instant::now();
    // Not every gateway exposes /models, so fall back to a 1-token chat when a model is configured.
    let mut outcome = client.fetch_models().await.map(|_| ()).map_err(|e| e.to_string());
    if outcome.is_err() && !cfg.model.is_empty() {
        outcome = client.chat_with_max_tokens("ping", "ping", &cfg.model, Some(1)).await.map(|_| ()).map_err(|e| e.to_string());
    }
    let ms = started.elapsed().as_millis();
    Ok(Json(match outcome {
        Ok(()) => TestResult { ok: true, ms, code: "ai_test_ok", detail: String::new() },
        Err(e) => TestResult { ok: false, ms, code: "ai_test_failed", detail: scrub(&e, &cfg.key) },
    }))
}

#[derive(Serialize)]
pub struct GenerateOut {
    questions: Vec<Question>,
}

/// Cap-enforced generation with the platform's saved provider. Multipart: `params` (ExamParams JSON,
/// optionally with `text`) and, unless `text` is given, a `file` to parse.
pub async fn generate_handler(State(s): State<Arc<AppState>>, h: HeaderMap, mut mp: Multipart) -> Res<Json<GenerateOut>> {
    let admin = require_admin(&s, &h)?;
    let mut file: Option<(String, Vec<u8>)> = None;
    let mut params_json = String::new();
    while let Some(field) = mp.next_field().await.map_err(|_| bad("invalid_upload"))? {
        match field.name() {
            Some("file") => {
                let name = field.file_name().unwrap_or("unknown").to_string();
                let bytes = field.bytes().await.map_err(|_| bad("invalid_file_size"))?;
                if bytes.len() > MAX_UPLOAD_BYTES {
                    return Err(bad("invalid_file_size"));
                }
                file = Some((name, bytes.to_vec()));
            }
            Some("params") => params_json = field.text().await.map_err(|_| bad("invalid_upload"))?,
            _ => {}
        }
    }
    let params: ExamParams = serde_json::from_str(&params_json).map_err(|_| bad("invalid_params"))?;
    let text = match params.text.as_deref() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => {
            let (name, bytes) = file.ok_or_else(|| bad("file_required"))?;
            crate::routes::extract_text(&name, &bytes).map_err(|_| bad("parse_failed"))?
        }
    };

    let now = now_ms();
    // Reserve the call before contacting the provider; nothing is sent if a cap is reached.
    let cfg = {
        let conn = lock(&s)?;
        rate_limit(&conn, &format!("ai-gen:{}", admin.id), 60_000, 30)?;
        let cfg = effective_config(&conn, &s.platform.crypto, &Env::from_process())?;
        if cfg.endpoint.is_empty() || cfg.key.is_empty() {
            return Err(bad("ai_not_configured"));
        }
        consume(&conn, &s.platform.crypto, &admin.id, "generate", now)?;
        cfg
    };
    let client = AIClient::new(&cfg.endpoint, &cfg.key);
    match generate_exam(&client, &text, &params, &cfg.model).await {
        Ok(questions) => Ok(Json(GenerateOut { questions })),
        Err(_) => {
            release(&*lock(&s)?, &admin.id, "generate", now);
            Err(err(StatusCode::BAD_GATEWAY, "ai_failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{create_test_db, insert_test_user};

    fn cx() -> settings::Crypto {
        settings::Crypto::for_tests()
    }
    fn code(e: crate::relay::Err) -> (StatusCode, String) {
        let v: serde_json::Value = serde_json::from_str(&e.1).unwrap();
        (e.0, v["error"].as_str().unwrap().to_string())
    }
    fn req() -> SettingsReq {
        SettingsReq::default()
    }

    #[test]
    fn key_is_encrypted_at_rest_never_in_views_or_audit_and_env_is_the_fallback() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let env = Env { endpoint: "https://env.example/v1".into(), key: "env-key-123456".into(), model: "env-model".into() };

        let v = read_settings(&conn, &cx(), &env).unwrap();
        assert_eq!((v.ai.source, v.ai.key_saved, v.ai.effective_model.as_str()), ("env", false, "env-model"));

        let secret = "sk-SUPER-secret-value-98765";
        apply_settings(&conn, &cx(), &admin.id, &SettingsReq { api_key: Some(secret.into()), endpoint: Some("http://10.0.0.5:11434/v1/".into()), ..req() }).unwrap();

        let v = read_settings(&conn, &cx(), &env).unwrap();
        assert_eq!((v.ai.source, v.ai.key_saved), ("db", true));
        assert_eq!(v.ai.endpoint, "http://10.0.0.5:11434/v1", "trailing slash trimmed, plain http allowed for LAN gateways");
        assert_eq!(v.ai.effective_model, "env-model", "unsaved fields still fall back to the environment");
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains(secret) && !json.contains("env-key"), "the view never contains a key: {json}");

        let stored: (String, i64) = conn.query_row("SELECT value, encrypted FROM settings WHERE key = 'ai.key'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(stored.1, 1);
        assert!(!stored.0.contains(secret), "ciphertext only in the database");
        let audit_dump: String = conn.query_row("SELECT group_concat(action || ' ' || COALESCE(detail,'') || ' ' || COALESCE(target_id,''), ' ') FROM audit_log", [], |r| r.get(0)).unwrap();
        assert!(!audit_dump.contains(secret) && audit_dump.contains("ai.key"), "audit names the setting, not the value");
        assert_eq!(effective_config(&conn, &cx(), &env).unwrap().key, secret);

        // clearing restores the environment fallback
        apply_settings(&conn, &cx(), &admin.id, &SettingsReq { api_key: Some("".into()), ..req() }).unwrap();
        let c = effective_config(&conn, &cx(), &env).unwrap();
        assert_eq!((c.source, c.key.as_str()), ("env", "env-key-123456"));
    }

    #[test]
    fn invalid_values_are_rejected_atomically() {
        let conn = create_test_db();
        let admin = insert_test_user(&conn, "a@x.com", "admin", "active");
        let env = Env::default();
        for bad_req in [
            SettingsReq { endpoint: Some("ftp://x.com".into()), ..req() },
            SettingsReq { endpoint: Some("https://user:pw@x.com".into()), ..req() },
            SettingsReq { endpoint: Some("not a url".into()), ..req() },
            SettingsReq { api_key: Some("short".into()), ..req() },
            SettingsReq { api_key: Some("has space inside key".into()), ..req() },
            SettingsReq { model: Some("bad\nmodel".into()), ..req() },
            SettingsReq { cap_platform: Some(-1), ..req() },
            SettingsReq { cap_admin: Some(100_001), ..req() },
        ] {
            assert_eq!(apply_settings(&conn, &cx(), &admin.id, &bad_req).unwrap_err().0, StatusCode::BAD_REQUEST);
        }
        // a valid field next to an invalid one must not be applied
        let mixed = SettingsReq { model: Some("good-model".into()), cap_admin: Some(-5), ..req() };
        assert!(apply_settings(&conn, &cx(), &admin.id, &mixed).is_err());
        assert_eq!(read_settings(&conn, &cx(), &env).unwrap().ai.model, "", "nothing half-applied");
        assert_eq!(conn.query_row("SELECT count(*) FROM audit_log WHERE action = 'setting_changed'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }

    #[test]
    fn caps_are_enforced_per_admin_and_for_the_platform_with_a_reset_time() {
        let conn = create_test_db();
        let (a, b) = (insert_test_user(&conn, "a@x.com", "admin", "active"), insert_test_user(&conn, "b@x.com", "admin", "active"));
        apply_settings(&conn, &cx(), &a.id, &SettingsReq { cap_admin: Some(2), cap_platform: Some(3), ..req() }).unwrap();
        let now = 5 * DAY_MS + 1234;
        consume(&conn, &cx(), &a.id, "generate", now).unwrap();
        consume(&conn, &cx(), &a.id, "generate", now).unwrap();
        let e = consume(&conn, &cx(), &a.id, "generate", now).unwrap_err();
        assert_eq!((e.0, serde_json::from_str::<serde_json::Value>(&e.1).unwrap()["resets_at"].as_i64().unwrap()), (StatusCode::TOO_MANY_REQUESTS, 6 * DAY_MS));
        assert_eq!(code(consume(&conn, &cx(), &a.id, "generate", now).unwrap_err()).1, "ai_cap_admin");
        consume(&conn, &cx(), &b.id, "generate", now).unwrap(); // platform total now 3
        assert_eq!(code(consume(&conn, &cx(), &b.id, "generate", now).unwrap_err()).1, "ai_cap_platform");
        // a refused call is not counted
        assert_eq!(usage(&conn, &cx(), &a.id, now).unwrap().today_platform, 3);
        // a new UTC day starts fresh
        consume(&conn, &cx(), &a.id, "generate", 6 * DAY_MS).unwrap();
        // cap 0 blocks everything
        apply_settings(&conn, &cx(), &a.id, &SettingsReq { cap_platform: Some(0), ..req() }).unwrap();
        assert_eq!(code(consume(&conn, &cx(), &b.id, "generate", 7 * DAY_MS).unwrap_err()).1, "ai_cap_platform");
    }

    #[test]
    fn release_refunds_and_never_goes_negative_and_usage_reports_per_admin() {
        let conn = create_test_db();
        let a = insert_test_user(&conn, "a@x.com", "admin", "active");
        let now = 3 * DAY_MS;
        consume(&conn, &cx(), &a.id, "generate", now).unwrap();
        release(&conn, &a.id, "generate", now);
        release(&conn, &a.id, "generate", now);
        let u = usage(&conn, &cx(), &a.id, now).unwrap();
        assert_eq!((u.today_platform, u.today_mine, u.cap_admin, u.cap_platform), (0, 0, DEFAULT_CAP_ADMIN, DEFAULT_CAP_PLATFORM));
        assert!(u.days.is_empty() && u.admins_today.is_empty(), "zero-call days are not listed");
        consume(&conn, &cx(), &a.id, "generate", now).unwrap();
        consume(&conn, &cx(), &a.id, "generate", now - DAY_MS).unwrap();
        let u = usage(&conn, &cx(), &a.id, now).unwrap();
        assert_eq!((u.days.len(), u.days[0].day, u.days[0].calls, u.admins_today.len(), u.admins_today[0].calls), (2, 3, 1, 1, 1));
        assert_eq!(u.resets_at, 4 * DAY_MS);
    }

    #[test]
    fn scrub_removes_the_key_from_provider_messages() {
        assert_eq!(scrub("HTTP 401: bad key sk-abc123456 given", "sk-abc123456"), "HTTP 401: bad key *** given");
        assert_eq!(scrub(&"x".repeat(500), "k").chars().count(), 200);
    }
}
