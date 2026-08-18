//! `Ipe.Db.Dsn` — a typed, opaque database connection descriptor
//! (parse-don't-validate).
//!
//! The ONLY way to obtain a `Dsn` is through [`dsn_parse`] (from a URL string) or
//! [`dsn_build`] (from typed parts); both run the SAME validators, so a `Dsn`
//! value is a proof that the connection descriptor passed every fail-closed
//! check. There is no un-parsed way to construct one.
//!
//! A `Dsn` deliberately stores its password as a [`Secret`], never a plain
//! `String`: the descriptor's most sensitive field cannot be `Debug`-printed,
//! `Display`-rendered, logged, or echoed into an error. The `Debug` impl below is
//! hand-written to redact the whole struct, and no accessor returns the password
//! as a plain `String` — only the reserved `Secret` surface (`Secret.use` /
//! `Secret.redacted`) may touch it.
//!
//! # Trust model — what `Dsn` does and does NOT guarantee
//!
//! A `Dsn` guarantees the descriptor is structurally valid and TLS-secure: a
//! known driver, a present host for a network driver, an in-range port, no
//! control characters in any component, and a transport that is not explicitly
//! downgraded to cleartext (`sslmode=disable` is a hard parse error). It
//! deliberately does NOT decide whether the host is safe to REACH — that is the
//! separate authority of the connect step (a future slice), which owns the
//! `network` capability and any SSRF-style host policy. `Dsn` is the syntactic
//! parse boundary; connecting is a distinct, separately-reviewed act.

use super::IpeResult;
use crate::secret::{Secret, secret_from_string};

/// The closed set of drivers the runtime can describe. Exactly the two sqlx
/// drivers the `db` feature links (`sqlite`, `postgres`); a driver the runtime
/// cannot dial is unrepresentable here rather than a free string the parser would
/// have to string-compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DsnDriver {
    Postgres,
    Sqlite,
}

/// The transport-security posture. `Require`/`Prefer` are the two accepted modes;
/// `Disable` exists for exhaustiveness and a future explicitly-disclosed
/// opt-in, but the parse path REJECTS it — a `Dsn` is never a proof of a
/// cleartext transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    Require,
    Prefer,
    Disable,
}

/// `Ipe.Db.Dsn`'s opaque, validated descriptor. Every field is the result of a
/// fail-closed parse; the password is a [`Secret`] so the struct cannot leak it.
///
/// `Debug` is hand-written (below) to redact the whole value; `Clone` is safe
/// (cloning observes no plaintext — `Secret`'s own `Clone` does not reveal the
/// payload). No `Display`/`IpeStringify` is derived: a `Dsn` is only ever
/// rendered through its explicit redacted form.
#[derive(Clone)]
pub struct Dsn {
    driver: DsnDriver,
    host: String,
    port: u16,
    database: String,
    user: String,
    password: Secret,
    tls: TlsMode,
}

impl std::fmt::Debug for Dsn {
    /// Redact the whole descriptor. The password is already a `Secret` (its own
    /// `Debug` is the fixed placeholder), but the struct-level impl stays
    /// conservative: a `dbg!(dsn)` left in shipped code prints only the
    /// non-secret shape, never the credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dsn")
            .field("driver", &self.driver)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &self.password)
            .field("tls", &self.tls)
            .finish()
    }
}

/// A structural, credential-free rejection reason. Its `Display` NEVER embeds the
/// offending DSN string or any credential — only the category of failure — so a
/// parse `Err` surfaced or logged by the caller cannot echo a pasted password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DsnReject {
    Unparseable,
    UnknownDriver,
    MissingHost,
    InvalidPort,
    TlsDisabled,
    UnknownSslMode,
    ConflictingParameter,
    InvalidComponent,
}

impl DsnReject {
    /// The value-free message. Every arm is a fixed, credential-free string; the
    /// input is never interpolated, so the rejection cannot leak a pasted secret.
    fn message(self) -> &'static str {
        match self {
            Self::Unparseable => "Ipe.Db.Dsn: cannot parse DSN",
            Self::UnknownDriver => "Ipe.Db.Dsn: unknown driver",
            Self::MissingHost => "Ipe.Db.Dsn: missing host",
            Self::InvalidPort => "Ipe.Db.Dsn: invalid port",
            Self::TlsDisabled => "Ipe.Db.Dsn: TLS disabled is not permitted",
            Self::UnknownSslMode => "Ipe.Db.Dsn: unknown sslmode",
            Self::ConflictingParameter => "Ipe.Db.Dsn: conflicting or misplaced parameter",
            Self::InvalidComponent => "Ipe.Db.Dsn: invalid DSN component",
        }
    }
}

fn reject<E: From<String>>(r: DsnReject) -> IpeResult<E, Dsn> {
    IpeResult::Err(r.message().to_owned().into())
}

/// A generous-but-bounded length cap for any single DSN component (host, user,
/// database). Guards the oversize-allocation vector without rejecting any real
/// identifier.
const MAX_COMPONENT_LEN: usize = 512;

/// True when `s` is safe to carry as a DSN component: no control characters, no
/// embedded null, no leading/trailing/interior whitespace, and within the length
/// bound — checked on the PERCENT-DECODED form, so a `%0a`/`%00` that decodes to a
/// control byte is rejected too. Rejecting whitespace and control bytes closes the
/// injection/smuggling vector before any component is trusted.
fn component_ok(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_COMPONENT_LEN {
        return false;
    }
    // Validate the decoded bytes: a `%0a` in the raw form decodes to a newline, a
    // control byte the raw scan would miss. Lossy UTF-8 is fine here — we only
    // ever REJECT on a control/whitespace byte, never trust the decoded value.
    let decoded = percent_encoding::percent_decode_str(s).decode_utf8_lossy();
    !decoded.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// True when `s` names the Postgres driver.
fn driver_is_postgres(scheme: &str) -> bool {
    matches!(scheme, "postgres" | "postgresql")
}

/// True when `s` names the Sqlite driver.
fn driver_is_sqlite(scheme: &str) -> bool {
    matches!(scheme, "sqlite" | "file")
}

/// Parse an `sslmode` token into a `TlsMode`, or a rejection. `disable` is a
/// hard reject (a downgraded transport is not a value the parser mints); an
/// unrecognised token is fail-closed, never coerced to a permissive default.
fn parse_sslmode(token: &str) -> Result<TlsMode, DsnReject> {
    match token {
        "require" => Ok(TlsMode::Require),
        "prefer" => Ok(TlsMode::Prefer),
        "disable" => Err(DsnReject::TlsDisabled),
        _ => Err(DsnReject::UnknownSslMode),
    }
}

/// Read the TLS posture from a URL's query pairs, applying the SECURE DEFAULT
/// (`Require`) when no `sslmode` is present. A `password=` in the query string, a
/// duplicated `sslmode` with differing values, or any of the credential-smuggling
/// shapes is a `ConflictingParameter` rejection: the password must arrive through
/// the structured userinfo, never a re-parseable query segment.
fn tls_from_query(url: &::url::Url) -> Result<TlsMode, DsnReject> {
    let mut chosen: Option<TlsMode> = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "password" | "user" | "username" => {
                // Credential smuggled into the query string — reject; credentials
                // belong in the structured userinfo only.
                return Err(DsnReject::ConflictingParameter);
            }
            "sslmode" => {
                let mode = parse_sslmode(value.as_ref())?;
                match chosen {
                    Some(prev) if prev != mode => {
                        // Two different answers to "is TLS on" — ambiguity is
                        // fail-closed.
                        return Err(DsnReject::ConflictingParameter);
                    }
                    _ => chosen = Some(mode),
                }
            }
            _ => {}
        }
    }
    Ok(chosen.unwrap_or(TlsMode::Require))
}

/// `Ipe.Db.Dsn.parse : String -> Result Error Dsn` — THE seal from a full URL
/// string. Every `Dsn` built this way traces back to one call, so a reviewer can
/// grep this one symbol to audit every place a raw string becomes a descriptor.
///
/// Fails closed on all of: an unparseable string; an unknown driver scheme; a
/// missing host for a network driver; an out-of-range/non-numeric port; an
/// explicit `sslmode=disable`; an unknown `sslmode`; a credential or duplicated
/// security key smuggled into the query; and a control-character/oversized
/// component. The password is captured as a `Secret`, never a plain `String`.
#[must_use]
pub fn dsn_parse<E: From<String>>(s: String) -> IpeResult<E, Dsn> {
    let parsed = match ::url::Url::parse(&s) {
        Ok(u) => u,
        Err(_) => return reject(DsnReject::Unparseable),
    };

    let scheme = parsed.scheme();
    let driver = if driver_is_postgres(scheme) {
        DsnDriver::Postgres
    } else if driver_is_sqlite(scheme) {
        DsnDriver::Sqlite
    } else {
        return reject(DsnReject::UnknownDriver);
    };

    let tls = match tls_from_query(&parsed) {
        Ok(t) => t,
        Err(r) => return reject(r),
    };

    // Sqlite is a local file driver: the "host" is empty and the database is the
    // file path. Postgres is a network driver: a host is mandatory.
    let (host, port, database) = match driver {
        DsnDriver::Postgres => {
            let Some(host) = parsed.host_str() else {
                return reject(DsnReject::MissingHost);
            };
            if !component_ok(host) {
                return reject(DsnReject::InvalidComponent);
            }
            // Postgres' well-known default; an omitted port is not an error.
            let port = parsed.port().unwrap_or(5432);
            let database = parsed.path().trim_start_matches('/').to_owned();
            if !component_ok(&database) {
                return reject(DsnReject::InvalidComponent);
            }
            (host.to_owned(), port, database)
        }
        DsnDriver::Sqlite => {
            // A file-backed sqlite DSN has no network host and no port. The file
            // path is the database; validate it as a component.
            let path = parsed.path().trim_start_matches('/');
            if !component_ok(path) {
                return reject(DsnReject::InvalidComponent);
            }
            (String::new(), 0, path.to_owned())
        }
    };

    let user = parsed.username().to_owned();
    // The username may legitimately be empty (sqlite, or a Postgres DSN relying on
    // a default role); when present it must be a clean component.
    if !user.is_empty() && !component_ok(&user) {
        return reject(DsnReject::InvalidComponent);
    }

    let password = secret_from_string(parsed.password().unwrap_or("").to_owned());

    IpeResult::Ok(Dsn {
        driver,
        host,
        port,
        database,
        user,
        password,
        tls,
    })
}

/// `Ipe.Db.Dsn.build` — THE seal from typed parts. Runs the SAME component,
/// port, and TLS validators as [`dsn_parse`], so structured input cannot bypass
/// the parser. `driver`/`tls` arrive already as closed tags; `port` is validated
/// into `1..=65535` (no narrowing cast); `password` is a `Secret` on the way in.
///
/// The driver/TLS tags are passed as their small-integer discriminants
/// (`0 = Postgres`, `1 = Sqlite`; `0 = Require`, `1 = Prefer`, `2 = Disable`),
/// which is how the emitted ADT constructors marshal. An out-of-set discriminant
/// is a rejection rather than a panic.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn dsn_build<E: From<String>>(
    driver_tag: i64,
    host: String,
    port: i64,
    database: String,
    user: String,
    password: Secret,
    tls_tag: i64,
) -> IpeResult<E, Dsn> {
    let driver = match driver_tag {
        0 => DsnDriver::Postgres,
        1 => DsnDriver::Sqlite,
        _ => return reject(DsnReject::UnknownDriver),
    };
    let tls = match tls_tag {
        0 => TlsMode::Require,
        1 => TlsMode::Prefer,
        2 => TlsMode::Disable,
        _ => return reject(DsnReject::UnknownSslMode),
    };
    if tls == TlsMode::Disable {
        // A downgraded transport is never a value the seal mints.
        return reject(DsnReject::TlsDisabled);
    }

    // Port must be proven in range; no `as u16` narrowing (that truncation is
    // itself the bug being banned). Sqlite carries no port (0 is the sentinel).
    let port_u16: u16 = match driver {
        DsnDriver::Sqlite => 0,
        DsnDriver::Postgres => match u16::try_from(port) {
            Ok(p) if p >= 1 => p,
            _ => return reject(DsnReject::InvalidPort),
        },
    };

    match driver {
        DsnDriver::Postgres => {
            if !component_ok(&host) {
                return reject(DsnReject::InvalidComponent);
            }
        }
        DsnDriver::Sqlite => {
            // Sqlite has no network host; a non-empty host is a misuse.
            if !host.is_empty() {
                return reject(DsnReject::InvalidComponent);
            }
        }
    }
    if !component_ok(&database) {
        return reject(DsnReject::InvalidComponent);
    }
    if !user.is_empty() && !component_ok(&user) {
        return reject(DsnReject::InvalidComponent);
    }

    IpeResult::Ok(Dsn {
        driver,
        host: if driver == DsnDriver::Sqlite {
            String::new()
        } else {
            host
        },
        port: port_u16,
        database,
        user,
        password,
        tls,
    })
}

impl Dsn {
    /// The driver this descriptor names, for the connect step's dialect
    /// selection. Crate-internal: only the external-connection module reads it.
    pub(crate) fn driver(&self) -> DsnDriver {
        self.driver
    }

    /// The network host this descriptor names. Empty string for file-backed
    /// SQLite (no network host). Crate-internal: used by the connect step to
    /// apply the SSRF host gate before dialing.
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    /// The port this descriptor names. Zero for file-backed SQLite (no port).
    /// Crate-internal: used alongside [`Dsn::host`] for the SSRF host gate.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Reconstruct the connection URL the sqlx driver dials, consuming the
    /// password `Secret` at the point of use (never stored back as plaintext).
    /// The result is a live credential-bearing string; it is handed straight to
    /// the sqlx connector and dropped, never logged or returned to Ipê.
    ///
    /// For Postgres the TLS posture is folded into an `sslmode` query so the
    /// parsed secure default (`Require`/`Prefer`) survives the round-trip;
    /// `Disable` is unrepresentable (the parse/build path rejected it). For
    /// Sqlite the database field is the file path and `mode=rwc` is appended so a
    /// missing file is created, matching the app-local opener's behaviour.
    pub(crate) fn connection_url(&self) -> String {
        match self.driver {
            DsnDriver::Sqlite => {
                if self.database.contains(':') {
                    self.database.clone()
                } else {
                    format!("sqlite://{}?mode=rwc", self.database)
                }
            }
            DsnDriver::Postgres => {
                let password = crate::secret::secret_reveal(self.password.clone());
                let sslmode = match self.tls {
                    TlsMode::Require => "require",
                    TlsMode::Prefer => "prefer",
                    // Unreachable: a `Dsn` never carries `Disable`. Fall back to
                    // the strongest posture rather than emitting a downgrade.
                    TlsMode::Disable => "require",
                };
                let mut url = String::from("postgres://");
                if !self.user.is_empty() {
                    url.push_str(
                        &percent_encoding::utf8_percent_encode(
                            &self.user,
                            percent_encoding::NON_ALPHANUMERIC,
                        )
                        .to_string(),
                    );
                    if !password.is_empty() {
                        url.push(':');
                        url.push_str(
                            &percent_encoding::utf8_percent_encode(
                                &password,
                                percent_encoding::NON_ALPHANUMERIC,
                            )
                            .to_string(),
                        );
                    }
                    url.push('@');
                }
                url.push_str(&self.host);
                url.push(':');
                url.push_str(&self.port.to_string());
                url.push('/');
                url.push_str(&self.database);
                url.push_str("?sslmode=");
                url.push_str(sslmode);
                url
            }
        }
    }
}

/// `Ipe.Db.Dsn.driver : Dsn -> Driver` — the driver tag as its discriminant
/// (`0 = Postgres`, `1 = Sqlite`), which the emitted `Driver` ADT constructor
/// re-tags. Non-secret; safe to read.
#[must_use]
pub fn dsn_driver(d: Dsn) -> i64 {
    match d.driver {
        DsnDriver::Postgres => 0,
        DsnDriver::Sqlite => 1,
    }
}

/// `Ipe.Db.Dsn.host : Dsn -> String` — the host component (`""` for a
/// file-backed sqlite descriptor). Non-secret.
#[must_use]
pub fn dsn_host(d: Dsn) -> String {
    d.host
}

/// `Ipe.Db.Dsn.port : Dsn -> Int` — the port (`0` for sqlite). Non-secret.
#[must_use]
pub fn dsn_port(d: Dsn) -> i64 {
    i64::from(d.port)
}

/// `Ipe.Db.Dsn.database : Dsn -> String` — the database name or file path.
/// Non-secret.
#[must_use]
pub fn dsn_database(d: Dsn) -> String {
    d.database
}

/// `Ipe.Db.Dsn.user : Dsn -> String` — the connection user (`""` when none).
/// Non-secret.
#[must_use]
pub fn dsn_user(d: Dsn) -> String {
    d.user
}

/// `Ipe.Db.Dsn.tls : Dsn -> TlsMode` — the transport posture as its discriminant
/// (`0 = Require`, `1 = Prefer`, `2 = Disable`), which the emitted `TlsMode` ADT
/// constructor re-tags. Non-secret.
#[must_use]
pub fn dsn_tls(d: Dsn) -> i64 {
    match d.tls {
        TlsMode::Require => 0,
        TlsMode::Prefer => 1,
        TlsMode::Disable => 2,
    }
}

/// `Ipe.Db.Dsn.redacted : Dsn -> String` — a credential-free, human-readable
/// rendering of the descriptor. The password is NEVER included — it is a
/// `Secret`, and this render substitutes the fixed placeholder. This is the ONLY
/// display path a `Dsn` has.
#[must_use]
pub fn dsn_redacted(d: Dsn) -> String {
    let driver = match d.driver {
        DsnDriver::Postgres => "postgres",
        DsnDriver::Sqlite => "sqlite",
    };
    let tls = match d.tls {
        TlsMode::Require => "require",
        TlsMode::Prefer => "prefer",
        TlsMode::Disable => "disable",
    };
    // The `Secret`'s own `IpeStringify` yields the redacted placeholder; use it so
    // there is exactly one redaction convention.
    let user_part = if d.user.is_empty() {
        String::new()
    } else {
        format!("{}@", d.user)
    };
    match d.driver {
        DsnDriver::Sqlite => format!("{driver}://{}", d.database),
        DsnDriver::Postgres => format!(
            "{driver}://{user_part}{}:{}/{} (tls={tls}, password=[redacted])",
            d.host, d.port, d.database
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A distinctive sentinel: any test that finds this substring in a render or
    // error has leaked the password.
    const SENTINEL: &str = "hunter2-SENTINEL";

    fn parse_ok(s: &str) -> Dsn {
        match dsn_parse::<String>(s.to_string()) {
            IpeResult::Ok(d) => d,
            IpeResult::Err(e) => panic!("expected {s:?} to parse, got Err: {e}"),
        }
    }

    fn parse_err(s: &str) -> String {
        match dsn_parse::<String>(s.to_string()) {
            IpeResult::Ok(_) => panic!("expected {s:?} to be rejected, but it parsed"),
            IpeResult::Err(e) => e,
        }
    }

    // ── The eight fail-closed parse cases ────────────────────────────────────

    #[test]
    fn rejects_unparseable() {
        assert!(parse_err("").contains("cannot parse"));
        assert!(parse_err("not a url at all").contains("cannot parse"));
    }

    #[test]
    fn rejects_unknown_driver() {
        assert!(parse_err("mysql://h:3306/d").contains("unknown driver"));
        assert!(parse_err("http://h/d").contains("unknown driver"));
    }

    #[test]
    fn rejects_missing_host_for_network_driver() {
        assert!(parse_err("postgres:///justadb").contains("missing host"));
    }

    #[test]
    fn rejects_invalid_port() {
        // 99999 is out of the u16 range — the `url` crate rejects it as
        // unparseable, which is still a fail-closed reject (no accepted value).
        let e = parse_err("postgres://h:99999/d");
        assert!(e.contains("cannot parse") || e.contains("invalid port"));
    }

    #[test]
    fn rejects_explicit_tls_disable() {
        assert!(parse_err("postgres://h:5432/d?sslmode=disable").contains("TLS disabled"));
    }

    #[test]
    fn rejects_unknown_sslmode() {
        assert!(parse_err("postgres://h:5432/d?sslmode=bananas").contains("unknown sslmode"));
    }

    #[test]
    fn rejects_smuggled_credential_and_conflicting_keys() {
        assert!(parse_err("postgres://h:5432/d?password=x").contains("conflicting or misplaced"));
        assert!(
            parse_err("postgres://h:5432/d?sslmode=require&sslmode=prefer")
                .contains("conflicting or misplaced")
        );
    }

    #[test]
    fn rejects_control_char_component() {
        // A percent-encoded newline in the database component.
        assert!(parse_err("postgres://h:5432/d%0aevil").contains("invalid DSN component"));
    }

    // ── Secure defaults ──────────────────────────────────────────────────────

    #[test]
    fn omitted_sslmode_defaults_to_require() {
        let d = parse_ok("postgres://user@h:5432/mydb");
        assert_eq!(dsn_tls(d), 0); // 0 = Require
    }

    #[test]
    fn accepts_explicit_prefer() {
        let d = parse_ok("postgres://h:5432/d?sslmode=prefer");
        assert_eq!(dsn_tls(d), 1); // 1 = Prefer
    }

    #[test]
    fn build_rejects_tls_disable_and_out_of_range_port() {
        // tls_tag 2 = Disable → reject.
        assert!(matches!(
            dsn_build::<String>(
                0,
                "h".into(),
                5432,
                "d".into(),
                "u".into(),
                secret_from_string("p".into()),
                2
            ),
            IpeResult::Err(_)
        ));
        // port 0 for a Postgres driver → reject (no narrowing accept).
        assert!(matches!(
            dsn_build::<String>(
                0,
                "h".into(),
                0,
                "d".into(),
                "u".into(),
                secret_from_string("p".into()),
                0
            ),
            IpeResult::Err(_)
        ));
        // port 70000 (> u16::MAX) → reject, not truncated.
        assert!(matches!(
            dsn_build::<String>(
                0,
                "h".into(),
                70000,
                "d".into(),
                "u".into(),
                secret_from_string("p".into()),
                0
            ),
            IpeResult::Err(_)
        ));
    }

    #[test]
    fn build_accepts_valid_typed_parts() {
        let d = dsn_build::<String>(
            0,
            "db.example.com".into(),
            5432,
            "app".into(),
            "reader".into(),
            secret_from_string("p".into()),
            0,
        );
        assert!(matches!(d, IpeResult::Ok(_)));
    }

    // ── The four Secret-non-leak invariants ──────────────────────────────────

    #[test]
    fn redacted_render_omits_password() {
        let d = parse_ok(&format!("postgres://reader:{SENTINEL}@h:5432/app"));
        let rendered = dsn_redacted(d);
        assert!(
            !rendered.contains(SENTINEL),
            "redacted render leaked the password"
        );
        assert!(rendered.contains("h")); // non-secret host still present
        assert!(rendered.contains("[redacted]"));
    }

    #[test]
    fn debug_inspect_omits_password() {
        let d = parse_ok(&format!("postgres://reader:{SENTINEL}@h:5432/app"));
        let shown = format!("{d:?}");
        assert!(!shown.contains(SENTINEL), "Debug leaked the password");
    }

    #[test]
    fn error_payload_omits_password() {
        // A DSN that carries a password AND trips a reject (sslmode=disable). The
        // error must not echo the credential-bearing input.
        let e = parse_err(&format!(
            "postgres://reader:{SENTINEL}@h:5432/app?sslmode=disable"
        ));
        assert!(!e.contains(SENTINEL), "parse error leaked the password");
    }

    // The fourth invariant — "no plain-String password accessor" — is enforced at
    // the type surface: this module exposes NO `dsn_password -> String`. A grep
    // for a password accessor over this file finds only the `Secret`-typed field
    // and the redaction path. (A compile-fail Ipê fixture guards the source
    // surface; see tests/golden.)
    #[test]
    fn no_plain_password_accessor_exists() {
        // Proof by absence over this module's own source: no public accessor
        // returns the password as a plain `String`. The needle is assembled at
        // runtime so this assertion's own text does not self-match.
        let src = include_str!("dsn.rs");
        let needle = format!("pub fn dsn_{}", "password");
        assert!(
            !src.contains(&needle),
            "a plain-String password accessor must never exist"
        );
    }
}
