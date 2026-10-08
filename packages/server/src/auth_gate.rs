//! Deny by default: every `/api/platform/*` request except a short public list must carry a valid session
//! *before* any handler (or request-body extractor) runs. Handlers still enforce their own role and ownership
//! rules; this is the safety net for the day someone adds a route and forgets a guard — and it makes
//! "unauthenticated requests get 401" true regardless of what the body looks like.

use crate::platform::{bearer, user_for_token};
use crate::routes::AppState;
use axum::{
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// Reachable without a session: sign-up, sign-in/out, and the public identity/legal pages.
pub fn is_public(path: &str) -> bool {
    matches!(path, "/api/platform/register" | "/api/platform/login" | "/api/platform/logout") || path.starts_with("/api/platform/public/")
}

/// Whether the gate applies to this request at all (other APIs, static files and CORS preflights pass through).
pub fn guarded(method: &Method, path: &str) -> bool {
    method != Method::OPTIONS && path.starts_with("/api/platform/") && !is_public(path)
}

pub async fn middleware(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if guarded(req.method(), req.uri().path()) {
        let valid = match bearer(req.headers()) {
            Some(token) => state.platform.conn.lock().map(|conn| user_for_token(&conn, token).is_some()).unwrap_or(false),
            None => false,
        };
        if !valid {
            // A token the server does not know costs a database lookup each time, and a flood can vary the token to
            // dodge the per-token budget — so those are also counted per client address (valid tokens never are).
            if bearer(req.headers()).is_some() {
                let who = crate::relay::client_ip(req.headers());
                if let Err(wait_ms) = state.auth_failures.check(&who, crate::relay::now_ms()) {
                    let secs = ((wait_ms + 999) / 1000).max(1).to_string();
                    return (StatusCode::TOO_MANY_REQUESTS, [(header::CONTENT_TYPE, "application/json".to_string()), (header::RETRY_AFTER, secs)], r#"{"error":"rate_limited"}"#).into_response();
                }
            }
            return (StatusCode::UNAUTHORIZED, [(header::CONTENT_TYPE, "application/json")], r#"{"error":"unauthorized"}"#).into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_listed_platform_paths_are_public() {
        for p in ["/api/platform/register", "/api/platform/login", "/api/platform/logout", "/api/platform/public/config", "/api/platform/public/logo", "/api/platform/public/legal/privacy"] {
            assert!(is_public(p), "{p}");
        }
        for p in [
            "/api/platform/me", "/api/platform/admin/users", "/api/platform/registers", "/api/platform/register/x", "/api/platform/login/x",
            "/api/platform/publicity", "/api/platform/public", "/api/platform/admin/public/config", "/api/platform/files/x", "/api/platform/",
        ] {
            assert!(!is_public(p), "{p}");
        }
    }

    #[test]
    fn the_gate_covers_platform_paths_only_and_lets_preflights_through() {
        assert!(guarded(&Method::GET, "/api/platform/me"));
        assert!(guarded(&Method::POST, "/api/platform/admin/backup"));
        assert!(guarded(&Method::DELETE, "/api/platform/me"));
        assert!(!guarded(&Method::OPTIONS, "/api/platform/me"), "CORS preflights carry no credentials");
        assert!(!guarded(&Method::POST, "/api/platform/login"));
        assert!(!guarded(&Method::GET, "/api/exam/code/ABC"), "the anonymous exam relay is not part of the platform");
        assert!(!guarded(&Method::GET, "/assets/app.js"));
        assert!(!guarded(&Method::GET, "/api/platformx/me"), "prefix must include the trailing slash");
    }
}
