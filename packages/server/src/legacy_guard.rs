//! Guard rails for the original, unauthenticated Exameow API (`/api/generate`, `/api/answer`, `/api/judge`,
//! `/api/explain`, `/api/models`). Those endpoints accept an AI endpoint URL from the caller and make the request
//! *from the server*, and fall back to the server's own `AI_*` key when the caller sends none. On a server that is
//! reachable from the internet that means:
//!
//! * **SSRF** — a stranger can point the server at internal addresses (`http://169.254.169.254/…`, `localhost`
//!   services) and read what comes back;
//! * **key abuse** — a stranger can spend the operator's `AI_API_KEY`, bypassing the platform's caps.
//!
//! The rules (callers on a loopback/private address — someone using their own machine or LAN — are exempt, so
//! personal Docker use with a local model such as Ollama keeps working):
//!
//! 1. only `http`/`https`, and the host must resolve exclusively to public addresses (`localhost` and `*.localhost`
//!    are refused by name; the AI client also refuses to follow redirects to another host);
//! 2. calls that rely on the server's own key are limited per client address (default 60/hour) and in total
//!    (default 2000/day) — `LEGACY_AI_PER_HOUR_PER_IP`, `LEGACY_AI_PER_DAY` (0 = unlimited);
//! 3. `LEGACY_AI_OUTSIDERS=off` closes these endpoints to outside callers altogether (403 `legacy_ai_disabled`).
//!
//! Residual risk (documented in `docs/PLATFORM_ar.md`): a hostile DNS server could answer the check and the real
//! connection differently (rebinding). Operators who cannot accept that use rule 3.

use crate::client_ip::is_local;
use crate::relay::{client_ip, err, now_ms, Err};
use crate::token_limit::TokenLimiter;
use axum::http::{HeaderMap, StatusCode};
use std::net::{IpAddr, SocketAddr};

pub struct LegacyGuard {
    per_ip: TokenLimiter,
    global: TokenLimiter,
    outsiders: bool,
}

/// Anything an outside caller must not be able to make the server connect to.
pub fn is_public_ip(ip: IpAddr) -> bool {
    use std::net::Ipv4Addr;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    if is_local(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || (o[0] == 100 && (64..=127).contains(&o[1])) // carrier-grade NAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
                || (o[0] == 198 && (18..=19).contains(&o[1])) // benchmarking
                || o[0] >= 240 // reserved
                || v4 == Ipv4Addr::new(0, 0, 0, 0))
        }
        IpAddr::V6(v6) => !(v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8)),
    }
}

impl LegacyGuard {
    pub fn from_env() -> Self {
        let n = |k: &str, d: u32| std::env::var(k).ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(d);
        let mut g = Self::new(n("LEGACY_AI_PER_HOUR_PER_IP", 60), n("LEGACY_AI_PER_DAY", 2000));
        g.outsiders = !matches!(std::env::var("LEGACY_AI_OUTSIDERS").unwrap_or_default().trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no");
        g
    }

    pub fn new(per_ip_hour: u32, per_day: u32) -> Self {
        LegacyGuard { per_ip: TokenLimiter::with_window(per_ip_hour, 3_600_000), global: TokenLimiter::with_window(per_day, 86_400_000), outsiders: true }
    }

    #[cfg(test)]
    pub fn with_outsiders(mut self, allowed: bool) -> Self {
        self.outsiders = allowed;
        self
    }

    /// `Ok` when this call may proceed. `custom_endpoint`: the endpoint the caller chose, `None` when the call goes to
    /// the operator's own `AI_ENDPOINT` (trusted even if it is a private address such as a local model container).
    /// `used_server_key`: the caller sent no key of its own, so the server's is used.
    pub async fn check(&self, headers: &HeaderMap, custom_endpoint: Option<&str>, used_server_key: bool) -> Result<(), Err> {
        let client = client_ip(headers).parse::<IpAddr>().ok();
        if client.map_or(false, is_local) {
            return Ok(());
        }
        if !self.outsiders {
            return Err(err(StatusCode::FORBIDDEN, "legacy_ai_disabled"));
        }
        if let Some(endpoint) = custom_endpoint {
            check_endpoint(endpoint).await?;
        }
        if used_server_key {
            let who = client.map_or("unknown".to_string(), |c| c.to_string());
            let now = now_ms();
            if self.per_ip.check(&who, now).is_err() || self.global.check("*", now).is_err() {
                return Err(err(StatusCode::TOO_MANY_REQUESTS, "ai_quota_exceeded"));
            }
        }
        Ok(())
    }
}

/// `http`/`https` to a host whose every address is public.
pub async fn check_endpoint(endpoint: &str) -> Result<(), Err> {
    let denied = || err(StatusCode::BAD_REQUEST, "endpoint_not_allowed");
    let url = url::Url::parse(endpoint.trim()).map_err(|_| denied())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(denied());
    }
    let host = url.host().ok_or_else(denied)?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<IpAddr> = match host {
        url::Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
        url::Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
        url::Host::Domain(d) if d.eq_ignore_ascii_case("localhost") || d.to_ascii_lowercase().ends_with(".localhost") => return Err(denied()),
        url::Host::Domain(d) => match tokio::net::lookup_host((d, port)).await {
            Ok(it) => it.map(|a: SocketAddr| a.ip()).collect(),
            // unresolvable: nothing to protect, the real call fails with the provider error
            Err(_) => return Ok(()),
        },
    };
    if addrs.iter().all(|a| is_public_ip(*a)) {
        Ok(())
    } else {
        Err(denied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn from(ip: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(crate::client_ip::HEADER, HeaderValue::from_str(ip).unwrap());
        h
    }

    #[test]
    fn only_genuinely_public_addresses_pass() {
        for ok in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:4700:4700::1111", "::ffff:8.8.8.8"] {
            assert!(is_public_ip(ok.parse().unwrap()), "{ok}");
        }
        for bad in [
            "127.0.0.1", "10.0.0.1", "172.16.5.5", "192.168.1.1", "169.254.169.254", "0.0.0.0", "100.64.0.1", "100.127.255.255", "198.18.0.1", "192.0.0.5",
            "224.0.0.1", "255.255.255.255", "240.0.0.1", "192.0.2.1", "::1", "fd00::1", "fe80::1", "::", "ff02::1", "2001:db8::1", "::ffff:127.0.0.1", "::ffff:10.1.1.1",
        ] {
            assert!(!is_public_ip(bad.parse().unwrap()), "{bad}");
        }
        assert!(is_public_ip("100.63.255.255".parse().unwrap()) && is_public_ip("100.128.0.1".parse().unwrap()), "CGNAT is exactly 100.64/10");
    }

    #[tokio::test]
    async fn endpoints_must_be_http_and_point_outside() {
        for bad in [
            "http://127.0.0.1:8080/v1", "http://localhost/v1", "http://169.254.169.254/latest/meta-data", "https://[::1]/v1", "http://0.0.0.0/", "http://2130706433/v1",
            "http://0x7f.0.0.1/", "http://10.1.2.3/", "file:///etc/passwd", "ftp://example.com/", "gopher://x/", "javascript:alert(1)", "", "not a url", "https://user:pw@8.8.8.8/",
            "http://[::ffff:127.0.0.1]/", "http://metadata.google.internal.localhost/",
        ] {
            let r = check_endpoint(bad).await;
            assert!(r.as_ref().err().map_or(false, |e| e.1.contains("endpoint_not_allowed")), "{bad} -> {r:?}");
        }
        for ok in ["https://8.8.8.8/v1", "http://93.184.216.34:11434/v1", "https://[2606:4700:4700::1111]/v1"] {
            assert!(check_endpoint(ok).await.is_ok(), "{ok}");
        }
    }

    #[tokio::test]
    async fn local_callers_are_exempt_so_a_local_model_keeps_working() {
        let g = LegacyGuard::new(1, 1);
        for _ in 0..5 {
            assert!(g.check(&from("127.0.0.1"), Some("http://localhost:11434/v1"), true).await.is_ok());
            assert!(g.check(&from("192.168.1.20"), Some("http://10.0.0.5:8000/v1"), true).await.is_ok());
        }
    }

    #[tokio::test]
    async fn outsiders_cannot_reach_internal_addresses_even_with_their_own_key() {
        let g = LegacyGuard::new(100, 100);
        assert!(g.check(&from("203.0.113.9"), Some("http://169.254.169.254/latest"), false).await.is_err());
        assert!(g.check(&from("203.0.113.9"), Some("http://localhost:3000/api/platform/me"), false).await.is_err());
        assert!(g.check(&from("203.0.113.9"), Some("https://8.8.8.8/v1"), false).await.is_ok());
        // a request without a recognisable client address is treated as an outsider
        assert!(g.check(&HeaderMap::new(), Some("http://127.0.0.1/"), false).await.is_err());
    }

    #[tokio::test]
    async fn the_operators_own_endpoint_is_trusted_even_when_it_is_private() {
        // AI_ENDPOINT=http://ollama:11434/v1 on a docker network: outsiders may use it (within the caps), they just
        // cannot name their own internal address
        let g = LegacyGuard::new(100, 100);
        assert!(g.check(&from("203.0.113.9"), None, true).await.is_ok());
        assert!(g.check(&from("203.0.113.9"), Some("http://ollama:11434/v1"), true).await.is_ok(), "unresolvable name: nothing to protect");
        assert!(g.check(&from("203.0.113.9"), Some("http://10.0.0.7:11434/v1"), true).await.is_err());
    }

    #[tokio::test]
    async fn the_operator_can_close_the_endpoints_to_outsiders_only() {
        let g = LegacyGuard::new(100, 100).with_outsiders(false);
        let e = g.check(&from("203.0.113.9"), Some("https://8.8.8.8/v1"), false).await.unwrap_err();
        assert_eq!((e.0, e.1.contains("legacy_ai_disabled")), (StatusCode::FORBIDDEN, true));
        assert!(g.check(&from("127.0.0.1"), Some("http://localhost:11434/v1"), true).await.is_ok(), "own machine keeps working");
        assert!(g.check(&from("203.0.113.9"), Some("https://8.8.8.8/v1"), false).await.is_err());
    }

    #[tokio::test]
    async fn using_the_servers_key_is_capped_per_client_and_in_total() {
        let g = LegacyGuard::new(3, 5);
        let a = from("203.0.113.1");
        let b = from("203.0.113.2");
        let url = "https://8.8.8.8/v1";
        for _ in 0..3 {
            assert!(g.check(&a, Some(url), true).await.is_ok());
        }
        assert_eq!(g.check(&a, Some(url), true).await.unwrap_err().0, StatusCode::TOO_MANY_REQUESTS, "per-client cap");
        assert!(g.check(&b, Some(url), true).await.is_ok());
        assert!(g.check(&b, Some(url), true).await.is_ok());
        // 5 calls were counted in total (3 + 2); the 6th from anyone is refused. (a's refused 4th call counted too)
        assert!(g.check(&from("203.0.113.3"), Some(url), true).await.is_err(), "global daily cap");
        // a caller's own key costs the operator nothing: never counted
        for _ in 0..20 {
            assert!(g.check(&from("203.0.113.4"), Some(url), false).await.is_ok());
        }
    }
}
