//! SSRF deny-private guard — reqwest-free URL/host validation.
//!
//! Factored out of `http_client.rs` so the WebSocket client (which dials outside
//! reqwest) and any other surface can validate URLs against the deny-private
//! policy WITHOUT linking the reqwest HTTP stack. URL parsing uses the `url`
//! crate directly — `reqwest::Url` is `pub use url::Url;` (reqwest/src/lib.rs),
//! so this is the exact same parser reqwest uses. The reqwest-coupled
//! `ssrf_apply` (it takes a `reqwest::ClientBuilder`) stays in `http_client.rs`
//! and imports the helpers below.
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
//!
//! ## Pinning
//!
//! A vetted name is resolved exactly once, through tokio's non-blocking
//! resolver under a bounded deadline, and the caller dials the vetted address
//! itself ([`VettedDial::Pinned`]). A dial that resolves the name a second
//! time would reopen the DNS-rebinding window: the first answer passes the
//! check, the second points at an internal host.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
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

/// The deny-private policy a dial runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialPolicy {
    /// Refuse every blocked address and pin the dial to the vetted one.
    DenyPrivate,
    /// Dial any host (local development, or an explicit opt-out).
    AllowAll,
}

impl DialPolicy {
    /// The policy `IPE_HTTP_DENY_PRIVATE` and the production gate select.
    #[must_use]
    pub fn from_env() -> Self {
        if ssrf_deny_private_enabled() {
            Self::DenyPrivate
        } else {
            Self::AllowAll
        }
    }
}

/// The class of a blocked address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedRange {
    /// Loopback: `127.0.0.0/8`, `::1`.
    Loopback,
    /// Link-local: `169.254.0.0/16` (cloud metadata), `fe80::/10`.
    LinkLocal,
    /// A private network: RFC 1918, unique-local `fc00::/7`, CGNAT `100.64.0.0/10`.
    Private,
    /// Unspecified, this-network, IETF protocol, benchmarking, or reserved space.
    Reserved,
}

impl std::fmt::Display for BlockedRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Loopback => "loopback",
            Self::LinkLocal => "link-local",
            Self::Private => "private",
            Self::Reserved => "reserved",
        })
    }
}

/// The blocked class `ip` belongs to, or `None` for a routable public address.
///
/// Covers:
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
/// - NAT64           (64:ff9b::/96) / 6to4 (2002::/16) whose embedded v4 is blocked
///
/// The std `Ipv4Addr` predicates for CGNAT / benchmarking / reserved are
/// nightly-only (`is_shared`/`is_benchmarking`/`is_reserved`), so those ranges
/// are matched by octet.
#[must_use]
pub fn blocked_range(ip: IpAddr) -> Option<BlockedRange> {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => blocked_v6(v6),
    }
}

fn blocked_v4(v4: Ipv4Addr) -> Option<BlockedRange> {
    // Array-destructure (not indexing) → provably total, no panic site.
    let [a, b, c, _] = v4.octets();
    // 100.64.0.0/10 (CGNAT, RFC 6598).
    let cgnat = a == 100 && (b & 0xc0) == 0x40;
    // 0.0.0.0/8 "this network" (RFC 1122), 192.0.0.0/24 (IETF protocol),
    // 198.18.0.0/15 (benchmarking), 240.0.0.0/4 (reserved, incl. broadcast).
    let reserved =
        a == 0 || (a == 192 && b == 0 && c == 0) || (a == 198 && (b & 0xfe) == 18) || a >= 240;
    if v4.is_loopback() {
        Some(BlockedRange::Loopback)
    } else if v4.is_link_local() {
        Some(BlockedRange::LinkLocal)
    } else if v4.is_private() || cgnat {
        Some(BlockedRange::Private)
    } else if v4.is_unspecified() || reserved {
        Some(BlockedRange::Reserved)
    } else {
        None
    }
}

/// The IPv4 address carried in two IPv6 segments (`hi:lo`).
const fn v4_from_segments(hi: u16, lo: u16) -> Ipv4Addr {
    let [a, b] = hi.to_be_bytes();
    let [c, d] = lo.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

fn blocked_v6(v6: Ipv6Addr) -> Option<BlockedRange> {
    if v6.is_loopback() {
        return Some(BlockedRange::Loopback);
    }
    if v6.is_unspecified() {
        return Some(BlockedRange::Reserved);
    }
    // Slice-pattern destructure (not indexing) → provably total, no panic site.
    let [s0, s1, s2, _s3, _s4, _s5, s6, s7] = v6.segments();
    // Link-local: fe80::/10
    if (s0 & 0xffc0) == 0xfe80 {
        return Some(BlockedRange::LinkLocal);
    }
    // Unique-local: fc00::/7 (covers fc00:: and fd00::)
    if (s0 & 0xfe00) == 0xfc00 {
        return Some(BlockedRange::Private);
    }
    // NAT64: 64:ff9b::/96 embeds the destination IPv4 in the low 32 bits.
    // to_ipv4_mapped/to_ipv4 miss it (high bits non-zero), so a private v4
    // reachable through a NAT64 gateway would slip past the v4 checks below.
    if s0 == 0x0064 && s1 == 0xff9b {
        return blocked_v4(v4_from_segments(s6, s7));
    }
    // 6to4: 2002::/16 embeds the IPv4 in segments 1..=2 (2002:V4HI:V4LO::/48).
    if s0 == 0x2002 {
        return blocked_v4(v4_from_segments(s1, s2));
    }
    // v4-mapped: ::ffff:0:0/96
    if let Some(v4) = v6.to_ipv4_mapped() {
        return blocked_v4(v4);
    }
    // v4-compatible (deprecated): ::a.b.c.d, e.g. ::10.0.0.1 still routes to
    // the embedded private IPv4 — to_ipv4() covers both compat + mapped.
    #[allow(deprecated)]
    if let Some(v4) = v6.to_ipv4() {
        return blocked_v4(v4);
    }
    None
}

/// Returns `true` when `ip` is in any range [`blocked_range`] classifies.
#[must_use]
pub fn is_private_ip(ip: IpAddr) -> bool {
    blocked_range(ip).is_some()
}

/// Why the SSRF gate refused a dial target.
///
/// `Display` carries no caller prefix; each surface adds its own (`http:`,
/// `ws:`, `db:`, `email.send/Smtp:`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SsrfRefusal {
    /// The host is, or resolved to, an address in a blocked range.
    Blocked {
        /// The host as the caller named it.
        host: String,
        /// The blocked address.
        ip: IpAddr,
        /// The class `ip` belongs to.
        range: BlockedRange,
    },
    /// The resolver failed for the host.
    Unresolvable {
        /// The host as the caller named it.
        host: String,
        /// The resolver's failure class.
        kind: std::io::ErrorKind,
    },
    /// The host resolved to no addresses.
    NoAddresses {
        /// The host as the caller named it.
        host: String,
    },
    /// Resolution did not finish before the deadline.
    Timeout {
        /// The host as the caller named it.
        host: String,
        /// The deadline that expired.
        after: Duration,
    },
    /// The target is a local Unix-domain socket, which reaches the local server
    /// exactly as loopback TCP does.
    LocalSocket,
    /// The connection URL names no host, so the driver picks a target the gate
    /// cannot prove safe.
    UnprovenTarget,
    /// Certificate verification needs the host name, which a pinned dial of
    /// the vetted address cannot carry.
    UnpinnableTlsName {
        /// The host the certificate would be verified against.
        host: String,
    },
}

impl std::fmt::Display for SsrfRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked { host, ip, range } => {
                if strip_ipv6_brackets(host).parse::<IpAddr>().is_ok() {
                    write!(f, "blocked: {range} host {ip}")?;
                } else {
                    write!(f, "blocked: host {host:?} resolved to {range} address {ip}")?;
                }
            }
            Self::Unresolvable { host, kind } => {
                write!(f, "blocked: could not resolve host {host:?}: {kind}")?;
            }
            Self::NoAddresses { host } => {
                write!(f, "blocked: host {host:?} resolved to no addresses")?;
            }
            Self::Timeout { host, after } => write!(
                f,
                "blocked: resolving host {host:?} timed out after {} ms",
                after.as_millis()
            )?,
            Self::LocalSocket => f.write_str("blocked: local socket dial target")?,
            Self::UnprovenTarget => f.write_str(
                "blocked: connection URL names no host, so the dial target is unproven",
            )?,
            Self::UnpinnableTlsName { host } => write!(
                f,
                "blocked: sslmode=verify-full checks the certificate against host {host:?}, \
                 but the dial is pinned to its vetted address; use an IP-literal host \
                 whose certificate names that address, or sslmode=verify-ca"
            )?,
        }
        f.write_str(" (IPE_HTTP_DENY_PRIVATE)")
    }
}

impl std::error::Error for SsrfRefusal {}

impl SsrfRefusal {
    /// This refusal with the host it names replaced by a placeholder.
    ///
    /// For a host read from a URL whose credentials may have spilled into it,
    /// so the refusal cannot echo part of a credential.
    fn withholding_host(self) -> Self {
        let withheld = || WITHHELD_HOST.to_owned();
        match self {
            Self::Blocked { ip, range, .. } => Self::Blocked {
                host: withheld(),
                ip,
                range,
            },
            Self::Unresolvable { kind, .. } => Self::Unresolvable {
                host: withheld(),
                kind,
            },
            Self::NoAddresses { .. } => Self::NoAddresses { host: withheld() },
            Self::Timeout { after, .. } => Self::Timeout {
                host: withheld(),
                after,
            },
            Self::UnpinnableTlsName { .. } => Self::UnpinnableTlsName { host: withheld() },
            Self::LocalSocket => Self::LocalSocket,
            Self::UnprovenTarget => Self::UnprovenTarget,
        }
    }
}

/// Name resolution the SSRF gate vets.
pub trait HostResolver: Sync {
    /// Every address `host:port` resolves to.
    fn lookup(
        &self,
        host: &str,
        port: u16,
    ) -> impl Future<Output = std::io::Result<Vec<SocketAddr>>> + Send;
}

/// The system resolver, through tokio's non-blocking lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemResolver;

impl HostResolver for SystemResolver {
    async fn lookup(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Ok(tokio::net::lookup_host((host, port)).await?.collect())
    }
}

/// Default deadline for one SSRF-gate name resolution.
const DNS_TIMEOUT_MS_DEFAULT: u64 = 5_000;

/// The deadline for one SSRF-gate name resolution.
///
/// A stalling resolver must not hold a task indefinitely: a remote party that
/// controls the target name's authoritative server could otherwise pile up
/// pending dials. Overridable via `IPE_HTTP_DNS_TIMEOUT_MS` (a positive
/// integer); anything else falls back to the default.
#[must_use]
pub fn dns_timeout() -> Duration {
    let ms = crate::system::read_env_var("IPE_HTTP_DNS_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DNS_TIMEOUT_MS_DEFAULT);
    Duration::from_millis(ms)
}

/// Strip a single surrounding `[`…`]` from an IPv6-literal host as it appears in
/// a URL authority (`Url::host_str()` returns `"[::1]"`, not `"::1"`). Returns the
/// inner slice when BOTH brackets are present, else the input unchanged — a plain
/// hostname or bare v4 literal is returned as-is (they carry no brackets), so a
/// subsequent `IpAddr::parse` still correctly fails for a hostname.
///
/// This is the single normalization point that puts every v6 URL host back on the
/// `is_private_ip` path; it is also used to key reqwest's `resolve_to_addrs`, whose
/// lookup key is the UNBRACKETED host (hyper's `Uri::host`).
pub(crate) fn strip_ipv6_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

fn refuse_blocked(host: &str, ip: IpAddr) -> Result<(), SsrfRefusal> {
    blocked_range(ip).map_or(Ok(()), |range| {
        Err(SsrfRefusal::Blocked {
            host: host.to_owned(),
            ip,
            range,
        })
    })
}

/// Every address a host passed the SSRF gate with — never empty.
///
/// The only constructor is [`vet_host_addrs_with`], so holding one proves
/// every address in it is outside every blocked range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VettedAddrs {
    first: SocketAddr,
    rest: Vec<SocketAddr>,
}

impl VettedAddrs {
    /// The address a single-address dialler uses.
    #[must_use]
    pub const fn first(&self) -> SocketAddr {
        self.first
    }

    /// Every vetted address, first one first.
    #[must_use]
    pub fn into_vec(self) -> Vec<SocketAddr> {
        let mut all = Vec::with_capacity(self.rest.len().saturating_add(1));
        all.push(self.first);
        all.extend(self.rest);
        all
    }
}

/// Resolve `host` once and return every address to dial, refusing any blocked one.
///
/// An IP literal (bracketed or not) is decided without a lookup. A name is
/// resolved through `resolver` within `deadline`; if ANY answer is blocked the
/// whole host is refused (a multi-record answer mixing public and private
/// addresses is ambiguous), otherwise every answer, carrying `port`, is
/// returned for the caller to dial directly.
///
/// # Errors
///
/// [`SsrfRefusal`] naming why the host was refused.
pub async fn vet_host_addrs_with<R: HostResolver>(
    resolver: &R,
    host: &str,
    port: u16,
    deadline: Duration,
) -> Result<VettedAddrs, SsrfRefusal> {
    // Parse-don't-validate at the boundary: a URL host taken from
    // `Url::host_str()` returns an IPv6 literal BRACKETED (`"[::1]"`). Strip a
    // single bracket pair and parse the IP literal FIRST, so every v6 literal
    // is decided by `blocked_range` instead of reaching the resolver.
    if let Ok(ip) = strip_ipv6_brackets(host).parse::<IpAddr>() {
        return refuse_blocked(host, ip).map(|()| VettedAddrs {
            first: SocketAddr::new(ip, port),
            rest: Vec::new(),
        });
    }
    let addrs = match tokio::time::timeout(deadline, resolver.lookup(host, port)).await {
        Ok(Ok(addrs)) => addrs,
        Ok(Err(e)) => {
            return Err(SsrfRefusal::Unresolvable {
                host: host.to_owned(),
                kind: e.kind(),
            });
        }
        Err(_elapsed) => {
            return Err(SsrfRefusal::Timeout {
                host: host.to_owned(),
                after: deadline,
            });
        }
    };
    for addr in &addrs {
        refuse_blocked(host, addr.ip())?;
    }
    let mut vetted = addrs
        .into_iter()
        .map(|addr| SocketAddr::new(addr.ip(), port));
    vetted.next().map_or_else(
        || {
            Err(SsrfRefusal::NoAddresses {
                host: host.to_owned(),
            })
        },
        |first| {
            Ok(VettedAddrs {
                first,
                rest: vetted.collect(),
            })
        },
    )
}

/// Resolve `host` once and return the address to dial, refusing any blocked one.
///
/// The first address [`vet_host_addrs_with`] vets, for a dialler that takes
/// one address.
///
/// # Errors
///
/// [`SsrfRefusal`] naming why the host was refused.
pub async fn vet_host_with<R: HostResolver>(
    resolver: &R,
    host: &str,
    port: u16,
    deadline: Duration,
) -> Result<SocketAddr, SsrfRefusal> {
    vet_host_addrs_with(resolver, host, port, deadline)
        .await
        .map(|vetted| vetted.first())
}

/// Resolve `host` once through the system resolver under [`dns_timeout`].
///
/// # Errors
///
/// [`SsrfRefusal`] naming why the host was refused.
pub async fn vet_host(host: &str, port: u16) -> Result<SocketAddr, SsrfRefusal> {
    vet_host_with(&SystemResolver, host, port, dns_timeout()).await
}

/// Proof that `host:port` passed the SSRF gate under the policy in effect.
///
/// The only constructors are [`VettedDial::for_host`],
/// [`VettedDial::for_host_with`], [`VettedDial::for_url`] and [`vet_url_with`].
/// Under [`DialPolicy::DenyPrivate`] the proof is the vetted address itself,
/// and the caller MUST dial that address rather than the name — dialling the
/// name resolves it again and reopens the DNS-rebinding window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(any(feature = "db", feature = "email", feature = "websocket_client")),
    allow(dead_code)
)]
pub(crate) enum VettedDial {
    /// Deny-private is on: dial exactly this address.
    Pinned(SocketAddr),
    /// Deny-private is off: the host may be dialled by name.
    Unrestricted,
}

#[cfg_attr(not(any(feature = "db", feature = "email")), allow(dead_code))]
impl VettedDial {
    /// Vet `host:port` under the environment's policy and the system resolver.
    pub(crate) async fn for_host(host: &str, port: u16) -> Result<Self, SsrfRefusal> {
        Self::for_host_with(DialPolicy::from_env(), &SystemResolver, host, port).await
    }

    /// Vet `host:port` under `policy`, resolving through `resolver`.
    pub(crate) async fn for_host_with<R: HostResolver>(
        policy: DialPolicy,
        resolver: &R,
        host: &str,
        port: u16,
    ) -> Result<Self, SsrfRefusal> {
        match policy {
            DialPolicy::DenyPrivate => vet_host_with(resolver, host, port, dns_timeout())
                .await
                .map(Self::Pinned),
            DialPolicy::AllowAll => Ok(Self::Unrestricted),
        }
    }

    /// Vet `url`'s host under the environment's policy and the system resolver.
    // Sole consumer is `ws_client.rs`. `cfg_attr`+`allow`, not `#[cfg]`:
    // generated projects include this module without declaring the
    // `websocket_client` Cargo feature, so a `#[cfg]` would remove the fn
    // from under a generated WebSocket caller.
    #[cfg_attr(not(feature = "websocket_client"), allow(dead_code))]
    pub(crate) async fn for_url(url: &str) -> Result<Self, UrlRefusal> {
        vet_url_with(DialPolicy::from_env(), &SystemResolver, url, dns_timeout()).await
    }

    /// The host string to hand a dialler: the vetted IP when pinned, else `host`.
    pub(crate) fn dial_host(self, host: &str) -> String {
        match self {
            Self::Pinned(addr) => addr.ip().to_string(),
            Self::Unrestricted => host.to_owned(),
        }
    }
}

/// The schemes an outbound surface may dial under deny-private.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatedSchemes {
    /// `http` and `https`: the HTTP request surface.
    Http,
    /// `http`, `https`, `ws` and `wss`.
    HttpAndWebSocket,
}

impl GatedSchemes {
    /// Every scheme this set admits.
    const fn schemes(self) -> &'static [&'static str] {
        match self {
            Self::Http => &["http", "https"],
            Self::HttpAndWebSocket => &["http", "https", "ws", "wss"],
        }
    }

    /// Whether `scheme` is one of this set's.
    fn admits(self, scheme: &str) -> bool {
        self.schemes().contains(&scheme)
    }
}

impl std::fmt::Display for GatedSchemes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.schemes().join("/"))
    }
}

/// The schemes a refusal may name.
///
/// `user:password@host` parses with the user name as its scheme, so a scheme
/// outside this well-known set is withheld rather than echoed.
const NAMEABLE_SCHEMES: [&str; 22] = [
    "http",
    "https",
    "ws",
    "wss",
    "ftp",
    "ftps",
    "sftp",
    "file",
    "data",
    "blob",
    "about",
    "javascript",
    "mailto",
    "tel",
    "gopher",
    "dict",
    "ldap",
    "ldaps",
    "ssh",
    "telnet",
    "postgres",
    "postgresql",
];

/// Stands in for a scheme that may be a user name or token.
const WITHHELD_SCHEME: &str = "<withheld: may be a user name>";

/// A URL's scheme as an error message may show it.
///
/// Holds no text read from the URL: only one of [`NAMEABLE_SCHEMES`] is named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemeShown {
    /// A well-known scheme.
    Known(&'static str),
    /// A scheme that may be part of the URL's credentials.
    Withheld,
}

impl SchemeShown {
    /// `scheme`, when it is well known; else withheld.
    pub(crate) fn of(scheme: &str) -> Self {
        NAMEABLE_SCHEMES
            .into_iter()
            .find(|known| known.eq_ignore_ascii_case(scheme))
            .map_or(Self::Withheld, Self::Known)
    }
}

impl std::fmt::Display for SchemeShown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(scheme) => write!(f, "{scheme:?}"),
            Self::Withheld => f.write_str(WITHHELD_SCHEME),
        }
    }
}

/// Why the SSRF gate refused a URL.
///
/// `Display` carries no caller prefix; each surface adds its own (`http:`,
/// `ws:`). No variant holds text read from the URL's credentials: an
/// unparseable URL is not shown at all, a scheme only through
/// [`SchemeShown`], and a host only when [`DisplayableUrl`] would show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlRefusal {
    /// The URL did not parse.
    Invalid {
        /// The parser's reason.
        reason: url::ParseError,
    },
    /// The scheme is not one the surface dials.
    Scheme {
        /// The URL's scheme, when it may be named.
        scheme: SchemeShown,
        /// The schemes the surface dials.
        admitted: GatedSchemes,
    },
    /// The URL names no host.
    NoHost,
    /// The URL's host was refused.
    Host(SsrfRefusal),
}

impl std::fmt::Display for UrlRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { reason } => {
                write!(f, "blocked: invalid URL: {reason} (IPE_HTTP_DENY_PRIVATE)")
            }
            Self::Scheme { scheme, admitted } => write!(
                f,
                "blocked: scheme {scheme} is not {admitted} (IPE_HTTP_DENY_PRIVATE)"
            ),
            Self::NoHost => f.write_str("blocked: URL has no host (IPE_HTTP_DENY_PRIVATE)"),
            Self::Host(refusal) => write!(f, "{refusal}"),
        }
    }
}

impl std::error::Error for UrlRefusal {}

/// Stands in for the part of a URL an error message cannot show safely.
const WITHHELD_URL_TAIL: &str = "<withheld: may hold credentials>";

/// Stands in for a refused host that may be part of a URL's credentials.
const WITHHELD_HOST: &str = "<withheld: may be part of the URL's credentials>";

/// Whether `raw`'s user name and password may run past what the parser read
/// as its userinfo.
///
/// Only the authority's last `@` separates the userinfo from the host, and the
/// authority ends at the first `/`, `?`, or `#` — and, for `http`, `https`,
/// `ws`, `wss`, `ftp`, and `file`, at `\`. A credential holding one of those
/// ends the authority early, and the parser reads the rest of it as host,
/// path, query, or fragment. The parser keeps `@` literal outside the
/// authority, so the userinfo is ambiguous when `parsed` holds an `@` there,
/// when `raw` holds a `\` under any scheme, or when `raw` does not parse and
/// holds an `@`.
pub(crate) fn userinfo_is_ambiguous(raw: &str, parsed: Option<&Url>) -> bool {
    raw.contains('\\')
        || parsed.map_or_else(
            || raw.contains('@'),
            |url| {
                url.path().contains('@')
                    || url.query().is_some_and(|query| query.contains('@'))
                    || url
                        .fragment()
                        .is_some_and(|fragment| fragment.contains('@'))
            },
        )
}

/// A URL as an error message or log line may show it.
///
/// Derived from the parse the gate dials, never from a second reading of the
/// text: the userinfo is removed, and everything after the scheme is withheld
/// when the URL does not parse, its scheme is not nameable, or its userinfo
/// is ambiguous ([`userinfo_is_ambiguous`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayableUrl(Shown);

/// How much of a URL [`DisplayableUrl`] shows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    /// The parsed URL with its userinfo removed.
    Whole(Url),
    /// Only the scheme, as [`SchemeShown`] allows.
    SchemeOnly(SchemeShown),
}

impl DisplayableUrl {
    /// `raw` as it may be shown, parsed with the gate's parser.
    // Sole consumer is `ws_client.rs`; see `VettedDial::for_url`.
    #[cfg_attr(not(feature = "websocket_client"), allow(dead_code))]
    pub(crate) fn of(raw: &str) -> Self {
        Self::of_parsed(raw, Url::parse(raw).ok().as_ref())
    }

    /// `raw` as it may be shown, given the gate's parse of it.
    pub(crate) fn of_parsed(raw: &str, parsed: Option<&Url>) -> Self {
        let Some(parsed) = parsed else {
            return Self(Shown::SchemeOnly(SchemeShown::Withheld));
        };
        let scheme = SchemeShown::of(parsed.scheme());
        if scheme == SchemeShown::Withheld || userinfo_is_ambiguous(raw, Some(parsed)) {
            return Self(Shown::SchemeOnly(scheme));
        }
        let mut whole = parsed.clone();
        if whole.set_password(None).is_err() || whole.set_username("").is_err() {
            return Self(Shown::SchemeOnly(scheme));
        }
        Self(Shown::Whole(whole))
    }

    /// Whether the host the parser read is shown, so it cannot be part of the
    /// URL's credentials.
    pub(crate) const fn shows_host(&self) -> bool {
        matches!(self.0, Shown::Whole(_))
    }
}

impl std::fmt::Display for DisplayableUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Shown::Whole(url) => write!(f, "{url}"),
            Shown::SchemeOnly(SchemeShown::Known(scheme)) => {
                write!(f, "{scheme}:{WITHHELD_URL_TAIL}")
            }
            Shown::SchemeOnly(SchemeShown::Withheld) => f.write_str(WITHHELD_URL_TAIL),
        }
    }
}

/// `refusal` for `raw`, naming its host only when [`DisplayableUrl`] shows it.
fn url_host_refusal(raw: &str, parsed: &Url, refusal: SsrfRefusal) -> UrlRefusal {
    UrlRefusal::Host(
        if DisplayableUrl::of_parsed(raw, Some(parsed)).shows_host() {
            refusal
        } else {
            refusal.withholding_host()
        },
    )
}

/// Parse `url` and admit only a scheme in `admitted`.
///
/// # Errors
///
/// [`UrlRefusal::Invalid`] or [`UrlRefusal::Scheme`].
pub(crate) fn parse_gated_url_within(url: &str, admitted: GatedSchemes) -> Result<Url, UrlRefusal> {
    let parsed = Url::parse(url).map_err(|reason| UrlRefusal::Invalid { reason })?;
    if admitted.admits(parsed.scheme()) {
        Ok(parsed)
    } else {
        Err(UrlRefusal::Scheme {
            scheme: SchemeShown::of(parsed.scheme()),
            admitted,
        })
    }
}

/// Parse `url` and admit only `http`, `https`, `ws`, or `wss`.
///
/// # Errors
///
/// [`UrlRefusal::Invalid`] or [`UrlRefusal::Scheme`].
pub(crate) fn parse_gated_url(url: &str) -> Result<Url, UrlRefusal> {
    parse_gated_url_within(url, GatedSchemes::HttpAndWebSocket)
}

/// Vet `url` under `policy`, resolving its host through `resolver` within `deadline`.
///
/// Under [`DialPolicy::DenyPrivate`] the URL must parse, carry a gated scheme
/// and a host, and the host must pass [`vet_host_with`]; the proof pins the
/// vetted address with the URL's port. Under [`DialPolicy::AllowAll`] the URL
/// is dialled as given.
///
/// # Errors
///
/// [`UrlRefusal`] naming why the URL was refused.
#[cfg_attr(not(feature = "websocket_client"), allow(dead_code))]
pub(crate) async fn vet_url_with<R: HostResolver>(
    policy: DialPolicy,
    resolver: &R,
    url: &str,
    deadline: Duration,
) -> Result<VettedDial, UrlRefusal> {
    match policy {
        DialPolicy::AllowAll => Ok(VettedDial::Unrestricted),
        DialPolicy::DenyPrivate => {
            let parsed = parse_gated_url(url)?;
            let host = parsed.host_str().ok_or(UrlRefusal::NoHost)?;
            // Every gated scheme has a known default port, so the fallback is
            // unreachable; port 0 would fail the dial, never reach a service.
            let port = parsed.port_or_known_default().unwrap_or(0);
            vet_host_with(resolver, host, port, deadline)
                .await
                .map(VettedDial::Pinned)
                .map_err(|refusal| url_host_refusal(url, &parsed, refusal))
        }
    }
}

/// Non-blocking redirect-hop guard: a URL's scheme and, for an IP-literal host, its range.
///
/// A named host is vetted by the HTTP client's connect-time resolver, which
/// runs the same [`vet_host_addrs_with`] gate, so re-resolving here would only
/// add a lookup inside reqwest's sync redirect closure. IP-literal redirect
/// targets bypass the resolver, so they MUST still be range-checked here —
/// that check is pure and non-blocking.
///
/// # Errors
///
/// [`UrlRefusal`] naming why the hop was refused.
// Only `http_client::ssrf_apply`'s reqwest redirect closure calls this; the `ssrf`
// module also compiles under `db`/`websocket_client` where that caller is absent.
#[cfg_attr(not(feature = "http_client"), allow(dead_code))]
pub(crate) fn ssrf_check_url_nonblocking(url: &str) -> Result<(), UrlRefusal> {
    let parsed = parse_gated_url(url)?;
    let host = parsed.host_str().ok_or(UrlRefusal::NoHost)?;
    // Only IP literals are decided here (no DNS); a named host defers to the
    // connect-time resolver. `strip_ipv6_brackets` puts a `[::1]`-style literal
    // back on the `blocked_range` path; a hostname simply fails the parse.
    strip_ipv6_brackets(host)
        .parse::<IpAddr>()
        .map_or(Ok(()), |ip| {
            refuse_blocked(host, ip).map_err(|refusal| url_host_refusal(url, &parsed, refusal))
        })
}

/// Stub resolvers for tests of every SSRF-gated surface — no network.
#[cfg(test)]
pub(crate) mod test_resolvers {
    use super::HostResolver;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A resolver that must never be consulted: every lookup fails.
    pub struct NoDns;

    impl HostResolver for NoDns {
        async fn lookup(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }

    /// A rebinding resolver: the first lookup answers a public address, every
    /// later lookup answers a private one.
    #[derive(Default)]
    pub struct PublicThenPrivate {
        /// Lookups answered so far.
        pub calls: AtomicUsize,
    }

    impl PublicThenPrivate {
        /// The first answer.
        pub const PUBLIC: IpAddr = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        /// Every later answer.
        pub const PRIVATE: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));

        /// A resolver that has answered nothing yet.
        pub const fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }

        /// Lookups answered so far.
        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl HostResolver for PublicThenPrivate {
        async fn lookup(&self, _host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
            let ip = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Self::PUBLIC
            } else {
                Self::PRIVATE
            };
            Ok(vec![SocketAddr::new(ip, port)])
        }
    }

    /// A resolver that answers a fixed address list.
    pub struct Answers(pub Vec<IpAddr>);

    impl HostResolver for Answers {
        async fn lookup(&self, _host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
            Ok(self.0.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
        }
    }

    /// A resolver that never answers.
    pub struct Stalls;

    impl HostResolver for Stalls {
        async fn lookup(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            std::future::pending().await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_resolvers::{Answers, NoDns, PublicThenPrivate, Stalls};
    use super::*;

    const DEADLINE: Duration = Duration::from_secs(5);

    /// The URL gate's deny-private verdict, rendered as the `http:` surface shows it.
    async fn ssrf_check_url(url: &str) -> Result<VettedDial, String> {
        vet_url_with(DialPolicy::DenyPrivate, &NoDns, url, DEADLINE)
            .await
            .map_err(|refusal| format!("http: {refusal}"))
    }

    /// The redirect-hop guard's verdict, rendered as the `http:` surface shows it.
    fn nonblocking_hop(url: &str) -> Result<(), String> {
        ssrf_check_url_nonblocking(url).map_err(|refusal| format!("http: {refusal}"))
    }

    // -----------------------------------------------------------------------
    // vet_url_with — the WebSocket client's single gate
    // -----------------------------------------------------------------------

    /// DNS rebinding through the URL gate: the dial is pinned to the first
    /// answer with the URL's port, and a fresh vet meets the rebound answer
    /// and is refused, so the private address is never handed to a dialler.
    #[tokio::test]
    async fn vet_url_pins_the_first_answer_against_rebinding() {
        let resolver = PublicThenPrivate::new();
        let url = "ws://rebind.example:9000/socket";
        let first = vet_url_with(DialPolicy::DenyPrivate, &resolver, url, DEADLINE).await;
        assert_eq!(
            first,
            Ok(VettedDial::Pinned(SocketAddr::new(
                PublicThenPrivate::PUBLIC,
                9000
            )))
        );
        assert_eq!(resolver.calls(), 1, "the name must be resolved once");
        let second = vet_url_with(DialPolicy::DenyPrivate, &resolver, url, DEADLINE).await;
        assert_eq!(
            second,
            Err(UrlRefusal::Host(SsrfRefusal::Blocked {
                host: "rebind.example".to_owned(),
                ip: PublicThenPrivate::PRIVATE,
                range: BlockedRange::Private,
            }))
        );
    }

    /// A mixed public and private answer refuses the whole URL.
    #[tokio::test]
    async fn vet_url_refuses_a_mixed_answer() {
        let mixed = Answers(vec![
            PublicThenPrivate::PUBLIC,
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        ]);
        let refused = vet_url_with(
            DialPolicy::DenyPrivate,
            &mixed,
            "wss://mixed.example/",
            DEADLINE,
        )
        .await;
        assert!(
            matches!(
                refused,
                Err(UrlRefusal::Host(SsrfRefusal::Blocked {
                    range: BlockedRange::Loopback,
                    ..
                }))
            ),
            "{refused:?}"
        );
    }

    /// A stalled resolver is cut off with a typed timeout.
    #[tokio::test]
    async fn vet_url_times_out_a_stalled_resolver() {
        let deadline = Duration::from_millis(20);
        let refused = vet_url_with(
            DialPolicy::DenyPrivate,
            &Stalls,
            "ws://slow.example/",
            deadline,
        )
        .await;
        assert_eq!(
            refused,
            Err(UrlRefusal::Host(SsrfRefusal::Timeout {
                host: "slow.example".to_owned(),
                after: deadline,
            }))
        );
    }

    /// Each malformed URL is refused with its own typed reason.
    #[tokio::test]
    async fn vet_url_refuses_each_malformed_url() {
        let scheme = vet_url_with(
            DialPolicy::DenyPrivate,
            &NoDns,
            "ftp://x.example/",
            DEADLINE,
        )
        .await;
        assert_eq!(
            scheme,
            Err(UrlRefusal::Scheme {
                scheme: SchemeShown::Known("ftp"),
                admitted: GatedSchemes::HttpAndWebSocket,
            })
        );
        let invalid = vet_url_with(DialPolicy::DenyPrivate, &NoDns, "not a url", DEADLINE).await;
        assert!(
            matches!(invalid, Err(UrlRefusal::Invalid { .. })),
            "{invalid:?}"
        );
    }

    /// An unparseable URL's refusal never carries its userinfo.
    #[tokio::test]
    async fn vet_url_invalid_refusal_redacts_userinfo() {
        let url = "http://admin:s3cr3t-pw@exa mple.com/";
        let refused = vet_url_with(DialPolicy::DenyPrivate, &NoDns, url, DEADLINE).await;
        assert!(
            matches!(refused, Err(UrlRefusal::Invalid { .. })),
            "{refused:?}"
        );
        let shown = format!(
            "{refused:?} {}",
            refused.map_or_else(|r| r.to_string(), |_| String::new())
        );
        assert!(!shown.contains("s3cr3t-pw"), "password leaked: {shown}");
        assert!(!shown.contains("admin"), "user leaked: {shown}");
    }

    /// With the policy off, the URL is dialled as given and no lookup runs.
    #[tokio::test]
    async fn vet_url_is_unrestricted_when_the_policy_allows_all() {
        let resolver = PublicThenPrivate::new();
        for url in ["ws://127.0.0.1/", "ws://internal.example/", "not a url"] {
            let vetted = vet_url_with(DialPolicy::AllowAll, &resolver, url, DEADLINE).await;
            assert_eq!(vetted, Ok(VettedDial::Unrestricted), "{url:?}");
        }
        assert_eq!(resolver.calls(), 0);
    }

    /// Every vetted address comes back, each carrying the caller's port.
    #[tokio::test]
    async fn vet_host_addrs_returns_every_public_answer() {
        let second = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        let vetted = vet_host_addrs_with(
            &Answers(vec![PublicThenPrivate::PUBLIC, second]),
            "multi.example",
            443,
            DEADLINE,
        )
        .await
        .map(VettedAddrs::into_vec);
        assert_eq!(
            vetted,
            Ok(vec![
                SocketAddr::new(PublicThenPrivate::PUBLIC, 443),
                SocketAddr::new(second, 443)
            ])
        );
    }

    // -----------------------------------------------------------------------
    // VettedDial
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn vetted_dial_refuses_every_blocked_literal_under_deny_private() {
        for (host, range) in [
            ("127.0.0.1", BlockedRange::Loopback),
            ("::1", BlockedRange::Loopback),
            ("169.254.169.254", BlockedRange::LinkLocal),
            ("10.0.0.5", BlockedRange::Private),
            ("0.0.0.0", BlockedRange::Reserved),
        ] {
            let refused =
                VettedDial::for_host_with(DialPolicy::DenyPrivate, &NoDns, host, 5432).await;
            assert!(
                matches!(refused, Err(SsrfRefusal::Blocked { range: r, .. }) if r == range),
                "{host:?} must be refused as {range}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn vetted_dial_pins_a_public_literal_with_its_port() {
        let vetted =
            VettedDial::for_host_with(DialPolicy::DenyPrivate, &NoDns, "1.1.1.1", 5432).await;
        assert_eq!(
            vetted,
            Ok(VettedDial::Pinned(SocketAddr::new(
                PublicThenPrivate::PUBLIC,
                5432
            )))
        );
    }

    #[tokio::test]
    async fn vetted_dial_is_unrestricted_when_the_policy_allows_all() {
        for host in ["127.0.0.1", "10.0.0.1", "internal.example"] {
            let vetted = VettedDial::for_host_with(DialPolicy::AllowAll, &NoDns, host, 5432).await;
            assert_eq!(vetted, Ok(VettedDial::Unrestricted), "{host:?}");
            assert_eq!(VettedDial::Unrestricted.dial_host(host), host);
        }
    }

    /// DNS rebinding: the name is resolved once and the dial is pinned to that
    /// answer, so a later answer pointing at a private host is never dialled.
    /// A fresh vet (a new dial) sees the rebound answer and is refused.
    #[tokio::test]
    async fn vetted_dial_pins_the_first_answer_against_rebinding() {
        let resolver = PublicThenPrivate::new();
        let first =
            VettedDial::for_host_with(DialPolicy::DenyPrivate, &resolver, "rebind.example", 5432)
                .await;
        assert_eq!(
            first,
            Ok(VettedDial::Pinned(SocketAddr::new(
                PublicThenPrivate::PUBLIC,
                5432
            )))
        );
        assert_eq!(
            first.map(|dial| dial.dial_host("rebind.example")),
            Ok(PublicThenPrivate::PUBLIC.to_string()),
            "the dialler must receive the vetted address, never the name"
        );
        assert_eq!(resolver.calls(), 1);

        let second =
            VettedDial::for_host_with(DialPolicy::DenyPrivate, &resolver, "rebind.example", 5432)
                .await;
        assert_eq!(
            second,
            Err(SsrfRefusal::Blocked {
                host: "rebind.example".to_string(),
                ip: PublicThenPrivate::PRIVATE,
                range: BlockedRange::Private,
            })
        );
    }

    /// A multi-record answer mixing public and private addresses is refused
    /// whole rather than trusting whichever one a dialler would pick.
    #[tokio::test]
    async fn vet_host_refuses_a_mixed_public_private_answer() {
        let mixed = Answers(vec![
            PublicThenPrivate::PUBLIC,
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
        ]);
        let refused = vet_host_with(&mixed, "mixed.example", 443, DEADLINE).await;
        assert!(
            matches!(
                refused,
                Err(SsrfRefusal::Blocked {
                    range: BlockedRange::LinkLocal,
                    ..
                })
            ),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn vet_host_refuses_an_empty_answer() {
        let refused = vet_host_with(&Answers(Vec::new()), "empty.example", 443, DEADLINE).await;
        assert_eq!(
            refused,
            Err(SsrfRefusal::NoAddresses {
                host: "empty.example".to_string()
            })
        );
    }

    #[tokio::test]
    async fn vet_host_refuses_an_unresolvable_name() {
        let refused = vet_host_with(&NoDns, "nowhere.example", 443, DEADLINE).await;
        assert_eq!(
            refused,
            Err(SsrfRefusal::Unresolvable {
                host: "nowhere.example".to_string(),
                kind: std::io::ErrorKind::NotFound,
            })
        );
    }

    /// A resolver that never answers is cut off at the deadline with a typed
    /// refusal instead of holding the task.
    #[tokio::test]
    async fn vet_host_times_out_a_stalled_resolver() {
        let deadline = Duration::from_millis(20);
        let refused = vet_host_with(&Stalls, "slow.example", 443, deadline).await;
        assert_eq!(
            refused,
            Err(SsrfRefusal::Timeout {
                host: "slow.example".to_string(),
                after: deadline,
            })
        );
    }

    #[tokio::test]
    async fn vet_host_returns_the_first_public_answer_with_the_callers_port() {
        let resolved = vet_host_with(
            &Answers(vec![PublicThenPrivate::PUBLIC]),
            "public.example",
            8443,
            DEADLINE,
        )
        .await;
        assert_eq!(
            resolved,
            Ok(SocketAddr::new(PublicThenPrivate::PUBLIC, 8443))
        );
    }

    /// Every refusal names what was refused and the policy that refused it,
    /// with no caller prefix of its own.
    #[test]
    fn every_ssrf_refusal_displays_its_reason_without_a_caller_prefix() {
        let cases = [
            (
                SsrfRefusal::Blocked {
                    host: "127.0.0.1".to_string(),
                    ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    range: BlockedRange::Loopback,
                },
                "loopback host 127.0.0.1",
            ),
            (
                SsrfRefusal::Blocked {
                    host: "meta.example".to_string(),
                    ip: IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
                    range: BlockedRange::LinkLocal,
                },
                "resolved to link-local address 169.254.169.254",
            ),
            (
                SsrfRefusal::Unresolvable {
                    host: "nowhere.example".to_string(),
                    kind: std::io::ErrorKind::NotFound,
                },
                "could not resolve host \"nowhere.example\"",
            ),
            (
                SsrfRefusal::NoAddresses {
                    host: "empty.example".to_string(),
                },
                "resolved to no addresses",
            ),
            (
                SsrfRefusal::Timeout {
                    host: "slow.example".to_string(),
                    after: Duration::from_millis(250),
                },
                "timed out after 250 ms",
            ),
            (SsrfRefusal::LocalSocket, "local socket"),
            (SsrfRefusal::UnprovenTarget, "names no host"),
            (
                SsrfRefusal::UnpinnableTlsName {
                    host: "db.example".to_string(),
                },
                "verify-full",
            ),
        ];
        for (refusal, reason) in cases {
            let shown = refusal.to_string();
            assert!(shown.starts_with("blocked: "), "{shown}");
            assert!(shown.contains(reason), "{shown} must contain {reason:?}");
            assert!(shown.ends_with("(IPE_HTTP_DENY_PRIVATE)"), "{shown}");
            for prefix in ["http:", "db:", "ws:"] {
                assert!(!shown.contains(prefix), "{shown} carries a caller prefix");
            }
        }
    }

    #[test]
    fn blocked_range_classifies_each_class() {
        let class = |ip: &str| blocked_range(ip.parse().unwrap());
        assert_eq!(class("127.0.0.1"), Some(BlockedRange::Loopback));
        assert_eq!(class("::1"), Some(BlockedRange::Loopback));
        assert_eq!(class("::ffff:127.0.0.1"), Some(BlockedRange::Loopback));
        assert_eq!(class("169.254.169.254"), Some(BlockedRange::LinkLocal));
        assert_eq!(class("fe80::1"), Some(BlockedRange::LinkLocal));
        assert_eq!(class("64:ff9b::a9fe:a9fe"), Some(BlockedRange::LinkLocal));
        assert_eq!(class("10.0.0.1"), Some(BlockedRange::Private));
        assert_eq!(class("100.64.0.1"), Some(BlockedRange::Private));
        assert_eq!(class("fd00::1"), Some(BlockedRange::Private));
        assert_eq!(class("2002:c0a8:101::1"), Some(BlockedRange::Private));
        assert_eq!(class("0.0.0.0"), Some(BlockedRange::Reserved));
        assert_eq!(class("::"), Some(BlockedRange::Reserved));
        assert_eq!(class("198.18.0.1"), Some(BlockedRange::Reserved));
        assert_eq!(class("255.255.255.255"), Some(BlockedRange::Reserved));
        assert_eq!(class("1.1.1.1"), None);
        assert_eq!(class("2606:4700:4700::1111"), None);
    }

    // -----------------------------------------------------------------------
    // is_private_ip
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

    #[tokio::test]
    async fn ssrf_check_url_rejects_non_http_scheme() {
        let err = ssrf_check_url("ftp://example.com/file").await.unwrap_err();
        assert!(
            err.contains("scheme"),
            "expected scheme rejection, got: {err}"
        );
        let err2 = ssrf_check_url("file:///etc/passwd").await.unwrap_err();
        assert!(
            err2.contains("scheme") || err2.contains("invalid"),
            "got: {err2}"
        );
    }

    #[tokio::test]
    async fn ssrf_check_url_rejects_private_ip_literal() {
        let err = ssrf_check_url("http://192.168.1.1/secret")
            .await
            .unwrap_err();
        assert!(
            err.starts_with("http: blocked"),
            "expected blocked, got: {err}"
        );
    }

    #[tokio::test]
    async fn ssrf_check_url_rejects_loopback_ip_literal() {
        let err = ssrf_check_url("http://127.0.0.1:8080/admin")
            .await
            .unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    #[tokio::test]
    async fn ssrf_check_url_rejects_aws_imds() {
        let err = ssrf_check_url("http://169.254.169.254/latest/meta-data/")
            .await
            .unwrap_err();
        assert!(err.contains("blocked"), "expected blocked, got: {err}");
    }

    #[tokio::test]
    async fn ssrf_check_url_rejects_invalid_url() {
        let err = ssrf_check_url("not a url at all").await.unwrap_err();
        assert!(!err.is_empty());
    }

    /// Every internal redirect-hop target is refused by the URL gate.
    #[tokio::test]
    async fn redirect_hop_revalidation_blocks_internal_targets() {
        for hop in [
            "http://127.0.0.1/admin",             // loopback
            "http://10.0.0.5/internal",           // RFC-1918 private
            "http://169.254.169.254/latest/meta", // AWS IMDS link-local
            "http://[::1]/",                      // v6 loopback
            "http://192.168.1.1/",                // RFC-1918 private
        ] {
            let err = ssrf_check_url(hop)
                .await
                .expect_err("a redirect hop to an internal address must be blocked");
            assert!(err.contains("blocked"), "hop {hop:?} → got: {err}");
        }
        // A redirect hop with a non-http(s) scheme is blocked at the same floor.
        let err = ssrf_check_url("ftp://example.com/x")
            .await
            .expect_err("a non-http(s) redirect hop must be blocked");
        assert!(err.contains("scheme"), "got: {err}");
    }

    #[tokio::test]
    async fn ssrf_check_url_allows_public_ip() {
        // 1.1.1.1 is public — should pass (no DNS needed for IP literals)
        assert!(ssrf_check_url("https://1.1.1.1/").await.is_ok());
    }

    // -----------------------------------------------------------------------
    // vet_host_with — IP literals are decided without the resolver
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn vet_host_returns_socket_addr_for_public_ip_literal() {
        let sa = vet_host_with(&NoDns, "1.1.1.1", 0, DEADLINE).await.unwrap();
        assert_eq!(sa, SocketAddr::new(PublicThenPrivate::PUBLIC, 0));
    }

    #[tokio::test]
    async fn vet_host_rejects_blocked_ip_literals() {
        for host in ["192.168.1.1", "127.0.0.1", "::ffff:127.0.0.1"] {
            let refused = vet_host_with(&NoDns, host, 0, DEADLINE).await;
            assert!(
                matches!(refused, Err(SsrfRefusal::Blocked { .. })),
                "{host:?}: {refused:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Bracketed IPv6-literal hosts (as returned by `Url::host_str()`) must be
    // decided by `blocked_range`, never reach the resolver: `NoDns` would turn
    // a resolver fallthrough into `Unresolvable`, not `Blocked`.
    // -----------------------------------------------------------------------

    async fn assert_blocked_by_range(host: &str) {
        let refused = vet_host_with(&NoDns, host, 0, DEADLINE).await;
        assert!(
            matches!(refused, Err(SsrfRefusal::Blocked { .. })),
            "host {host:?} must be blocked by blocked_range (literal path), got: {refused:?}"
        );
    }

    #[test]
    fn strip_ipv6_brackets_unwraps_only_a_matched_pair() {
        assert_eq!(strip_ipv6_brackets("[::1]"), "::1");
        assert_eq!(strip_ipv6_brackets("[fd00::1]"), "fd00::1");
        // No brackets / bare v4 / hostname pass through untouched.
        assert_eq!(strip_ipv6_brackets("::1"), "::1");
        assert_eq!(strip_ipv6_brackets("127.0.0.1"), "127.0.0.1");
        assert_eq!(strip_ipv6_brackets("example.com"), "example.com");
        // A lone bracket is not a pair — left as-is (still fails IpAddr::parse).
        assert_eq!(strip_ipv6_brackets("[::1"), "[::1");
        assert_eq!(strip_ipv6_brackets("::1]"), "::1]");
    }

    #[tokio::test]
    async fn vet_host_blocks_bracketed_private_v6_literals() {
        for host in [
            "[::1]",
            "[fd00::1]",
            "[fe80::1]",
            "[::ffff:127.0.0.1]",
            // 64:ff9b::7f00:1 → 127.0.0.1 (NAT64-wrapped loopback).
            "[64:ff9b::7f00:1]",
        ] {
            assert_blocked_by_range(host).await;
        }
    }

    #[tokio::test]
    async fn vet_host_allows_bracketed_public_v6() {
        // Cloudflare public v6, bracketed as a URL host would present it.
        let sa = vet_host_with(&NoDns, "[2606:4700:4700::1111]", 0, DEADLINE)
            .await
            .expect("public bracketed v6 literal must be allowed");
        assert_eq!(
            sa.ip(),
            "2606:4700:4700::1111".parse::<IpAddr>().unwrap(),
            "the vetted addr must be the unbracketed parsed literal"
        );
    }

    #[tokio::test]
    async fn ssrf_check_url_blocks_bracketed_v6_loopback_and_ula() {
        // The URL entrypoint (host_str returns the bracketed form) must block.
        for url in [
            "http://[::1]/admin",
            "http://[fd00::1]/x",
            "https://[fe80::1]/",
        ] {
            let err = ssrf_check_url(url)
                .await
                .expect_err("bracketed private v6 URL must be blocked");
            assert!(err.contains("blocked"), "url {url:?} → got: {err}");
        }
    }

    #[tokio::test]
    async fn ssrf_check_url_allows_bracketed_public_v6() {
        assert!(
            ssrf_check_url("https://[2606:4700:4700::1111]/")
                .await
                .is_ok()
        );
    }

    // -----------------------------------------------------------------------
    // #2536 item 1 — the non-blocking redirect-hop guard. It decides IP
    // literals (incl. bracketed v6) purely and defers named hosts to the
    // connect-time resolver (so a hostname is NOT rejected here for lack of DNS).
    // -----------------------------------------------------------------------

    #[test]
    fn nonblocking_check_blocks_private_ip_literals() {
        for url in [
            "http://127.0.0.1/admin",
            "http://10.0.0.5/x",
            "http://169.254.169.254/latest/meta",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[64:ff9b::7f00:1]/", // NAT64 → 127.0.0.1
        ] {
            let err = nonblocking_hop(url)
                .expect_err("private IP-literal hop must be blocked without DNS");
            assert!(err.contains("blocked"), "url {url:?} → got: {err}");
        }
    }

    #[test]
    fn nonblocking_check_rejects_non_http_scheme() {
        let err = nonblocking_hop("ftp://example.com/x").unwrap_err();
        assert!(err.contains("scheme"), "got: {err}");
    }

    #[test]
    fn nonblocking_check_allows_public_ip_and_defers_hostnames() {
        // Public IP literal: allowed. Hostname: deferred (no DNS here) → Ok.
        assert!(nonblocking_hop("https://1.1.1.1/").is_ok());
        assert!(nonblocking_hop("https://[2606:4700:4700::1111]/").is_ok());
        assert!(nonblocking_hop("https://example.com/path").is_ok());
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

    /// URLs whose credentials an error message must never show, each with the
    /// secret substrings it holds: userinfo the parser reads, userinfo cut
    /// short by a `/`, `?`, `#`, or `\`, and a user name parsed as the scheme.
    const CREDENTIAL_URLS: [(&str, &[&str]); 14] = [
        (
            "ws://alice:s3cr3t@example.com:9000/feed",
            &["alice", "s3cr3t"],
        ),
        ("http://admin\\s3cr3t@public.example/", &["admin", "s3cr3t"]),
        ("ftp://admin\\s3cr3t@public.example/", &["admin", "s3cr3t"]),
        ("admin:s3cr3t@host", &["admin", "s3cr3t"]),
        ("admin:s3cr3t@host/x", &["admin", "s3cr3t"]),
        ("apikey123:x@api.example", &["apikey123"]),
        ("ws://admin:5432/s3cr3t@db.example", &["admin", "s3cr3t"]),
        ("http://admin@s3cr3t/-pw@host/x", &["admin", "s3cr3t"]),
        ("https://admin:s3cr3t?pw@host", &["admin", "s3cr3t"]),
        ("wss://admin#s3cr3t@host", &["admin", "s3cr3t"]),
        ("ws://bob:s3cr3t@@@host/a@b", &["bob", "s3cr3t"]),
        ("admin/s3cr3t@host", &["admin", "s3cr3t"]),
        ("http://admin:s3cr3t@[bad/x", &["admin", "s3cr3t"]),
        ("http://127.0.0.1/admin@s3cr3t", &["admin", "s3cr3t"]),
    ];

    /// Asserts neither `Display` nor `Debug` of `value` holds any of `secrets`.
    fn assert_shows_none_of<T: std::fmt::Display + std::fmt::Debug>(value: &T, secrets: &[&str]) {
        for shown in [value.to_string(), format!("{value:?}")] {
            for secret in secrets {
                assert!(!shown.contains(secret), "{secret:?} leaked into {shown}");
            }
        }
    }

    /// No rendering of a displayable URL holds its credentials.
    #[test]
    fn displayable_url_never_shows_credentials() {
        for (url, secrets) in CREDENTIAL_URLS {
            assert_shows_none_of(&DisplayableUrl::of(url), secrets);
        }
    }

    /// A URL whose userinfo the parser reads unambiguously keeps its host,
    /// port, path, and query, with the userinfo removed.
    #[test]
    fn displayable_url_strips_the_parsed_userinfo() {
        for (url, shown) in [
            (
                "ws://alice:s3cr3t@example.com:9000/feed?token=abc",
                "ws://example.com:9000/feed?token=abc",
            ),
            ("wss://admin@host/x", "wss://host/x"),
            ("ws://example.com/feed", "ws://example.com/feed"),
            ("http://u:p@@@host", "http://host/"),
        ] {
            let displayable = DisplayableUrl::of(url);
            assert_eq!(displayable.to_string(), shown, "{url:?}");
            assert!(displayable.shows_host(), "{url:?}");
        }
    }

    /// A credential cut short by a `/`, `?`, `#`, or `\` leaves an `@` or `\`
    /// the parser did not read as the userinfo's end, so only the scheme is
    /// shown; an unparseable URL or an unnameable scheme shows nothing.
    #[test]
    fn displayable_url_withholds_an_ambiguous_url() {
        for (url, scheme) in [
            ("http://admin\\s3cr3t@public.example/", "http"),
            ("ftp://admin\\s3cr3t@public.example/", "ftp"),
            ("http://admin\\no-at.example/", "http"),
            ("ws://admin:5432/s3cr3t@db.example", "ws"),
            ("http://admin@s3cr3t/-pw@host/x", "http"),
            ("wss://admin#s3cr3t@host", "wss"),
            ("ws://bob:s3cr3t@@@host/a@b", "ws"),
            ("http://127.0.0.1/admin@s3cr3t", "http"),
            ("https://public.example/?user=admin@s3cr3t", "https"),
        ] {
            let displayable = DisplayableUrl::of(url);
            assert_eq!(
                displayable.to_string(),
                format!("{scheme}:{WITHHELD_URL_TAIL}"),
                "{url:?}"
            );
            assert!(!displayable.shows_host(), "{url:?}");
        }
        for url in [
            "admin:s3cr3t@host",
            "apikey123:x@api.example",
            "admin/s3cr3t@host",
            "https://admin:s3cr3t?pw@host",
            "not a url",
        ] {
            let displayable = DisplayableUrl::of(url);
            assert_eq!(displayable.to_string(), WITHHELD_URL_TAIL, "{url:?}");
            assert!(!displayable.shows_host(), "{url:?}");
        }
    }

    /// A user name or token parsed as the scheme is refused without being
    /// named; a well-known scheme is named with the schemes the surface dials.
    #[test]
    fn scheme_refusal_names_only_a_well_known_scheme() {
        for (url, secrets) in [
            ("admin:s3cr3t@host", ["admin", "s3cr3t"]),
            ("admin:s3cr3t@host/x", ["admin", "s3cr3t"]),
            ("apikey123:x@api.example", ["apikey123", "api.example"]),
        ] {
            for admitted in [GatedSchemes::Http, GatedSchemes::HttpAndWebSocket] {
                let refused = parse_gated_url_within(url, admitted).err();
                assert_eq!(
                    refused,
                    Some(UrlRefusal::Scheme {
                        scheme: SchemeShown::Withheld,
                        admitted,
                    }),
                    "{url:?}"
                );
                if let Some(refused) = refused {
                    assert_shows_none_of(&refused, &secrets);
                }
            }
        }
        let ws = parse_gated_url_within("ws://example.com/", GatedSchemes::Http).err();
        assert_eq!(
            ws.as_ref().map(ToString::to_string),
            Some("blocked: scheme \"ws\" is not http/https (IPE_HTTP_DENY_PRIVATE)".to_owned())
        );
        let ftp = parse_gated_url("ftp://example.com/").err();
        assert_eq!(
            ftp.as_ref().map(ToString::to_string),
            Some(
                "blocked: scheme \"ftp\" is not http/https/ws/wss (IPE_HTTP_DENY_PRIVATE)"
                    .to_owned()
            )
        );
    }

    /// No refusal of a gated URL holds its credentials, whether the parser
    /// rejects it, refuses its scheme, or refuses its host.
    #[tokio::test]
    async fn url_refusals_never_show_credentials() {
        for (url, secrets) in CREDENTIAL_URLS {
            let vetted = vet_url_with(DialPolicy::DenyPrivate, &NoDns, url, DEADLINE).await;
            if let Err(refused) = vetted {
                assert_shows_none_of(&refused, secrets);
            }
            if let Err(refused) = ssrf_check_url_nonblocking(url) {
                assert_shows_none_of(&refused, secrets);
            }
        }
    }

    /// An unparseable URL is refused naming only the parser's reason.
    #[test]
    fn invalid_url_refusal_never_echoes_userinfo() {
        for url in ["http://admin:s3cr3t@[bad/x", "http://admin:s3cr3t/@[bad"] {
            let refused = parse_gated_url(url).err();
            assert!(
                matches!(refused, Some(UrlRefusal::Invalid { .. })),
                "{url:?}: {refused:?}"
            );
            if let Some(refused) = refused {
                assert_shows_none_of(&refused, &["admin", "s3cr3t"]);
            }
        }
    }

    /// When the URL's userinfo is ambiguous, the host the parser read may be
    /// part of a credential, so a host refusal withholds it.
    #[tokio::test]
    async fn host_refusal_withholds_a_host_that_may_be_a_credential() {
        for url in [
            "http://admin:80/s3cr3t@public.example/",
            "http://admin\\s3cr3t@public.example/",
            "wss://admin\\s3cr3t@public.example/",
        ] {
            let refused = vet_url_with(DialPolicy::DenyPrivate, &NoDns, url, DEADLINE).await;
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|e| e.to_string().contains(WITHHELD_HOST)),
                "{url:?}: {refused:?}"
            );
            if let Err(refused) = refused {
                assert_shows_none_of(&refused, &["admin", "s3cr3t"]);
            }
        }
        for hop in [
            "http://127.0.0.1/admin@s3cr3t",
            "http://10.0.0.1\\s3cr3t@public.example/",
        ] {
            let refused = ssrf_check_url_nonblocking(hop);
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|e| e.to_string().contains(WITHHELD_HOST)),
                "{hop:?}: {refused:?}"
            );
            if let Err(refused) = refused {
                assert_shows_none_of(&refused, &["admin", "s3cr3t"]);
            }
        }
    }

    /// A URL with unambiguous userinfo keeps naming the refused host.
    #[tokio::test]
    async fn host_refusal_names_a_host_read_from_a_clean_url() {
        for url in [
            "http://nowhere.example/path",
            "http://alice:s3cr3t@nowhere.example/path",
        ] {
            let refused = ssrf_check_url(url).await;
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|e| e.contains("nowhere.example")),
                "{refused:?}"
            );
        }
    }
}
