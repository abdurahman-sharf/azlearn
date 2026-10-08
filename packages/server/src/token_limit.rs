//! A coarse, in-memory safety net for signed-in API traffic: at most N requests per minute per session token.
//!
//! The platform serves every request through one SQLite connection, so a runaway client (a buggy loop, a script
//! using a stolen or its own token) could starve everyone else. Normal use sits far below the limit — the busiest
//! legitimate client, a student sitting an exam, makes roughly 15 requests a minute — so this never gets in the
//! way, and costs no database writes. Per-endpoint limits (login, export, backups, …) stay in `platform.rs`.

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

const WINDOW_MS: i64 = 60_000;
const MAX_TRACKED: usize = 50_000;
pub const DEFAULT_PER_MINUTE: u32 = 600;

pub struct TokenLimiter {
    per_minute: u32,
    window_ms: i64,
    /// token hash → (window start, requests in the window)
    windows: Mutex<HashMap<u64, (i64, u32)>>,
}

impl TokenLimiter {
    pub fn new(per_minute: u32) -> Self {
        Self::with_window(per_minute, WINDOW_MS)
    }

    /// `max` requests per `window_ms` per key (the name `per_minute` in the struct is the budget, whatever the window).
    pub fn with_window(max: u32, window_ms: i64) -> Self {
        TokenLimiter { per_minute: max, window_ms, windows: Mutex::new(HashMap::new()) }
    }

    pub fn from_env() -> Self {
        let n = std::env::var("PLATFORM_TOKEN_RPM").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(DEFAULT_PER_MINUTE);
        Self::new(n)
    }

    pub fn per_minute(&self) -> u32 {
        self.per_minute
    }

    /// Counts one request; `Err(retry_after_ms)` once the token is over its budget for the current window.
    pub fn check(&self, token: &str, now: i64) -> Result<(), i64> {
        if self.per_minute == 0 {
            return Ok(()); // switched off
        }
        let mut h = DefaultHasher::new();
        token.hash(&mut h);
        let key = h.finish();
        let Ok(mut map) = self.windows.lock() else { return Ok(()) };
        if map.len() > MAX_TRACKED {
            let w = self.window_ms;
            map.retain(|_, (start, _)| now - *start < w);
        }
        let entry = map.entry(key).or_insert((now, 0));
        if now - entry.0 >= self.window_ms {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > self.per_minute {
            Err((entry.0 + self.window_ms - now).max(1000))
        } else {
            Ok(())
        }
    }
}

fn bearer(req: &Request) -> Option<&str> {
    req.headers().get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim).filter(|t| !t.is_empty())
}

pub async fn middleware(State(limiter): State<Arc<TokenLimiter>>, req: Request, next: Next) -> Response {
    if req.uri().path().starts_with("/api/platform/") {
        if let Some(token) = bearer(&req) {
            if let Err(wait_ms) = limiter.check(token, crate::relay::now_ms()) {
                let mut res = (StatusCode::TOO_MANY_REQUESTS, [(header::CONTENT_TYPE, "application/json")], r#"{"error":"rate_limited"}"#).into_response();
                if let Ok(v) = HeaderValue::from_str(&(wait_ms / 1000 + 1).to_string()) {
                    res.headers_mut().insert(header::RETRY_AFTER, v);
                }
                return res;
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_gets_its_budget_per_minute_and_then_waits() {
        let l = TokenLimiter::new(3);
        for i in 0..3 {
            assert!(l.check("tok", 1000 + i).is_ok());
        }
        let wait = l.check("tok", 1500).unwrap_err();
        assert!((1000..=60_000).contains(&wait), "{wait}");
        assert!(l.check("tok", 30_000).is_err(), "still the same window");
        assert!(l.check("tok", 1000 + 59_999).is_err(), "one ms before the window ends");
        assert!(l.check("tok", 1000 + 60_000).is_ok(), "a fresh window");
    }

    #[test]
    fn tokens_are_independent() {
        let l = TokenLimiter::new(1);
        assert!(l.check("a", 0).is_ok());
        assert!(l.check("a", 1).is_err());
        assert!(l.check("b", 1).is_ok(), "someone else's budget is untouched");
    }

    #[test]
    fn zero_switches_it_off() {
        let l = TokenLimiter::new(0);
        for i in 0..10_000 {
            assert!(l.check("x", i).is_ok());
        }
    }

    #[test]
    fn expired_windows_are_dropped_so_memory_stays_bounded() {
        let l = TokenLimiter::new(5);
        for i in 0..(MAX_TRACKED as i64 + 10) {
            let _ = l.check(&format!("t{i}"), 0);
        }
        assert!(l.windows.lock().unwrap().len() > MAX_TRACKED);
        let _ = l.check("late", 2 * WINDOW_MS);
        assert!(l.windows.lock().unwrap().len() < 10, "everything older than a window was swept");
    }

    #[test]
    fn the_retry_hint_shrinks_as_the_window_runs_out() {
        let l = TokenLimiter::new(1);
        l.check("t", 0).unwrap();
        let early = l.check("t", 1_000).unwrap_err();
        let late = l.check("t", 50_000).unwrap_err();
        assert!(early > late && late >= 1000, "{early} {late}");
    }
}
