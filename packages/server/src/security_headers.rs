//! Response headers that harden the browser side of the platform:
//!
//! * `Content-Security-Policy` — no inline or third-party script execution (the main XSS lever), plugins off,
//!   framing only by ourselves, embeds only from the two video hosts the platform allows. Scripts may come from
//!   this origin and from the pinned `onnxruntime-web` path on jsDelivr that the OCR feature loads; styles allow
//!   inline (the UI uses style bindings); `connect-src` stays open because the generator calls the user's own AI
//!   endpoint straight from the browser. Switch with `PLATFORM_CSP=enforce` (default) | `report-only` | `off`.
//! * `Cache-Control: no-store` on the signed-in API, so answers, grades and personal data are never kept by a
//!   shared cache or served again from the back button.
//! * `Permissions-Policy` that turns off powerful features nobody needs here.
//! * `Strict-Transport-Security` only when `PLATFORM_HSTS=1` (it is meaningless over plain HTTP; usually the TLS
//!   proxy sets it).

use axum::{
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

pub const CSP: &str = "default-src 'self'; \
script-src 'self' 'wasm-unsafe-eval' blob: https://cdn.jsdelivr.net/npm/onnxruntime-web@1.26.0/dist/ https://cdn.jsdelivr.net/npm/onnxruntime-web@1.27.0/dist/; \
style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
font-src 'self' data: https://fonts.gstatic.com; \
img-src 'self' data: blob: https:; \
media-src 'self' blob: data: mediastream:; \
connect-src 'self' https: http: ws: wss: blob: data:; \
worker-src 'self' blob:; \
frame-src https://www.youtube-nocookie.com https://player.vimeo.com; \
manifest-src 'self'; \
object-src 'none'; \
base-uri 'self'; \
form-action 'self'; \
frame-ancestors 'self'";

const PERMISSIONS: &str = "geolocation=(), payment=(), usb=(), serial=(), bluetooth=(), interest-cohort=()";
const HSTS: &str = "max-age=31536000; includeSubDomains";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CspMode {
    Enforce,
    ReportOnly,
    Off,
}

impl CspMode {
    /// Anything unrecognised enforces: a typo must never silently weaken the policy.
    pub fn parse(v: Option<&str>) -> Self {
        match v.map(|s| s.trim().to_lowercase()).as_deref() {
            Some("off" | "none" | "false" | "disabled" | "0") => CspMode::Off,
            Some("report-only" | "report_only" | "reportonly" | "report") => CspMode::ReportOnly,
            _ => CspMode::Enforce,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub csp: CspMode,
    pub hsts: bool,
}

impl Config {
    pub fn from_env() -> Arc<Self> {
        let hsts = matches!(std::env::var("PLATFORM_HSTS").ok().as_deref().map(str::trim), Some("1" | "true" | "on" | "yes"));
        Arc::new(Config { csp: CspMode::parse(std::env::var("PLATFORM_CSP").ok().as_deref()), hsts })
    }
}

/// Adds the headers a response should carry. Pure so it can be tested without a server.
pub fn apply(cfg: &Config, path: &str, headers: &mut axum::http::HeaderMap) {
    let api = path.starts_with("/api/");
    if !api {
        let name = match cfg.csp {
            CspMode::Enforce => Some(header::CONTENT_SECURITY_POLICY),
            CspMode::ReportOnly => Some(HeaderName::from_static("content-security-policy-report-only")),
            CspMode::Off => None,
        };
        if let Some(n) = name {
            headers.entry(n).or_insert(HeaderValue::from_static(CSP));
        }
    }
    if path.starts_with("/api/platform/") && !path.starts_with("/api/platform/public/") {
        headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    }
    headers.entry(HeaderName::from_static("permissions-policy")).or_insert(HeaderValue::from_static(PERMISSIONS));
    if cfg.hsts {
        headers.entry(header::STRICT_TRANSPORT_SECURITY).or_insert(HeaderValue::from_static(HSTS));
    }
}

pub async fn middleware(State(cfg): State<Arc<Config>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    apply(&cfg, &path, res.headers_mut());
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn run(csp: CspMode, hsts: bool, path: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        apply(&Config { csp, hsts }, path, &mut h);
        h
    }

    #[test]
    fn the_policy_blocks_inline_script_plugins_and_foreign_framing() {
        let directives: Vec<&str> = CSP.split(';').map(str::trim).collect();
        let get = |name: &str| directives.iter().find(|d| d.starts_with(name)).copied().unwrap_or_default().to_string();
        let script = get("script-src");
        assert!(!script.contains("'unsafe-inline'") && !script.contains("'unsafe-eval'"), "{script}");
        assert!(script.contains("'self'") && script.contains("onnxruntime-web@1.27.0"), "{script}");
        assert!(!script.contains(" https: ") && !script.ends_with(" https:") && !script.contains("*"), "no wildcard script hosts: {script}");
        assert_eq!(get("object-src"), "object-src 'none'");
        assert_eq!(get("base-uri"), "base-uri 'self'");
        assert_eq!(get("form-action"), "form-action 'self'");
        assert_eq!(get("frame-ancestors"), "frame-ancestors 'self'");
        assert_eq!(get("frame-src"), "frame-src https://www.youtube-nocookie.com https://player.vimeo.com", "only the two embed hosts the server itself produces");
        assert!(get("default-src").starts_with("default-src 'self'"));
        assert!(HeaderValue::from_str(CSP).is_ok(), "must be a valid header value");
    }

    #[test]
    fn modes_map_to_the_right_header_and_unknown_values_enforce() {
        assert!(run(CspMode::Enforce, false, "/").contains_key("content-security-policy"));
        let ro = run(CspMode::ReportOnly, false, "/");
        assert!(ro.contains_key("content-security-policy-report-only") && !ro.contains_key("content-security-policy"));
        let off = run(CspMode::Off, false, "/");
        assert!(!off.contains_key("content-security-policy") && !off.contains_key("content-security-policy-report-only"));
        for (v, want) in [(None, CspMode::Enforce), (Some("enforce"), CspMode::Enforce), (Some(" OFF "), CspMode::Off), (Some("report-only"), CspMode::ReportOnly), (Some("0"), CspMode::Off), (Some("offf"), CspMode::Enforce), (Some(""), CspMode::Enforce)] {
            assert_eq!(CspMode::parse(v), want, "{v:?}");
        }
    }

    #[test]
    fn the_signed_in_api_is_never_cached_but_public_config_and_static_files_are_left_alone() {
        assert_eq!(run(CspMode::Enforce, false, "/api/platform/attempts/x").get("cache-control").unwrap(), "no-store");
        assert_eq!(run(CspMode::Enforce, false, "/api/platform/admin/system").get("cache-control").unwrap(), "no-store");
        assert!(run(CspMode::Enforce, false, "/api/platform/public/config").get("cache-control").is_none(), "public pages keep their own caching");
        assert!(run(CspMode::Enforce, false, "/assets/app.js").get("cache-control").is_none());
        let mut h = HeaderMap::new();
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=5"));
        apply(&Config { csp: CspMode::Enforce, hsts: false }, "/api/platform/me", &mut h);
        assert_eq!(h.get("cache-control").unwrap(), "private, max-age=5", "a handler's own choice wins");
    }

    #[test]
    fn csp_is_for_documents_not_the_json_api() {
        assert!(!run(CspMode::Enforce, false, "/api/platform/me").contains_key("content-security-policy"));
        assert!(!run(CspMode::Enforce, false, "/api/exam/code/ABC").contains_key("content-security-policy"));
        assert!(run(CspMode::Enforce, false, "/assets/index.js").contains_key("content-security-policy"));
    }

    #[test]
    fn hsts_only_when_asked_and_permissions_always() {
        assert!(!run(CspMode::Enforce, false, "/").contains_key("strict-transport-security"));
        assert_eq!(run(CspMode::Enforce, true, "/").get("strict-transport-security").unwrap(), HSTS);
        let p = run(CspMode::Off, false, "/api/platform/me").get("permissions-policy").unwrap().to_str().unwrap().to_string();
        assert!(p.contains("geolocation=()") && p.contains("payment=()"));
        assert!(!p.contains("camera=()"), "the camera-search feature needs the camera");
    }
}
