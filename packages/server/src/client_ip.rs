//! Who is the client? Rate limits (login, register, relay publishing, reports) key on the client address, and
//! `X-Forwarded-For` is a header any client can forge. This module decides, once per request, which address to
//! trust and hands it to the handlers through an internal header the client cannot set (any inbound copy is
//! overwritten).
//!
//! * `TRUSTED_PROXY_HOPS=0` — no reverse proxy in front: use the TCP peer, ignore `X-Forwarded-For`.
//! * `TRUSTED_PROXY_HOPS=N` — N proxies in front: take the N-th entry **from the right** of `X-Forwarded-For`
//!   (each trusted proxy appends the address it saw; everything to the left was supplied by the client).
//! * unset / `auto` (default) — behave as 1 hop when the TCP peer is a loopback/private address (a proxy on the
//!   same host or network), and as 0 hops when the peer is a public address (the client itself). Behind a public
//!   CDN/load balancer (e.g. Cloudflare) set the number of hops explicitly.

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue},
    middleware::Next,
    response::Response,
};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

/// Internal header carrying the resolved client address to the handlers.
pub const HEADER: &str = "x-exameow-client-ip";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proxy {
    Auto,
    Hops(usize),
}

impl Proxy {
    pub fn from_env() -> Self {
        Self::parse(std::env::var("TRUSTED_PROXY_HOPS").ok().as_deref())
    }

    /// Anything unparseable falls back to `Auto` rather than silently trusting or distrusting everything.
    pub fn parse(v: Option<&str>) -> Self {
        match v.map(|s| s.trim().to_lowercase()) {
            Some(s) if !s.is_empty() && s != "auto" => s.parse::<usize>().ok().filter(|n| *n <= 10).map_or(Proxy::Auto, Proxy::Hops),
            _ => Proxy::Auto,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Proxy::Auto => "auto (X-Forwarded-For is used only when the TCP peer is a loopback/private address)".into(),
            Proxy::Hops(0) => "0 hops (X-Forwarded-For is ignored)".into(),
            Proxy::Hops(n) => format!("{n} hop(s) (client = {n}-th X-Forwarded-For entry from the right)"),
        }
    }
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

/// Loopback, private (RFC 1918), link-local and IPv6 unique-local addresses.
pub fn is_local(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80 || v6 == Ipv6Addr::UNSPECIFIED,
    }
}

/// The address to key limits on. `xff` is every `X-Forwarded-For` value joined with commas, in order.
pub fn resolve(peer: IpAddr, xff: Option<&str>, mode: Proxy) -> IpAddr {
    let peer = canonical(peer);
    let hops = match mode {
        Proxy::Hops(n) => n,
        Proxy::Auto => usize::from(is_local(peer)),
    };
    if hops == 0 {
        return peer;
    }
    let entries: Vec<&str> = xff.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    entries
        .len()
        .checked_sub(hops)
        .and_then(|i| entries.get(i))
        .and_then(|s| s.parse::<IpAddr>().ok().or_else(|| s.parse::<SocketAddr>().ok().map(|a| a.ip())))
        .map_or(peer, canonical)
}

pub async fn middleware(State(mode): State<Proxy>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    let xff = req.headers().get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join(",");
    let ip = resolve(peer.ip(), Some(&xff), mode);
    if let Ok(v) = HeaderValue::from_str(&ip.to_string()) {
        req.headers_mut().insert(HEADER, v); // replaces anything the client sent under this name
    }
    next.run(req).await
}

/// The client address as resolved by `middleware` (`"unknown"` outside the server, e.g. in unit tests).
pub fn from_headers(headers: &HeaderMap) -> String {
    headers.get(HEADER).and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty()).unwrap_or("unknown").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn locality_covers_loopback_private_link_local_and_unique_local_but_not_public() {
        for local in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.254", "192.168.1.1", "169.254.1.1", "::1", "fd00::1", "fe80::1", "::ffff:10.0.0.1", "::ffff:127.0.0.1"] {
            assert!(is_local(ip(local)), "{local}");
        }
        for public in ["8.8.8.8", "172.32.0.1", "172.15.0.1", "192.169.0.1", "100.64.0.1", "2001:db8::1", "::ffff:8.8.8.8", "1.1.1.1"] {
            assert!(!is_local(ip(public)), "{public}");
        }
    }

    #[test]
    fn forged_left_hand_entries_never_win() {
        let p = ip("10.0.0.5"); // a proxy on the private network
        // one trusted proxy appended the real client (rightmost); the client pre-filled the left side
        assert_eq!(resolve(p, Some("6.6.6.6, 1.2.3.4, 203.0.113.9"), Proxy::Hops(1)), ip("203.0.113.9"));
        assert_eq!(resolve(p, Some("6.6.6.6, 1.2.3.4, 203.0.113.9"), Proxy::Auto), ip("203.0.113.9"));
        // two proxies: the entry the OUTER proxy appended is second from the right
        assert_eq!(resolve(p, Some("6.6.6.6, 198.51.100.7, 10.0.0.9"), Proxy::Hops(2)), ip("198.51.100.7"));
        // a proxy that overwrites the header leaves a single value
        assert_eq!(resolve(p, Some("198.51.100.7"), Proxy::Hops(1)), ip("198.51.100.7"));
    }

    #[test]
    fn a_public_peer_is_the_client_and_its_header_is_ignored_in_auto_mode() {
        let client = ip("203.0.113.50");
        assert_eq!(resolve(client, Some("1.2.3.4"), Proxy::Auto), client, "direct exposure: a forged header changes nothing");
        assert_eq!(resolve(client, None, Proxy::Auto), client);
        assert_eq!(resolve(ip("203.0.113.50"), Some("1.2.3.4"), Proxy::Hops(0)), client);
        // a public CDN peer needs the hop count configured explicitly
        assert_eq!(resolve(client, Some("9.9.9.9"), Proxy::Hops(1)), ip("9.9.9.9"));
    }

    #[test]
    fn zero_hops_ignores_the_header_even_from_a_local_peer() {
        assert_eq!(resolve(ip("127.0.0.1"), Some("1.2.3.4"), Proxy::Hops(0)), ip("127.0.0.1"));
    }

    #[test]
    fn missing_short_or_garbage_headers_fall_back_to_the_peer() {
        let p = ip("127.0.0.1");
        for xff in [None, Some(""), Some(" , "), Some("not-an-ip"), Some("unknown"), Some("<script>")] {
            assert_eq!(resolve(p, xff, Proxy::Auto), p, "{xff:?}");
        }
        assert_eq!(resolve(p, Some("1.2.3.4"), Proxy::Hops(2)), p, "fewer entries than trusted hops");
    }

    #[test]
    fn ports_and_ipv6_in_the_header_are_understood() {
        let p = ip("127.0.0.1");
        assert_eq!(resolve(p, Some("203.0.113.9:51234"), Proxy::Auto), ip("203.0.113.9"));
        assert_eq!(resolve(p, Some("2001:db8::7"), Proxy::Auto), ip("2001:db8::7"));
        assert_eq!(resolve(p, Some("[2001:db8::7]:443"), Proxy::Auto), ip("2001:db8::7"));
        assert_eq!(resolve(ip("::ffff:127.0.0.1"), Some("::ffff:203.0.113.9"), Proxy::Auto), ip("203.0.113.9"), "v4-mapped is the same address");
    }

    #[test]
    fn configuration_parsing() {
        assert_eq!(Proxy::parse(None), Proxy::Auto);
        assert_eq!(Proxy::parse(Some("")), Proxy::Auto);
        assert_eq!(Proxy::parse(Some(" AUTO ")), Proxy::Auto);
        assert_eq!(Proxy::parse(Some("0")), Proxy::Hops(0));
        assert_eq!(Proxy::parse(Some(" 2 ")), Proxy::Hops(2));
        assert_eq!(Proxy::parse(Some("11")), Proxy::Auto, "absurd values are not trusted");
        assert_eq!(Proxy::parse(Some("-1")), Proxy::Auto);
        assert_eq!(Proxy::parse(Some("yes")), Proxy::Auto);
    }

    #[test]
    fn handlers_read_the_resolved_address_only() {
        let mut h = HeaderMap::new();
        assert_eq!(from_headers(&h), "unknown");
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6"));
        assert_eq!(from_headers(&h), "unknown", "the raw header is never consulted by handlers");
        h.insert(HEADER, HeaderValue::from_static("203.0.113.9"));
        assert_eq!(from_headers(&h), "203.0.113.9");
    }
}
