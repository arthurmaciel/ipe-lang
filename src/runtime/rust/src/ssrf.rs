//! SSRF deny-private guard — reqwest-free URL/host validation.
//!
//! Factored out of `http_client.rs` so the WebSocket client (which dials outside
//! reqwest) and any other surface can validate URLs against the deny-private
//! policy WITHOUT linking the reqwest HTTP stack. URL parsing uses the `url`
//! crate directly — `reqwest::Url` is `pub use url::Url;` (reqwest/src/lib.rs),
//! so this is the exact same parser reqwest uses; moving here changes no parse
//! semantics. The reqwest-coupled `ssrf_apply` (it takes a `reqwest::ClientBuilder`)
//! stays in `http_client.rs` and imports the helpers below.
//!
//! ## SSRF protection
//!
//! Blocks requests whose resolved host is loopback, RFC-1918 private,
//! link-local, unique-local (ULA), unspecified, or v4-mapped-private.
//! The guard is **default-ON in production, default-OFF in dev**:
//!
//! * `IPE_HTTP_DENY_PRIVATE` set to a truthy value (`1`/`on`/`true`) → ON.
//! * `IPE_HTTP_DENY_PRIVATE` set to anything else → OFF (explicit opt-out).
//! * `IPE_HTTP_DENY_PRIVATE` unset → follows the production gate
//!   (`production_from_env`): ON in production, OFF in dev so localhost
//!   development keeps working.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use url::Url;

/// Returns `true` when the SSRF deny-private guard is active.
///
/// Default-ON in production (PRINCIPLES #1: the safe outcome must be the default
/// for untrusted input). The decision:
///   * `IPE_HTTP_DENY_PRIVATE` set to a truthy value (`1`/`on`/`true`) → ON.
///   * `IPE_HTTP_DENY_PRIVATE` set to anything else (`0`/`off`/`false`/…) → OFF
///     (explicit opt-out, so a production deploy that genuinely needs to reach a
///     private host can disable it deliberately).
///   * `IPE_HTTP_DENY_PRIVATE` UNSET → tied to the production gate
///     (`production_from_env`): ON in production (`ENV`/`IPE_ENV` not in
///     {unset, dev, development, local}), OFF in dev so localhost development
///     keeps working unchanged.
///
/// Env is read only through the crate's locked accessors (`read_env_var` +
/// `production_from_env`), never raw `std::env`.
pub(crate) fn ssrf_deny_private_enabled() -> bool {
    match crate::system::read_env_var("IPE_HTTP_DENY_PRIVATE") {
        Ok(v) => matches!(v.to_ascii_lowercase().trim(), "1" | "on" | "true"),
        Err(_) => crate::telemetry::production_from_env(),
    }
}

/// Returns `true` when the address belongs to a range that must be blocked
/// under `IPE_HTTP_DENY_PRIVATE`:
///
/// - loopback        (127.0.0.0/8, ::1)
/// - RFC-1918        (10/8, 172.16/12, 192.168/16)
/// - link-local      (169.254/16, fe80::/10)
/// - unique-local    (fc00::/7 — fc00:: and fd00::)
/// - unspecified     (0.0.0.0, ::)
/// - this-network    (0.0.0.0/8 — RFC 1122; whole /8, not just 0.0.0.0)
/// - CGNAT/shared    (100.64.0.0/10 — RFC 6598; cloud internal hosts live here)
/// - IETF protocol   (192.0.0.0/24 — incl. 192.0.0.192)
/// - benchmarking    (198.18.0.0/15 — RFC 2544)
/// - reserved/bcast  (240.0.0.0/4 — incl. 255.255.255.255)
/// - v4-mapped IPv6  (::ffff:0:0/96) whose embedded v4 is in the above ranges
/// - NAT64           (64:ff9b::/96) / 6to4 (2002::/16) whose embedded v4 is private
///
/// The std `Ipv4Addr` predicates for CGNAT / benchmarking / reserved are
/// nightly-only (`is_shared`/`is_benchmarking`/`is_reserved`), so the extra
/// ranges are matched by octet here (audit finding L1: RFC-1918-only coverage
/// left 100.64/10 + 240/4 reachable under the deny-private guard).
pub(crate) fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            // Array-destructure (not indexing) → provably total, no panic site.
            let [a, b, c, _] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || a == 0                             // 0.0.0.0/8 "this host on this network" (RFC 1122)
                || (a == 100 && (b & 0xc0) == 0x40)   // 100.64.0.0/10
                || (a == 192 && b == 0 && c == 0)     // 192.0.0.0/24
                || (a == 198 && (b & 0xfe) == 18)     // 198.18.0.0/15
                || a >= 240 // 240.0.0.0/4 (incl. 255.255.255.255)
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // Slice-pattern destructure (not indexing) → provably total, no panic site.
            let [s0, s1, s2, _s3, _s4, _s5, s6, s7] = v6.segments();
            // Link-local: fe80::/10
            if (s0 & 0xffc0) == 0xfe80 {
                return true;
            }
            // Unique-local: fc00::/7 (covers fc00:: and fd00::)
            if (s0 & 0xfe00) == 0xfc00 {
                return true;
            }
            // NAT64: 64:ff9b::/96 embeds the destination IPv4 in the low 32 bits.
            // to_ipv4_mapped/to_ipv4 miss it (high bits non-zero), so a private v4
            // reachable through a NAT64 gateway would slip past the v4 checks below.
            if s0 == 0x0064 && s1 == 0xff9b {
                let v4 = Ipv4Addr::new(
                    (s6 >> 8) as u8,
                    (s6 & 0xff) as u8,
                    (s7 >> 8) as u8,
                    (s7 & 0xff) as u8,
                );
                return is_private_ip(IpAddr::V4(v4));
            }
            // 6to4: 2002::/16 embeds the IPv4 in segments 1..=2 (2002:V4HI:V4LO::/48).
            if s0 == 0x2002 {
                let v4 = Ipv4Addr::new(
                    (s1 >> 8) as u8,
                    (s1 & 0xff) as u8,
                    (s2 >> 8) as u8,
                    (s2 & 0xff) as u8,
                );
                return is_private_ip(IpAddr::V4(v4));
            }
            // v4-mapped: ::ffff:0:0/96
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            // v4-compatible (deprecated): ::a.b.c.d, e.g. ::10.0.0.1 still routes to
            // the embedded private IPv4 — to_ipv4() covers both compat + mapped.
            #[allow(deprecated)]
            if let Some(v4) = v6.to_ipv4() {
                return is_private_ip(IpAddr::V4(v4));
            }
            false
        }
    }
}

/// Resolves `host` (plain hostname or IP literal), checks that none of its
/// addresses is in a disallowed private range, and returns the first non-private
/// `SocketAddr` (port 0) so the caller can **pin** reqwest's DNS resolver to
/// that exact address via `ClientBuilder::resolve_to_addrs`.
///
/// This closes the TOCTOU / DNS-rebinding window: the IP that passed the check
/// is the IP reqwest connects to — a rebind happening between the check and the
/// TCP connect has no effect because reqwest's per-client DNS override wins.
///
/// Returns `Ok(SocketAddr)` if allowed (with the vetted address), or
/// `Err(message)` if blocked or if the host could not be resolved.
///
/// Uses port 0 for the `ToSocketAddrs` resolution; the port is not significant
/// for the DNS lookup but is required by the API.  Callers must override the
/// port in `resolve_to_addrs` if needed — reqwest's `resolve_to_addrs` only
/// overrides the host→IP mapping; it uses the URL's port for the actual
/// connection, so port 0 here is safe.
pub(crate) fn resolve_first_non_private_addr(host: &str) -> Result<SocketAddr, String> {
    // The HTTP client only needs the IP (reqwest's resolve_to_addrs ignores the
    // port), so port 0 is fine here.
    resolve_first_non_private_addr_with_port(host, 0)
}

/// Resolve `host` to its first non-private `SocketAddr` carrying `port`, rejecting
/// if any resolved address is private/loopback/link-local. Used by the WebSocket
/// pin (which dials the returned addr directly so the connect cannot re-resolve
/// to a rebind target).
pub(crate) fn resolve_first_non_private_addr_with_port(
    host: &str,
    port: u16,
) -> Result<SocketAddr, String> {
    // Try parsing as an IP literal first (avoids a DNS round-trip for bare IPs).
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(format!(
                "http: blocked: private/loopback host {} (IPE_HTTP_DENY_PRIVATE)",
                ip
            ));
        }
        return Ok(SocketAddr::new(ip, port));
    }

    // Hostname → DNS resolve via std (synchronous; called before the async send).
    let addr_iter = match (host, port).to_socket_addrs() {
        Ok(it) => it,
        Err(e) => {
            return Err(format!(
                "http: blocked: could not resolve host {:?}: {} (IPE_HTTP_DENY_PRIVATE)",
                host, e
            ));
        }
    };

    let mut first_ok: Option<SocketAddr> = None;
    for sock_addr in addr_iter {
        let ip = sock_addr.ip();
        if is_private_ip(ip) {
            return Err(format!(
                "http: blocked: private/loopback host {:?} resolved to {} (IPE_HTTP_DENY_PRIVATE)",
                host, ip
            ));
        }
        if first_ok.is_none() {
            first_ok = Some(sock_addr);
        }
    }

    first_ok.ok_or_else(|| {
        format!(
            "http: blocked: host {:?} resolved to no addresses (IPE_HTTP_DENY_PRIVATE)",
            host
        )
    })
}

/// WebSocket SSRF pin: when `IPE_HTTP_DENY_PRIVATE` is on, resolve `url`'s host to
/// a vetted non-private `SocketAddr` (with the real ws/wss port) so the caller can
/// dial THAT addr — closing the DNS-rebinding TOCTOU that an unpinned
/// `connect_async` (which re-resolves the name at connect) would leave open.
/// Returns `Ok(None)` when the guard is off (caller uses the normal path).
///
/// Sole consumer is `ws_client.rs`. Use `cfg_attr`+`allow`, NOT `#[cfg(...)]`:
/// generated projects include ssrf by MODULE (Project.hs) without declaring a
/// `websocket_client` Cargo feature, so a `#[cfg]` would remove the fn and E0425
/// a generated ws caller. The attribute only silences the dead-code lint in
/// standalone subsets that compile ssrf without the ws client.
#[cfg_attr(not(feature = "websocket_client"), allow(dead_code))]
pub(crate) fn ssrf_pinned_ws_addr(url: &str) -> Result<Option<SocketAddr>, String> {
    if !ssrf_deny_private_enabled() {
        return Ok(None);
    }
    let parsed = Url::parse(url).map_err(|e| {
        format!(
            "ws: blocked: invalid URL {:?}: {} (IPE_HTTP_DENY_PRIVATE)",
            url, e
        )
    })?;
    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| "ws: blocked: URL has no host (IPE_HTTP_DENY_PRIVATE)".to_string())?;
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if scheme == "wss" { 443 } else { 80 });
    resolve_first_non_private_addr_with_port(host, port).map(Some)
}

/// Validates a URL's host against the SSRF deny-private policy without
/// returning the resolved address.  Used in the redirect policy callback
/// where we only have a URL and cannot rebuild the client.
///
/// SECURITY — redirect-hop scheme/host + IP-literal check. The DNS-rebind window
/// this once left open (validate-then-discard: the vetted address was not the one
/// reqwest re-resolved at connect) is now CLOSED by `http_client::DenyPrivateResolver`,
/// a `reqwest::dns::Resolve` that runs `is_private_ip` at resolution time for EVERY
/// hop under `IPE_HTTP_DENY_PRIVATE` (so reqwest connects only to vetted addrs, no
/// re-resolve by name). This function remains the literal/scheme guard and a
/// belt-and-suspenders layer — IP-literal redirect targets bypass the resolver, so
/// the per-hop `ssrf_check_url` call is still mandatory.
///
/// Returns `Ok(())` if allowed, `Err(message)` if blocked.
fn check_host_not_private(host: &str) -> Result<(), String> {
    resolve_first_non_private_addr(host).map(|_| ())
}

/// Validates a URL string under the SSRF deny-private policy.
/// Rejects non-http/https schemes and private-range hosts.
///
/// Returns `Ok(())` if the request is allowed, `Err(message)` if blocked.
pub(crate) fn ssrf_check_url(url: &str) -> Result<(), String> {
    let parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(e) => {
            return Err(format!(
                "http: blocked: invalid URL {:?}: {} (IPE_HTTP_DENY_PRIVATE)",
                url, e
            ));
        }
    };

    // Permit http / https (HTTP client + redirect hops) AND ws / wss (the
    // WebSocket client validates through this same fn). Without ws/wss here the
    // guard rejected EVERY WebSocket URL when enabled (deny-all) and the private-
    // IP host check below never ran for ws/wss — i.e. the guard was a no-op for
    // the WebSocket surface. Everything else (ftp/file/…) stays rejected.
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" && scheme != "ws" && scheme != "wss" {
        return Err(format!(
            "http: blocked: scheme {:?} is not http/https/ws/wss (IPE_HTTP_DENY_PRIVATE)",
            scheme
        ));
    }

    // Extract and check the host.
    let host = match parsed.host_str() {
        Some(h) => h,
        None => {
            return Err("http: blocked: URL has no host (IPE_HTTP_DENY_PRIVATE)".to_string());
        }
    };

    check_host_not_private(host)
}

/// Validate a single URL against the deny-private guard (no client build) — for
/// surfaces (WebSocket) that connect outside reqwest. No-op when the guard is off.
/// Sole consumer is `ws_client.rs`; `cfg_attr`+`allow` (not `#[cfg]`) for the same
/// generated-module-inclusion reason as `ssrf_pinned_ws_addr` above.
#[cfg_attr(not(feature = "websocket_client"), allow(dead_code))]
pub(crate) fn ssrf_validate_url(url: &str) -> Result<(), String> {
    if ssrf_deny_private_enabled() {
        ssrf_check_url(url)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // SSRF guard unit tests (no network — purely local logic)
    // -----------------------------------------------------------------------

    #[test]
    fn is_private_ip_loopback_v4() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("127.255.255.255".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_rfc1918() {
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("172.31.255.255".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_link_local_v4() {
        assert!(is_private_ip("169.254.0.1".parse().unwrap()));
        assert!(is_private_ip("169.254.169.254".parse().unwrap())); // AWS IMDS
    }

    #[test]
    fn is_private_ip_unspecified_v4() {
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_loopback_v6() {
        assert!(is_private_ip("::1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_link_local_v6() {
        assert!(is_private_ip("fe80::1".parse().unwrap()));
        assert!(is_private_ip("fe80::dead:beef".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_ula_v6() {
        assert!(is_private_ip("fc00::1".parse().unwrap()));
        assert!(is_private_ip("fd00::1".parse().unwrap()));
        assert!(is_private_ip("fdff:ffff:ffff::1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_v4mapped_private() {
        // ::ffff:192.168.1.1 — v4-mapped RFC-1918
        assert!(is_private_ip("::ffff:192.168.1.1".parse().unwrap()));
        // ::ffff:127.0.0.1 — v4-mapped loopback
        assert!(is_private_ip("::ffff:127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_this_network_v4() {
        // 0.0.0.0/8 "this host on this network" (RFC 1122) — whole /8, not just 0.0.0.0.
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
        assert!(is_private_ip("0.0.0.1".parse().unwrap()));
        assert!(is_private_ip("0.255.255.255".parse().unwrap()));
        // Boundary just above the /8 must stay public.
        assert!(!is_private_ip("1.0.0.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_nat64_embedded_private() {
        // 64:ff9b::/96 embeds the destination IPv4 in the low 32 bits.
        assert!(is_private_ip("64:ff9b::7f00:1".parse().unwrap())); // → 127.0.0.1
        assert!(is_private_ip("64:ff9b::a9fe:a9fe".parse().unwrap())); // → 169.254.169.254 (AWS IMDS)
        assert!(is_private_ip("64:ff9b::c0a8:101".parse().unwrap())); // → 192.168.1.1
        // NAT64 wrapping a public v4 stays public.
        assert!(!is_private_ip("64:ff9b::101:101".parse().unwrap())); // → 1.1.1.1
    }

    #[test]
    fn is_private_ip_6to4_embedded_private() {
        // 2002::/16 embeds the IPv4 in segments 1..=2 (2002:V4HI:V4LO::).
        assert!(is_private_ip("2002:7f00:1::1".parse().unwrap())); // → 127.0.0.1
        assert!(is_private_ip("2002:a9fe:a9fe::1".parse().unwrap())); // → 169.254.169.254
        assert!(is_private_ip("2002:c0a8:101::1".parse().unwrap())); // → 192.168.1.1
        // 6to4 wrapping a public v4 stays public.
        assert!(!is_private_ip("2002:101:101::1".parse().unwrap())); // → 1.1.1.1
    }

    #[test]
    fn is_private_ip_public_is_allowed() {
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("2606:4700:4700::1111".parse().unwrap())); // Cloudflare v6
    }

    #[test]
    fn is_private_ip_extra_reserved_ranges_blocked() {
        // audit L1: non-RFC-1918 ranges that std's is_private misses.
        assert!(is_private_ip("100.64.0.1".parse().unwrap())); // CGNAT lo
        assert!(is_private_ip("100.127.255.255".parse().unwrap())); // CGNAT hi
        assert!(is_private_ip("192.0.0.192".parse().unwrap())); // IETF protocol
        assert!(is_private_ip("198.18.0.1".parse().unwrap())); // benchmarking lo
        assert!(is_private_ip("198.19.255.255".parse().unwrap())); // benchmarking hi
        assert!(is_private_ip("240.0.0.1".parse().unwrap())); // reserved
        assert!(is_private_ip("255.255.255.255".parse().unwrap())); // broadcast
        // Boundaries that must STAY public (no over-block):
        assert!(!is_private_ip("100.63.255.255".parse().unwrap())); // just below CGNAT
        assert!(!is_private_ip("100.128.0.0".parse().unwrap())); // just above CGNAT
        assert!(!is_private_ip("192.0.1.1".parse().unwrap())); // 192.0.1/24 is public
        assert!(!is_private_ip("198.20.0.0".parse().unwrap())); // just above benchmarking
        assert!(!is_private_ip("239.255.255.255".parse().unwrap())); // just below reserved (multicast, routable-ish)
    }

    #[test]
    fn ssrf_check_url_rejects_non_http_scheme() {
        let err = ssrf_check_url("ftp://example.com/file").unwrap_err();
        assert!(
            err.contains("scheme"),
            "expected scheme rejection, got: {err}"
        );
        let err2 = ssrf_check_url("file:///etc/passwd").unwrap_err();
        assert!(
            err2.contains("scheme") || err2.contains("invalid"),
            "got: {err2}"
        );
    }

    #[test]
    fn ssrf_check_url_rejects_private_ip_literal() {
        let err = ssrf_check_url("http://192.168.1.1/secret").unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    #[test]
    fn ssrf_check_url_rejects_loopback_ip_literal() {
        let err = ssrf_check_url("http://127.0.0.1:8080/admin").unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    #[test]
    fn ssrf_check_url_rejects_aws_imds() {
        let err = ssrf_check_url("http://169.254.169.254/latest/meta-data/").unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    #[test]
    fn ssrf_check_url_rejects_invalid_url() {
        let err = ssrf_check_url("not a url at all").unwrap_err();
        assert!(!err.is_empty());
    }

    /// The redirect-hop re-validation floor. `ssrf_apply`'s redirect policy calls
    /// `ssrf_check_url(attempt.url())` on EVERY hop before following it, so a
    /// redirect whose `Location` points at an internal / loopback / link-local /
    /// metadata address is blocked even though the FIRST hop was a safe public
    /// URL — the initial validation is not trusted for later hops. This asserts
    /// the exact function the policy invokes rejects each such hop target.
    #[test]
    fn redirect_hop_revalidation_blocks_internal_targets() {
        for hop in [
            "http://127.0.0.1/admin",             // loopback
            "http://10.0.0.5/internal",           // RFC-1918 private
            "http://169.254.169.254/latest/meta", // AWS IMDS link-local
            "http://[::1]/",                      // v6 loopback
            "http://192.168.1.1/",                // RFC-1918 private
        ] {
            let err = ssrf_check_url(hop)
                .expect_err("a redirect hop to an internal address must be blocked");
            assert!(err.contains("blocked"), "hop {hop:?} → got: {err}");
        }
        // A redirect hop with a non-http(s) scheme is blocked at the same floor.
        let err = ssrf_check_url("ftp://example.com/x")
            .expect_err("a non-http(s) redirect hop must be blocked");
        assert!(err.contains("scheme"), "got: {err}");
    }

    #[test]
    fn ssrf_check_url_allows_public_ip() {
        // 1.1.1.1 is public — should pass (no DNS needed for IP literals)
        assert!(ssrf_check_url("https://1.1.1.1/").is_ok());
    }

    // -----------------------------------------------------------------------
    // resolve_first_non_private_addr — pin-address path
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_non_private_returns_socket_addr_for_public_ip_literal() {
        let sa = resolve_first_non_private_addr("1.1.1.1").unwrap();
        assert_eq!(sa.ip(), "1.1.1.1".parse::<IpAddr>().unwrap());
        // Port is 0 — reqwest uses the URL's port; we just need the IP.
        assert_eq!(sa.port(), 0);
    }

    #[test]
    fn resolve_non_private_rejects_private_ip_literal() {
        let err = resolve_first_non_private_addr("192.168.1.1").unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    #[test]
    fn resolve_non_private_rejects_loopback_ip_literal() {
        let err = resolve_first_non_private_addr("127.0.0.1").unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    #[test]
    fn resolve_non_private_rejects_v4mapped_loopback() {
        let err = resolve_first_non_private_addr("::ffff:127.0.0.1").unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    // -----------------------------------------------------------------------
    // Parse-parity: prove `url::Url::parse` (used here) extracts the same
    // scheme/host/port as `reqwest::Url::parse` (the pre-refactor parser) for
    // the SSRF-relevant cases. reqwest::Url IS url::Url (pub use), so these are
    // a belt-and-braces regression against an accidental parser divergence.
    // Only compiled when reqwest is linked (http_client/email/live builds).
    #[cfg(feature = "http_client")]
    #[test]
    fn url_parse_parity_with_reqwest_for_ssrf_extractions() {
        let cases = [
            "http://user:pass@example.com:8080/path?q=1",
            "https://xn--n3h.example/", // punycode/IDN host
            "http://0x7f.0.0.1/",       // hex-ish IPv4-looking host
            "http://0177.0.0.1/",       // octal-ish IPv4-looking host
            "http://[::ffff:127.0.0.1]/",
            "wss://[::1]:8443/socket",
            "ws://example.com./",   // trailing-dot host
            "https://例え.テスト/", // raw IDN
            "http://127.0.0.1:8080/admin",
            "https://1.1.1.1/",
        ];
        for c in cases {
            let a = reqwest::Url::parse(c);
            let b = Url::parse(c);
            assert_eq!(a.is_ok(), b.is_ok(), "parse-ok divergence for {c:?}");
            if let (Ok(ua), Ok(ub)) = (a, b) {
                assert_eq!(ua.scheme(), ub.scheme(), "scheme divergence for {c:?}");
                assert_eq!(ua.host_str(), ub.host_str(), "host divergence for {c:?}");
                assert_eq!(
                    ua.port_or_known_default(),
                    ub.port_or_known_default(),
                    "port divergence for {c:?}"
                );
            }
        }
    }
}
