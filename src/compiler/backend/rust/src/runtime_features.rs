//! Single source of truth for the `ipe-runtime-rust` cargo features a program
//! needs.
//!
//! The emitter trims the vendored runtime to the modules + crates a program
//! actually reaches. Today that trimming is expressed by the per-surface
//! manifest augmenters (`db_cargo_toml`, `url_cargo_toml`, …) and the
//! `RUNTIME_MOD_RS_*_APPEND` module appends in [`crate::project`]. This module
//! restates the SAME reachability, once, as a mapping from an [`EmitCtx`]'s
//! `uses_*` / `reaches_*` predicates to the exact set of runtime-crate cargo
//! features that program selects.
//!
//! The feature set is a typed value, not a bag of strings: [`RuntimeFeature`]'s
//! variants ARE the runtime crate's declared feature universe, so a feature
//! that does not exist in `src/runtime/rust/Cargo.toml` cannot be named here,
//! and the closure SEAL (`tests/runtime_featureset_closure.rs`) proves the
//! image over the whole flag space stays inside that universe.
//!
//! This is the authority the dependency-model emit will read to write the
//! `features = [...]` list. It is introduced here and unit-/SEAL-tested; the
//! emit path is not yet switched to it, so the emitted output is unchanged.

use std::collections::BTreeSet;

use crate::{DbDriver, EmitCtx};

/// One runtime-crate cargo feature. Every variant maps to a feature declared in
/// `src/runtime/rust/Cargo.toml`'s `[features]` table; [`Self::as_str`] is that
/// exact feature name. Keeping the set closed as an enum makes a
/// "select a feature the crate does not declare" state unrepresentable at the
/// SSOT boundary — the drift can only be a stale variant, which the closure
/// SEAL catches against the crate manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeFeature {
    /// `json` — the JSON codec (`serde_json`, and via `json = […, "serde"]` the
    /// serde stack). Selected by `reaches_json()`: a program that NAMES the
    /// `Value`/`Decoder` type (a `Json`-building kernel, a `Json`/`Decoder`
    /// type-mention, or a db/config/jwt/web surface whose crate feature lists
    /// `json`). A program that names none of these drops the crate.
    Json,
    /// `async` — the tokio reactor spine, selected for any reactor-requiring
    /// program (mirrors [`EmitCtx::uses_async_runtime`]).
    Async,
    /// `db-sqlite` — the sqlite driver alias set (`uses_db` under the Sqlite
    /// driver). Implies `db`.
    DbSqlite,
    /// `db-postgres` — the postgres driver alias set (`uses_db` under the
    /// Postgres driver). Implies `db` (and, per the crate graph, `sqlx/sqlite`
    /// for the session store).
    DbPostgres,
    /// `server` — axum + tower-http (`uses_server`, and forced by `web` /
    /// `webview`, whose runtime modules import the server surface).
    Server,
    /// `web` — the Live web app runtime (`uses_web`, forced by `webview`).
    Web,
    /// `tui` — the terminal-UI backend (`uses_tui`).
    Tui,
    /// `webview` — the native-window backend (`uses_webview`).
    Webview,
    /// `websocket_client` — the outbound WebSocket client (`uses_websocket`).
    WebsocketClient,
    /// `email` — the SMTP transport (`uses_email`).
    Email,
    /// `http_client` — the reqwest outbound HTTP stack
    /// (`reaches_http_client()`: an HTTP kernel or a surface whose runtime
    /// module — server / web / webview / email — calls into it).
    HttpClient,
    /// `url` — the `Ipe.Url` typed-URL module + `ssrf` validators, i.e. the
    /// `url` crate and its idna → ICU4X subtree (`reaches_url()`: a URL kernel
    /// or a surface that parses with `url` — the HTTP or WebSocket client).
    Url,
    /// `config` — the TOML/YAML `Config` decoders (`uses_config`).
    Config,
    /// `compression` — flate2 + zstd (`uses_compression`).
    Compression,
    /// `csv_kernel` — the csv crate (`uses_csv`).
    CsvKernel,
    /// `time` — the IANA-zone calendar surface, `chrono-tz` (`uses_time`).
    Time,
    /// `encoding` — the `base64` + `hex` + `percent-encoding` codec crates and
    /// the `encoding.rs` / `bytes.rs` runtime modules (`reaches_encoding()`: an
    /// `Ipe.Encoding` / `Ipe.Bytes` kernel, OR a crypto/db/server/email/jwt/web
    /// surface whose runtime module uses the raw codec crates). A program that
    /// reaches none of these drops `base64` + `hex` (`percent-encoding` also
    /// enters via the always-on `serde_urlencoded` floor dep, untouched here).
    Encoding,
    /// `regex` — the `regex` crate (+ its `aho-corasick` / `regex-automata` /
    /// `regex-syntax` subtree) and the `regex_kernel.rs` module (`reaches_regex()`:
    /// an `Ipe.Regex` kernel or `String.isUrl`, whose validator relocated into
    /// that module). A standalone leaf — no surface implies it. A program that
    /// reaches neither drops all four crates.
    Regex,
    /// `uuid` — the `uuid` crate and the `uuid_kernel.rs` module
    /// (`reaches_uuid()`: an `Ipe.Uuid` kernel, OR the `server` / `web` surfaces
    /// whose runtime modules mint ids via `uuid::new_v4`). A bare Program that
    /// reaches none drops the crate.
    Uuid,
    /// `random` — the `random.rs` module (`reaches_random()`: an `Ipe.Random`
    /// kernel). A standalone leaf — no surface implies it. The feature gates the
    /// MODULE only, not `getrandom` (kept by the always-on `crypto_core` floor
    /// until the crypto-core demotion phase).
    Random,
    /// `log` — the `log.rs` module (`reaches_log()`: an `Ipe.Log` kernel). A
    /// standalone leaf — no surface implies it. Enables base `chrono` (`log =
    /// ["dep:chrono"]`), so it is one of the two selectors that keep `chrono` in
    /// the graph (the other is `time-core`).
    Log,
    /// `time-core` — base `chrono` and the `time.rs` module
    /// (`reaches_time_core()`: `log` OR any Time/Db/Web/WebView surface). The IANA
    /// zone DB (`chrono-tz`) is the separate `Time` feature, which implies this.
    /// The single selector for whether the emitted crate keeps `chrono`.
    TimeCore,
    /// `decimal` — the `decimal.rs`/`money.rs` modules + `rust_decimal`
    /// (`reaches_decimal()`: a `Decimal.*`/`Money.*` kernel OR the `Db` surface,
    /// which decodes numeric columns through `rust_decimal`). `money.rs` builds on
    /// `decimal.rs`'s `Decimal`, so one feature gates both.
    Decimal,
    /// `char-category` — the `char_category.rs` module + `unicode-general-category`
    /// (`reaches_char_category()`: an `Ipe.Char` `General_Category` predicate). A
    /// standalone leaf. The std-only `Ipe.Char` kernels stay in `char_kernel.rs`.
    CharCategory,
    /// `crypto-core` — the cryptographic floor: `crypto_core.rs` and its `sha2`
    /// hash / `hmac` / `subtle` constant-time / `getrandom` entropy deps
    /// (`reaches_crypto_core()`: a crypto-floor kernel, OR the crypto / jwt / db /
    /// web / webview / email / server surfaces, all of which reach the floor). A
    /// bare synchronous Program reaches none of these and drops the module, the
    /// `sha2`/`hmac`/`subtle` subtree, and — since `getrandom` is enabled only by
    /// `random || crypto-core` — `getrandom` too.
    CryptoCore,
    /// `secret` — the `secret.rs` opaque secret-string module and its `zeroize`
    /// dep (`reaches_secret()`: a `Secret.*` kernel / `Secret`-typed value, or the
    /// JWT / Auth surface whose `Algorithm` is a `secret::Secret`). Implies
    /// `crypto-core` for the shared `subtle` compare.
    Secret,
    /// `crypto` — the heavy crypto surface: rsa + bcrypt + AEAD + pbkdf2
    /// (`uses_crypto`). Implies `crypto-core`.
    Crypto,
    /// `jwt` — the JWT encode/decode surface, `jsonwebtoken` (`reaches_jwt()`:
    /// a JWT kernel or the `Ipe.Auth` surface). Implies `json` + `crypto`.
    Jwt,
}

impl RuntimeFeature {
    /// The exact cargo feature name in `src/runtime/rust/Cargo.toml`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Async => "async",
            Self::DbSqlite => "db-sqlite",
            Self::DbPostgres => "db-postgres",
            Self::Server => "server",
            Self::Web => "web",
            Self::Tui => "tui",
            Self::Webview => "webview",
            Self::WebsocketClient => "websocket_client",
            Self::Email => "email",
            Self::HttpClient => "http_client",
            Self::Url => "url",
            Self::Config => "config",
            Self::Compression => "compression",
            Self::CsvKernel => "csv_kernel",
            Self::Time => "time",
            Self::Encoding => "encoding",
            Self::Regex => "regex",
            Self::Uuid => "uuid",
            Self::Random => "random",
            Self::Log => "log",
            Self::TimeCore => "time-core",
            Self::Decimal => "decimal",
            Self::CharCategory => "char-category",
            Self::CryptoCore => "crypto-core",
            Self::Secret => "secret",
            Self::Crypto => "crypto",
            Self::Jwt => "jwt",
        }
    }
}

/// The set of runtime-crate features a program selects. A thin newtype over a
/// sorted, deduplicated [`BTreeSet`] so callers get a canonical, stable
/// `features = [...]` order and cannot construct a set from arbitrary strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeFeatureSet(BTreeSet<RuntimeFeature>);

impl RuntimeFeatureSet {
    /// The selected feature names, canonical order — the `features = [...]`
    /// list a dependency-model manifest would write.
    pub fn as_feature_names(&self) -> Vec<&'static str> {
        self.0.iter().map(|f| f.as_str()).collect()
    }
}

/// The runtime-crate features a program's kernel/surface usage selects.
///
/// This is the SSOT restating the trimming in [`crate::project`] once. Each
/// insertion below pairs one-to-one with an augmenter/append in
/// `assemble_project_files`, keyed to the SAME predicate — so the SSOT and the
/// emitter can never disagree once the emit path reads this (the closure SEAL
/// enforces that the referenced modules are covered).
pub fn runtime_features(ctx: &EmitCtx) -> RuntimeFeatureSet {
    let mut set = BTreeSet::new();

    // JSON codec (`serde_json`, and via `json = […, "serde"]` the serde stack).
    // Selected only when the program NAMES the `Value`/`Decoder`
    // type (`reaches_json`: a `Json`-building kernel, a `Json`/`Decoder`
    // type-mention, or a db/config/jwt surface whose decoders spell `Decoder` and
    // whose crate feature implies `json`). A program that reaches none drops the
    // two prelude aliases + `serde_json` + the whole serde proc-macro stack. The
    // `jwt` split keeps `jsonwebtoken` off this feature.
    if ctx.reaches_json() {
        set.insert(RuntimeFeature::Json);
    }

    // Reactor spine — the tokio-bound halves of the floor + every async surface.
    if ctx.uses_async_runtime {
        set.insert(RuntimeFeature::Async);
    }

    // Database: exactly one driver alias set, chosen by the project's driver.
    // Both aliases imply `db`; the emitter never selects both (a program targets
    // one driver), which the crate's `compile_error!` guards fail-closed.
    if ctx.uses_db {
        set.insert(match ctx.db_driver {
            DbDriver::Sqlite => RuntimeFeature::DbSqlite,
            DbDriver::Postgres => RuntimeFeature::DbPostgres,
        });
    }

    // Server (axum): the server surface, plus web/webview whose runtime modules
    // import it — matching `server_cargo_toml`'s guard.
    if ctx.uses_server || ctx.uses_web || ctx.uses_webview {
        set.insert(RuntimeFeature::Server);
    }
    // Web app runtime — forced by webview (its backend imports `web`).
    if ctx.uses_web || ctx.uses_webview {
        set.insert(RuntimeFeature::Web);
    }
    if ctx.uses_tui {
        set.insert(RuntimeFeature::Tui);
    }
    if ctx.uses_webview {
        set.insert(RuntimeFeature::Webview);
    }
    if ctx.uses_websocket {
        set.insert(RuntimeFeature::WebsocketClient);
    }
    if ctx.uses_email {
        set.insert(RuntimeFeature::Email);
    }

    // Outbound HTTP client (reqwest) — an HTTP kernel or a surface that reaches
    // `http_client.rs` (server / web / webview / email).
    if ctx.reaches_http_client() {
        set.insert(RuntimeFeature::HttpClient);
    }
    // `url` crate + idna/ICU4X subtree — a URL kernel or a surface that parses
    // with `url` (the HTTP or WebSocket client).
    if ctx.reaches_url() {
        set.insert(RuntimeFeature::Url);
    }

    if ctx.uses_config {
        set.insert(RuntimeFeature::Config);
    }
    if ctx.uses_compression {
        set.insert(RuntimeFeature::Compression);
    }
    if ctx.uses_csv {
        set.insert(RuntimeFeature::CsvKernel);
    }
    if ctx.uses_time {
        set.insert(RuntimeFeature::Time);
    }

    // Encoding codecs (base64/hex/percent-encoding) + the `encoding`/`bytes`
    // modules. `reaches_encoding()` folds the direct encoding/bytes kernels with
    // every surface whose runtime module uses the raw codec crates — crypto/db/
    // server/email/jwt/web. The crate-side feature implications (crypto/db/server/
    // email/jwt/web each list `encoding`) carry the same closure, so this insertion
    // and the graph agree even at `--no-default-features`.
    if ctx.reaches_encoding() {
        set.insert(RuntimeFeature::Encoding);
    }

    // Regex (`regex` crate + its aho-corasick/regex-automata/regex-syntax subtree)
    // + the `regex_kernel.rs` module. A standalone leaf: `uses_regex` folds an
    // `Ipe.Regex` kernel with `String.isUrl` (its body relocated into
    // `regex_kernel.rs`); no surface implies it.
    if ctx.uses_regex {
        set.insert(RuntimeFeature::Regex);
    }

    // Uuid (`uuid` crate) + the `uuid_kernel.rs` module. `reaches_uuid()` folds
    // the direct `Ipe.Uuid` kernels with the server/web surfaces (whose runtime
    // modules mint ids via `uuid::new_v4`). The crate-side implications (server/web
    // each list `uuid`) carry the same closure at `--no-default-features`.
    if ctx.reaches_uuid() {
        set.insert(RuntimeFeature::Uuid);
    }

    // Random (`random.rs` module). `reaches_random()` folds the direct
    // `Ipe.Random` kernels with the async runtime — `task.rs`'s tokio retry-jitter
    // path draws from `random`'s LCG, so any tokio program needs the module. The
    // crate-side `async`/`db`/`server`/… features (which enable `tokio`) each list
    // `random`, carrying the same closure at `--no-default-features`. The feature
    // gates the MODULE only; `getrandom` stays non-optional (the always-on
    // `crypto_core` floor needs it) until the crypto-core demotion phase, so a bare
    // (sync) Program keeps `getrandom` but drops `random.rs`.
    if ctx.reaches_random() {
        set.insert(RuntimeFeature::Random);
    }

    // Log (`log.rs` module + base `chrono`). A standalone leaf: `reaches_log()`
    // is exactly `uses_log` (an `Ipe.Log` kernel). Selecting `log` enables
    // `chrono` via `log = ["dep:chrono"]`; the `time-core` insertion below folds
    // `log` into the broader `chrono`-keeping union so the manifest and this SSOT
    // agree even at `--no-default-features`.
    if ctx.reaches_log() {
        set.insert(RuntimeFeature::Log);
    }

    // Time-core (base `chrono` + the `time.rs` module). `reaches_time_core()`
    // folds `log` with every timestamp-rendering surface — any `Ipe.Time` kernel
    // and the db/web/webview modules. This is the single selector for whether the
    // emitted crate keeps `chrono`. The IANA zone DB (`chrono-tz`) rides the
    // separate `Time` feature below, which the crate graph makes imply
    // `time-core`. FAIL-CLOSED: any uncertain `chrono` consumer keeps it on.
    if ctx.reaches_time_core() {
        set.insert(RuntimeFeature::TimeCore);
    }

    // Decimal (`decimal.rs`/`money.rs` modules + `rust_decimal`).
    // `reaches_decimal()` folds a `Decimal.*`/`Money.*` kernel with the `Db`
    // surface, which decodes numeric SQL columns through `rust_decimal`. The
    // crate-side `db` feature lists `decimal`, carrying the same closure at
    // `--no-default-features`. FAIL-CLOSED: any uncertain `rust_decimal` consumer
    // keeps it on.
    if ctx.reaches_decimal() {
        set.insert(RuntimeFeature::Decimal);
    }

    // Char-category (`char_category.rs` module + `unicode-general-category`). A
    // standalone leaf: `reaches_char_category()` is exactly `uses_char_category`
    // (an `Ipe.Char` General_Category predicate). The std-only `Ipe.Char` kernels
    // stay in the always-compiled `char_kernel.rs`, so a program using only them
    // drops the crate.
    if ctx.reaches_char_category() {
        set.insert(RuntimeFeature::CharCategory);
    }

    // Heavy crypto (rsa/bcrypt/AEAD/pbkdf2). The `crypto_core` floor
    // (sha2/hmac/entropy) stays unconditional in the base manifest; only the
    // heavy surface is a feature. `reaches_crypto_core_heavy()` (crypto ∪ jwt)
    // is covered because `jwt` implies `crypto` in the crate graph, so selecting
    // `jwt` alone already enables the RSA arm the JWT RS256 path needs.
    if ctx.uses_crypto {
        set.insert(RuntimeFeature::Crypto);
    }

    // Crypto floor (`crypto_core.rs` + `sha2`/`hmac`/`subtle`/`getrandom`).
    // `reaches_crypto_core()` folds a direct crypto-floor kernel with every
    // surface whose runtime module reaches the floor: crypto (re-export/reveal),
    // jwt (HMAC/RSA sign + `secret::Secret` Algorithm), db (migration-checksum
    // SHA-256), web/webview (client-JS SRI SHA-256 + CSRF `subtle` compare), email
    // (SMTP-auth HMAC-SHA-256), server (session-id `subtle` compare). Each is
    // verified against the runtime source. The crate-side implications (crypto/jwt/
    // db/web/email/server each list `crypto-core`) carry the same closure at
    // `--no-default-features`. FAIL-CLOSED: any uncertain floor consumer keeps it.
    if ctx.reaches_crypto_core() {
        set.insert(RuntimeFeature::CryptoCore);
    }
    // Secret (`secret.rs` + `zeroize`). `reaches_secret()` folds a direct
    // `Secret.*` kernel / `Secret`-typed value with the JWT/Auth surface (whose
    // `Algorithm` is a `secret::Secret`). `secret` implies `crypto-core` (shared
    // `subtle`) in the crate graph, so the two selections agree at
    // `--no-default-features`.
    if ctx.reaches_secret() {
        set.insert(RuntimeFeature::Secret);
    }

    // JWT surface (`jsonwebtoken`) — a JWT kernel or the `Ipe.Auth` surface.
    if ctx.reaches_jwt() {
        set.insert(RuntimeFeature::Jwt);
    }

    RuntimeFeatureSet(set)
}

#[cfg(test)]
mod tests {
    use super::{RuntimeFeature, runtime_features};
    use crate::{DbDriver, RustBackend};
    use ipe_intern::Interner;
    use ipe_ir::{ModPath, Module, Program};

    /// A body-free module with the named `uses_*` flags set. `configure` sets the
    /// surface flags under test; the async spine + tui⇒ui invariants the lowerer
    /// enforces are restored afterwards so the ctx matches a real program.
    fn ctx_module(name: ipe_intern::Symbol, configure: impl FnOnce(&mut Module)) -> Module {
        let mut m = Module {
            name: ModPath(vec![name]),
            types: vec![],
            funcs: vec![],
            entry: None,
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_ui: false,
            uses_web: false,
            uses_tui: false,
            uses_webview: false,
            uses_css: false,
            uses_auth: false,
            uses_websocket: false,
            uses_email: false,
            uses_time: false,
            uses_env_public: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
            uses_encoding: false,
            uses_regex: false,
            uses_uuid: false,
            uses_random: false,
            uses_log: false,
            uses_decimal: false,
            uses_char_category: false,
            uses_crypto_core: false,
            uses_secret: false,
            uses_json: false,
            uses_crypto: false,
            uses_jwt: false,
            uses_url: false,
            uses_debug: false,
            uses_ffi: false,
            uses_async_runtime: false,
        };
        configure(&mut m);
        m
    }

    /// Compute the selected feature names for a single-module program via the
    /// real backend's `EmitCtx`, so the SSOT is exercised through the exact ctx
    /// the emitter builds.
    fn features_for(configure: impl FnOnce(&mut Module)) -> Vec<&'static str> {
        let mut interner = Interner::new();
        let main = interner.intern("Main").expect("intern Main");
        let prog = Program {
            modules: vec![ctx_module(main, configure)],
        };
        let backend = RustBackend::new(&interner);
        let ctx = backend.emit_ctx_for_tests(&prog).expect("build EmitCtx");
        runtime_features(&ctx).as_feature_names()
    }

    #[test]
    fn hello_world_selects_no_features() {
        // A pure program (no surface, no reactor, no Json type) selects NOTHING;
        // the emitted crate carries no `serde_json` and no serde stack, leaving
        // `app + ipe_runtime + libc`.
        assert!(
            features_for(|_| {}).is_empty(),
            "a bare program selects no runtime feature: {:?}",
            features_for(|_| {})
        );
    }

    #[test]
    fn json_type_mention_selects_json() {
        // A program that NAMES the `Value`/`Decoder` type (here via the
        // `uses_json` flag the lowerer sets from a type-mention or Json kernel)
        // keeps `json` — the fail-closed case the two prelude aliases need.
        let f = features_for(|m| {
            m.uses_json = true;
        });
        assert_eq!(
            f,
            vec!["json"],
            "a Json-naming program selects `json`: {f:?}"
        );
    }

    #[test]
    fn tui_selects_tui_and_async() {
        let f = features_for(|m| {
            m.uses_tui = true;
            m.uses_ui = true;
            m.uses_async_runtime = true;
        });
        assert!(f.contains(&"tui"), "tui program selects `tui`: {f:?}");
        assert!(f.contains(&"async"), "tui program selects `async`: {f:?}");
        // `tui` does NOT list `json` in the crate graph, and this program names no
        // `Value`/`Decoder` type — so `json` (demoted from the floor) is dropped.
        assert!(
            !f.contains(&"json"),
            "a bare tui program drops `json`: {f:?}"
        );
    }

    #[test]
    fn web_pulls_server_http_client_and_url() {
        // A web program reaches server (axum), the http_client surface, and the
        // url parser transitively — none named directly.
        let f = features_for(|m| {
            m.uses_web = true;
            m.uses_async_runtime = true;
        });
        for want in ["web", "server", "http_client", "url", "async", "json"] {
            assert!(f.contains(&want), "web program must select `{want}`: {f:?}");
        }
        assert!(
            !f.contains(&"tui"),
            "web program must not select `tui`: {f:?}"
        );
    }

    #[test]
    fn http_client_only_selects_encoding() {
        // A bare `Ipe.Http` client (no server/web/db surface) still reaches
        // `http_client.rs`, which form-url-decodes query pairs through
        // `encoding.rs`. Dropping `encoding` here ships a program whose `cargo
        // build` fails on an unresolved `form_url_decode` — the forbidden
        // under-inclusion. `encoding` must ride along with `http_client`.
        let f = features_for(|m| {
            m.uses_http = true;
            m.uses_async_runtime = true;
        });
        assert!(
            f.contains(&"http_client"),
            "http program selects `http_client`: {f:?}"
        );
        assert!(
            f.contains(&"encoding"),
            "http_client form-url-decodes → must select `encoding`: {f:?}"
        );
        assert!(
            !f.contains(&"server"),
            "a bare client must not select `server`: {f:?}"
        );
    }

    #[test]
    fn websocket_pulls_url_but_not_http_client() {
        // The WS client parses URLs (via `url`) but does not link the reqwest
        // HTTP stack — the option-B split the crate features encode.
        let f = features_for(|m| {
            m.uses_websocket = true;
            m.uses_async_runtime = true;
        });
        assert!(f.contains(&"websocket_client"), "{f:?}");
        assert!(f.contains(&"url"), "ws client selects `url`: {f:?}");
        assert!(
            !f.contains(&"http_client"),
            "ws client must NOT select `http_client`: {f:?}"
        );
    }

    #[test]
    fn auth_reaches_jwt_and_crypto_core() {
        // `Ipe.Auth` reaches `jwt`; `jwt` implies `crypto` in the crate graph,
        // so the RSA (crypto_core heavy) arm the RS256 path needs is enabled.
        let f = features_for(|m| {
            m.uses_auth = true;
            m.uses_async_runtime = true;
        });
        assert!(f.contains(&"jwt"), "auth reaches `jwt`: {f:?}");
        assert!(
            !f.contains(&"crypto"),
            "auth need not select heavy `crypto` directly — `jwt` implies it: {f:?}"
        );
    }

    #[test]
    fn db_sqlite_vs_postgres_selects_the_driver_alias() {
        let mut interner = Interner::new();
        let main = interner.intern("Main").expect("intern Main");
        // The db flag is derived from an injected SqlValue enum, not a bare
        // `uses_*` — drive it through the driver-aware backend instead.
        for (driver, want) in [
            (DbDriver::Sqlite, "db-sqlite"),
            (DbDriver::Postgres, "db-postgres"),
        ] {
            let prog = Program {
                modules: vec![db_module(&mut interner, main)],
            };
            let backend = RustBackend::new(&interner).with_db_driver(driver);
            let ctx = backend.emit_ctx_for_tests(&prog).expect("build EmitCtx");
            let f = super::runtime_features(&ctx).as_feature_names();
            assert!(f.contains(&want), "{driver:?} selects `{want}`: {f:?}");
            let other = if want == "db-sqlite" {
                "db-postgres"
            } else {
                "db-sqlite"
            };
            assert!(
                !f.contains(&other),
                "{driver:?} selects exactly one driver alias, not `{other}`: {f:?}"
            );
        }
    }

    /// A module carrying the injected `SqlValue` enum the backend reads to set
    /// `uses_db` — the same signal a real Db program lowers to.
    fn db_module(interner: &mut Interner, name: ipe_intern::Symbol) -> Module {
        use ipe_ir::{EnumDef, TypeDef};
        let sqlvalue = interner.intern("SqlValue").expect("intern SqlValue");
        let mut m = ctx_module(name, |m| {
            m.uses_async_runtime = true;
        });
        m.types.push(TypeDef::Enum(EnumDef {
            name: sqlvalue,
            home: ModPath(vec![name]),
            variants: vec![],
            type_params: vec![],
        }));
        m
    }

    #[test]
    fn every_variant_maps_to_a_distinct_nonempty_name() {
        // The variant→name map is total and injective (no two features share a
        // cargo name, none empty).
        let all = [
            RuntimeFeature::Json,
            RuntimeFeature::Async,
            RuntimeFeature::DbSqlite,
            RuntimeFeature::DbPostgres,
            RuntimeFeature::Server,
            RuntimeFeature::Web,
            RuntimeFeature::Tui,
            RuntimeFeature::Webview,
            RuntimeFeature::WebsocketClient,
            RuntimeFeature::Email,
            RuntimeFeature::HttpClient,
            RuntimeFeature::Url,
            RuntimeFeature::Config,
            RuntimeFeature::Compression,
            RuntimeFeature::CsvKernel,
            RuntimeFeature::Time,
            RuntimeFeature::Decimal,
            RuntimeFeature::CharCategory,
            RuntimeFeature::Crypto,
            RuntimeFeature::Jwt,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for f in all {
            assert!(!f.as_str().is_empty(), "empty name for {f:?}");
            assert!(seen.insert(f.as_str()), "duplicate name {}", f.as_str());
        }
    }
}
