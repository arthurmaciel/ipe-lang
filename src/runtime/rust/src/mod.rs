// Ipe Runtime — all modules (for standalone crate compilation).
// In generated projects, this file is overridden by the compiler.

// ── Pedantic-lint policy for the public emitter API ──────────────────────────
//
// `needless_pass_by_value`: Every `pub fn` in this crate is emitter-facing API
// called by generated Rust code that passes owned `String`/`Vec<T>` values.
// Changing to `&str`/`&[T]` would require a coordinated emitter change; the
// lint is correct in general but wrong at this API boundary.
//
// `implicit_hasher`: Public functions that accept `HashMap` parameters are part
// of the emitter-facing API. Generalising over `S: BuildHasher` is correct in
// library code but requires a matching emitter change to pass the correct hasher
// at call sites; deferred to the emitter/runtime co-evolution task.
//
// `cast_possible_truncation` / `cast_sign_loss` / `cast_precision_loss` /
// `cast_possible_wrap` / `cast_lossless`: The runtime bridges Ipê's uniform
// `i64` integer type to Rust APIs that use `u32`, `usize`, `f64`, etc.
// Ipê's type system guarantees all integer values are valid `i64`; kernel
// pre-conditions narrow the domain further (e.g. a char code is always in
// 0..=0x10FFFF, list indices are always non-negative). Converting these to
// `u32`/`usize`/`f64` at the bridge is correct under those invariants but
// cannot be proven locally. Replacing every bridge cast with `TryFrom` would
// propagate fallibility throughout the runtime without adding safety — the
// caller (emitted code) cannot recover from a type-system invariant violation
// in any way other than returning an Ipe error, which the runtime already
// does. Where an actual runtime boundary (e.g. a value read from JSON or from
// the environment) CAN violate the invariant, checked conversion is already used.
//
// `many_single_char_names`: Combinator functions in `core.rs` (map2/map3/map4…)
// use `A,B,C,D` as type parameters and `a,b,c,d` as their corresponding bound
// values. Single-letter names ARE the idiomatic form for n-ary products in a
// functional-style combinator; renaming would reduce, not improve, clarity.
#![allow(
    clippy::needless_pass_by_value,
    clippy::implicit_hasher,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_lossless,
    clippy::many_single_char_names
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        // Promoted from the quality-audit advisory set to a HARD deny: these are
        // all panic vectors a well-typed Ipe program must never reach. `cargo
        // clippy` now FAILS on any of them in non-test runtime code, so riipe
        // code cannot be merged (CI security-audit gate + local clippy enforce
        // it). See `## Settled rules` in AGENTS.md.
        clippy::todo,
        clippy::unimplemented,
        clippy::panic_in_result_fn
    )
)]

pub mod config;
#[cfg(feature = "config")]
pub mod config_decode;
pub mod core;
// Constant-time byte equality — the SSOT predicate all secret/tag/key
// newtypes use for `PartialEq`. Gated on `crypto-core` (which pulls `subtle`).
// `secret` implies `crypto-core`, so this gate covers every caller.
#[cfg(feature = "crypto-core")]
pub mod ct_eq;

// The cryptographic floor: the entropy pair, the SHA-2 hash/HMAC family, the RSA
// sign/verify pair, the typed `Key`/`Mac` newtypes and the constant-time compare.
// Behind the `crypto-core` feature (`sha2`/`hmac`/`subtle`/`getrandom`): a
// program that reaches no crypto-floor kernel — and no crypto/jwt/db/web/webview/
// email/server surface that reaches the floor (each of those features implies
// `crypto-core`) — drops the module and its subtree. The heavy RSA arms inside
// the module stay `cfg(feature = "crypto")`; wasm32 (`wasm-client` implies
// `crypto-core`) compiles the entropy pair + pure hash family.
#[cfg(feature = "crypto-core")]
pub mod crypto_core;
// Floor re-export, mirroring the emitted `pub use crypto_core::*` — the SHA-2 /
// HMAC family, entropy pair, and typed Key/Mac are reachable at the crate root
// when `crypto-core` is on. When `crypto` is on it implies `crypto-core`, and
// `crypto.rs` re-exports the same items again; a glob of identical items is not
// ambiguous.
#[cfg(feature = "crypto-core")]
pub use crypto_core::*;
// wasm32: `crypto_random_bytes`/`crypto_random_token` (the browser entropy
// substitute) plus the pure hash family compile without the native-only AEAD
// deps — those functions are individually `cfg(not(target_arch = "wasm32"))`
// inside `crypto.rs` (see its module doc). The `crypto` module re-exports the
// `crypto_core` floor so every `crypto::…` path resolves.
#[cfg(any(
    feature = "crypto",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub mod crypto;
// Heavy-crypto glob re-export — the legacy `crypto_sha1` / `crypto_md5` checksums
// and the AEAD/PBKDF2 kernels are named unqualified at the crate root by emitted
// user bodies (`Crypto.sha1` → `crypto_sha1`), so the glob must surface them.
// Gated on the same `crypto` feature as the module. `crypto.rs` also re-globs the
// `crypto_core` floor; when both are on, a glob of identical items is not
// ambiguous.
#[cfg(any(
    feature = "crypto",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub use crypto::*;
pub mod file;
// `log` is behind the `log` feature: `log.rs`'s native RFC3339-nano timestamp is
// the one always-emittable `chrono` consumer, so a program that reaches no
// `Ipe.Log.*` kernel drops the module — and, via `time-core`, `chrono`. Its
// Ipê-facing kernels return `IpeTask`/`IpeResult` (from `core`, no tokio
// dependency) and its bodies split only on the wasm32 console sink. wasm32:
// `Log.*` routes to `console.{debug,info,warn,error}` (see `log.rs`'s
// `cfg(target_arch = "wasm32")` sink split) using `js_sys::Date`, not `chrono`;
// `wasm-client` implies `log` so the static wasm module set still resolves.
#[cfg(feature = "log")]
pub mod log;
// `Ipe.Random` non-cryptographic PRNG. Behind the `random` feature: a program
// that reaches no `Ipe.Random` kernel drops the module. The feature gates only
// this module. `getrandom` (the entropy source shared with `crypto_core` and
// with this module's wasm seed arm) is enabled by `random || crypto-core`, so
// gating `random` alone never removes `getrandom`.
#[cfg(feature = "random")]
pub mod random;
// `system` is always compiled (not tokio-gated): it owns the process-global
// env RwLock + the `read_env_var` / `read_env_var_os` / `locked_set_var` /
// `locked_set_var_if_absent` / `locked_remove_var` accessors that EVERY module
// (always-compiled telemetry/core/file/csv/… included) must route process-env
// access through for the reader↔mutator serialisation to hold by construction.
// Its Ipê-facing helpers return `IpeTask`/`IpeResult` (defined in `core`, no
// tokio dependency) and otherwise use only std, so it compiles without tokio.
pub mod system;
// wasm32: the pure future-combinator half of `Task.*` (`map`/`andThen`/
// `mapError`/`succeed`/`fail`/`fromResult`/`andThenResult`/`onError`/`lazy`/
// `sequence`) compiles + runs unchanged — no tokio dependency. The
// tokio-bound half (`block_on`/`Task.run`/`Task.parallel`/`Task.retryWith`)
// stays `cfg(not(target_arch = "wasm32"))` inside the file (see its doc).
// `task` is always compiled: its reactor spine (`block_on`, `task_parallel`,
// `task_retry_with`, the shared runtime) is `#[cfg(feature = "tokio")]`, and its
// pure combinators plus the std-only `block_on` compile without tokio — so the
// module is available in every config, mirroring the always-on `pub mod task;`
// in the emitted crate's runtime template.
pub mod task;
// `time` is behind the `time-core` feature (base `chrono`). The whole module —
// the reactor-free clock reads (`Time.now`/`unixMillis`), the `Time.sleep`
// timer, the calendar math, and the `chrono-tz`-gated IANA zone helpers — lives
// behind `time-core`; a program that reaches no `Ipe.Time` kernel (and no
// Log/Db/Web surface) drops the module and `chrono`. The IANA zone helpers keep
// their inner `#[cfg(feature = "time")]` on top (they additionally need
// `chrono-tz`). wasm32: `wasm-client` implies `time`, so the static wasm module
// set resolves; its arms read `js_sys::Date`/`gloo-timers`, not `chrono`.
#[cfg(feature = "time-core")]
pub mod time;
// Always declared, matching the emitted floor (`templates/ipe_runtime/mod.rs`
// declares `pub mod trace;` for every program). `trace.rs` builds a std
// `IpeTask` future (`async move`/`.await` on the boxed future, no `tokio::`
// item), so it compiles under `--no-default-features`; the always-on glob
// re-export below then keeps `trace_span`/`trace_event`/`trace_attr` reachable
// at the crate root for a sync dependency-model program that names them.
pub mod trace;
pub use file::*;

// The lexical path-validation algorithm — the SINGLE source of truth shared
// with the compiler's `path "…"` gate (the `ipe_path_core` crate `include!`s
// this exact file). A sibling module (not an extern crate) so it also resolves
// when the runtime is vendored as `mod ipe_runtime` into an emitted app. No
// glob re-export: `path` reaches it via `super::path_core::…`.
pub mod path_core;

pub mod path;
pub use path::*;

// `url`: behind the `url` feature (the `::url::` crate is now optional). A
// program that reaches no URL/SSRF/HTTP/WebSocket surface drops it, matching the
// emitted floor where `url` is appended only under `uses_url`.
#[cfg(feature = "url")]
pub mod url;
#[cfg(feature = "url")]
pub use url::*;

#[cfg(feature = "db")]
pub mod db;
// `dsn`: the typed, opaque `Ipe.Db.Dsn` connection descriptor
// (parse-don't-validate). Behind `db` (a DSN is a database-domain type); it
// parses with the `url` crate, which `db` now pulls.
#[cfg(feature = "db")]
pub mod dsn;
#[cfg(feature = "db")]
pub use dsn::*;
// `external_conn`: the live `Ipe.Db.Connection` to a database the app was not
// built against, opened from a parsed `Dsn`. Read-only by phantom type; an
// independent pool of the driver the `Dsn` named. Behind `db` (both sqlx
// drivers link under `db`, so either external dialect is buildable).
#[cfg(feature = "db")]
pub mod external_conn;
#[cfg(feature = "db")]
pub use external_conn::*;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "db")]
pub use db::*;
// Telemetry spill — write-through SQLite persistence behind the
// db feature; the always-compiled telemetry sink calls its cfg-stubbed hook.
#[cfg(feature = "db")]
pub mod telemetry_spill;

pub use config::*;
#[cfg(feature = "config")]
pub use config_decode::*;
pub use core::*;
#[cfg(feature = "json")]
pub use json::*;
// Floor-module glob re-exports: the always-declared modules are unconditional,
// matching the emitted floor. Their tokio-bound items are item-gated inside each
// file (`block_on`, `system_load_env`, … carry both a `feature = "tokio"` and a
// `not(feature = "tokio")` arm), so a sync program's generated prelude — which
// names `block_on` / `system_*` unqualified at the crate root — resolves under
// `--no-default-features`. Gating those globs on `tokio` instead would drop the
// std-available items and break every non-async dependency-model program (E0425).
//
// `log` / `time` re-exports track their module gates (`log` / `time-core`): a
// program that reaches no Log/Time surface drops the module, so its glob must
// drop with it. The emitted prelude's `log_*` / `time_*` wrappers are cut in the
// same case (`project::native_runtime_bindings`), so no unqualified name is left
// dangling at the crate root.
#[cfg(feature = "log")]
pub use log::*;
#[cfg(feature = "random")]
pub use random::*;
pub use system::*;
pub use task::*;
#[cfg(feature = "time-core")]
pub use time::*;
pub use trace::*;

// `Ipe.Encoding` codecs (base64 / url-percent / hex). Behind the `encoding`
// feature (the `base64` / `hex` / `percent-encoding` crates are optional): a
// program that reaches no encoding/bytes kernel — and no crypto/db/server/email/
// jwt/web surface that implies `encoding` — drops all three crates.
#[cfg(feature = "encoding")]
pub mod encoding;
#[cfg(feature = "encoding")]
pub use encoding::*;

// `Ipe.Bytes` — distinct `Vec<u8>` byte buffer. Behind `encoding` (its hex /
// base64 kernels use those crates; the std-only half moves with it — module-
// granular gating, matched by the emitted `mod.rs` append).
// Divergence from Ipê: Ipê aliases Bytes = String; Rust maps Bytes to Vec<u8>.
#[cfg(feature = "encoding")]
pub mod bytes;
#[cfg(feature = "encoding")]
pub use bytes::*;

// `Ipe.Regex` regular expressions + `String.isUrl` (its validator body lives
// here). Behind the `regex` feature (the `regex` crate is optional): a program
// that reaches neither an `Ipe.Regex` kernel nor `String.isUrl` drops `regex` and
// its aho-corasick / regex-automata / regex-syntax subtree.
#[cfg(feature = "regex")]
pub mod regex_kernel;
#[cfg(feature = "regex")]
pub use regex_kernel::*;

// JWT needs `jsonwebtoken` (decode) plus json (the Go-parity JSON encoder for
// the token payload) and crypto (the HMAC / RSA signing primitives the encode
// path reuses for byte-identical-to-Go tokens). Gated on the `jwt` feature,
// which implies both `json` and `crypto`; keeping `jsonwebtoken` out of the
// floor `json` feature so a plain JSON program does not link it.
#[cfg(feature = "jwt")]
pub mod jwt;
#[cfg(feature = "jwt")]
pub use jwt::*;

// `Ipe.Decimal` arbitrary-precision decimal + `Ipe.Money`. Behind the `decimal`
// feature (the `rust_decimal` crate, with its `arrayvec` subtree): `money.rs`
// builds on `decimal.rs`'s `Decimal` newtype, so the two modules gate together.
// A program that reaches no `Decimal.*`/`Money.*` kernel and no `Db` surface
// (whose `SqlValue` numeric columns decode through `rust_decimal`) drops the
// crate. The `stringify.rs` `IpeStringify for Decimal` impl carries the same
// gate, so a program without the feature still compiles.
#[cfg(feature = "decimal")]
pub mod decimal;
#[cfg(feature = "decimal")]
pub use decimal::*;

#[cfg(feature = "compression")]
pub mod compression;
#[cfg(feature = "compression")]
pub use compression::*;

#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "csv")]
pub use csv::*;

#[cfg(feature = "cache_kernel")]
pub mod cache;
#[cfg(feature = "cache_kernel")]
pub use cache::*;

#[cfg(feature = "tui")]
pub mod tui;
// NB: no `pub use tui::*` — its `diff` module name collides with web's `diff`.
// Re-export only the kernels generated code calls unqualified: `tui_app_ui`
// (Element view, `Terminal.appScreen`). `tui_app` is the String-view driver
// reused by the `Ui.cells` raw-cell escape.
#[cfg(feature = "tui")]
pub use tui::{tui_app, tui_app_ui};

// `Ipe.Uuid` v4 / v7 / parse. Behind the `uuid` feature (the `uuid` crate is
// optional): a program that reaches no `Ipe.Uuid` kernel — and no `server` /
// `web` surface (whose runtime modules mint session/CSRF ids via `uuid::new_v4`,
// so both imply `uuid`) — drops the crate.
#[cfg(feature = "uuid")]
pub mod uuid_kernel;
#[cfg(feature = "uuid")]
pub use uuid_kernel::*;

// `Ipe.Secret` — opaque secret-string wrapper. Behind the `secret` feature (its
// `zeroize`-on-`Drop` buffer + `subtle` compare): a program that reaches no
// `Secret.*` kernel and holds no `Secret`-typed value drops the module and
// `zeroize`. `secret` implies `crypto-core` for the shared `subtle`. The JWT/Auth
// surface reaches it (its `Algorithm` is a `secret::Secret`), so `jwt` implies
// `secret`.
#[cfg(feature = "secret")]
pub mod secret;
#[cfg(feature = "secret")]
pub use secret::*;

// Canonical HTTP header-name casing, shared by Ipe.Web, Ipe.Http.Server AND
// the outbound `http_client` response path (`http_client` does NOT
// imply `server`, so `server`-only gating would break an `http_client`-only
// build). Gated on the union of its consumers; a default-features build
// omits it (dead code otherwise). The EMITTED project's `mod.rs` declares it
// unconditionally (base module set — `http_client` is always emitted).
#[cfg(any(
    feature = "server",
    feature = "http_client",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub mod http_header;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub use server::*;
#[cfg(feature = "server")]
pub mod server_stream;
#[cfg(feature = "server")]
pub use server_stream::*;

// ssrf: reqwest-free SSRF deny-private validators. Also enabled when any
// feature that dials a network host is active — db (postgres dial), http_client
// (reqwest), websocket_client — so `VettedDial` is available at every outbound
// network boundary regardless of which transport stack is linked.
// wasm32: NOT pulled in — the browser sandbox, not app code, owns DNS/socket
// access; see `http_client.rs`'s wasm32 doc comment for the full rationale.
#[cfg(any(
    feature = "http_client",
    feature = "websocket_client",
    feature = "db",
    feature = "db-sqlite",
    feature = "db-postgres",
))]
pub mod ssrf;

// wasm32: `Http.get`/`post`/`request` route to `fetch` (see `http_client.rs`'s
// `cfg(target_arch = "wasm32")` split) instead of reqwest.
#[cfg(any(
    feature = "http_client",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub mod http_client;
#[cfg(any(
    feature = "http_client",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub use http_client::*;
#[cfg(feature = "http_client")]
pub mod http_stream;
#[cfg(feature = "http_client")]
pub use http_stream::*;

#[cfg(feature = "email")]
pub mod email;
#[cfg(feature = "email")]
pub use email::*;

// `tea` carries the target-neutral `IpeCmd`/`IpeSub` types plus the native
// (tokio) loop; on wasm32 the wasm-client sink drives the same types with
// the loop halves cfg'd out inside the file.
#[cfg(any(
    feature = "tokio",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub mod tea;
#[cfg(any(
    feature = "tokio",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub use tea::*;

// Browser-WASM TEA sink (mount / patch-apply / delegated events). Only
// meaningful on wasm32; the feature keeps native builds' dep graph untouched.
#[cfg(all(target_arch = "wasm32", feature = "wasm-client"))]
pub mod wasm;
// `Cmd.publish` / `Cmd.publishNoEcho` / `PubSub.publish` / `PubSub.publishNoEcho`
// / `Sub.subscribeTopic` resolve to `ipe_runtime::web::pubsub::*` natively; the
// wasm target has no `web` module (Layer 3 — no tokio/axum to link), so its
// in-tab broker (`wasm::pubsub`) exports the SAME bare kernel names. Selective
// re-export (not `pub use wasm::pubsub::*;`) so the broker's internal `Broker` /
// `Listener` types stay unexported, matching the native `live/pubsub.rs`
// re-export.
#[cfg(all(target_arch = "wasm32", feature = "wasm-client"))]
pub use wasm::pubsub::{
    cmd_publish, cmd_publish_no_echo, pubsub_publish, pubsub_publish_no_echo, sub_subscribe_topic,
};

// wasm32: `Ipe.WebSocket` client routes to `web_sys::WebSocket` (see
// `ws_client.rs`'s `cfg(target_arch = "wasm32")` split) instead of
// tokio-tungstenite.
#[cfg(any(
    feature = "websocket_client",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub mod ws_client;
#[cfg(any(
    feature = "websocket_client",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub use ws_client::*;

// Ipe.Html / Ipe.Ui render surface — the Html/Attribute/Event ADTs + renderer +
// htmlXxx kernel wrappers. Pure (std only), so always available; a non-Web
// Ipe.Ui app renders via Html.toString without the `web` server module. The
// web module re-exports from here.
pub mod html;
pub use html::*;

// Target-neutral DOM data path (diff → `Vec<Patch>`, handler index, form
// decode). Pure over `html`; shared by the Web SSE wire, the Webview bridge,
// and the browser-WASM sink. NOT glob-re-exported at the root: `web` already
// lifts its items, and a second root glob would shadow-collide under `web`.
pub mod dom;

// Shared CSS/style injection-safety encoders (SafeCssValue / SafeCssPropertyName
// / SafeCssSelector / strip_style_close). One policy, one place — imported by the
// Ipe.Ui inline-style path (`ui/render.rs`), the `<style>` sink (`html.rs`),
// and the Ipe.Css renderers (`css.rs`). See design §Q5.
pub mod css_safety;

// Ipe.Css leaf security kernels (safe_value / safe_prop_name / safe_selector /
// strip_style_close_kernel) — the four primitive shims the compiled-source
// `Ipe.Css` funnels every free-string entry through. Re-exported at the
// crate root so the emitted `pub use ipe_runtime::*` resolves the bare kernel
// names that `naming::kernel_name` emits. Typed length/colour constructors +
// the render fold stay pure Ipê in `Std/Css.ipe`.
pub mod css;
pub use css::*;

// In-process telemetry sink (log/error rings + request counters) — always
// compiled so `Ipe.Log.*` can feed it; the Ipe.Web `console` module serves it.
pub mod telemetry;

// Ipe.Ui shared element tree — the general UI abstraction (Element/Attribute/
// Length/Color/...). Backends (Web/Tui/WebView) each render it to their target.
// Referenced by qualified path (`ipe_runtime::ui::*`) from generated code; NOT
// glob-re-exported (its `Attribute` would collide with html's).
pub mod ui;

// Ipe.WebView — native desktop window backend (a TEA app, so gated on the async
// runtime like `tea`). The cross-platform floor (a stub returning a graceful Err)
// keeps `import Ipe.Tea.WebView` linking everywhere; the real wry/tao window backend
// needs the system webview dev libs (staged behind the webview design doc).
// Mirrors Go's webview_stub.go.
#[cfg(feature = "tokio")]
pub mod webview;
#[cfg(feature = "tokio")]
pub use webview::{WebViewAppCfg, WebViewWindowCfg, webview_app};

#[cfg(feature = "web")]
pub mod web;
#[cfg(feature = "web")]
pub use web::*;

// Browser-WASM without the full `web` feature: the wasm TEA sink
// (`wasm/mod.rs`) routes URLs through `web::route`, the pure URL-pattern matcher
// (no server/tokio deps — it compiles on wasm32). Expose that one submodule
// through a lean `web` shell so `crate::web::route` resolves, without pulling
// the heavy `web` surface (axum, SSE, session store). Mirrors the emitted
// browser-WASM module set (`ipe_backend_rust`'s `WASM_RUNTIME_MOD_RS`).
#[cfg(all(target_arch = "wasm32", feature = "wasm-client", not(feature = "web")))]
pub mod web {
    pub mod route;
}

pub mod ffi_polyfills;
pub use ffi_polyfills::*;

// `Ipe.Money` — built on `decimal.rs`'s `Decimal`, so it rides the same
// `decimal` feature (see the `decimal` module above).
#[cfg(feature = "decimal")]
pub mod money;
#[cfg(feature = "decimal")]
pub use money::*;

pub mod math;
pub use math::*;

pub mod bitwise;
pub use bitwise::*;

pub mod dict;
pub use dict::*;
pub mod set;
pub use set::*;

pub mod string;
pub use string::*;

// `Ipe.Locale` — opaque BCP-47 locale handle + locale-aware case mapping.
// The `Locale` struct and `locale_from_tag`/`locale_to_tag`/
// `string_to_upper_in`/`string_to_lower_in` fns are always present (the struct
// is a plain `String` newtype with no optional dep); the ICU4X parsing and
// case-mapping bodies activate under `--features locale`.
pub mod locale;
pub use locale::*;

pub mod basics;
pub use basics::*;

pub mod error;
pub use error::*;

pub mod stringify;
pub use stringify::*;

pub mod char_kernel;
pub use char_kernel::*;

// `Ipe.Char` General_Category predicates (`isAlpha`/`isDigit`/`isLower`/
// `isUpper`/`isAlphaNum`). Behind the `char-category` feature (the sole consumer
// of the `unicode-general-category` table): a program that reaches none of these
// predicates drops the crate. The std-only `Ipe.Char` kernels stay in the
// always-compiled `char_kernel` sibling above.
#[cfg(feature = "char-category")]
pub mod char_category;
#[cfg(feature = "char-category")]
pub use char_category::*;

pub mod list;
pub use list::*;

pub mod io;
pub use io::*;

pub mod debug;
pub use debug::*;

// auth.rs's module-level deps are `bcrypt` (crypto) + `jsonwebtoken` (jwt) +
// `serde_json` (json), all carried by `jwt` (which implies crypto + json). Its
// DB flows (register/login/setRole, which reach `sqlx`/`Db`) are individually
// `#[cfg(feature = "db")]` INSIDE the file, so the module compiles under `jwt`
// alone — a no-DB auth program (hashPassword/verify/signToken) needs `auth`
// without pulling `db`. Gating the module on `jwt` alone (db surface item-gated
// within) mirrors the emitted floor, which declares `pub mod auth;` for any
// auth program and gates the db wrappers per-item.
#[cfg(feature = "jwt")]
pub mod auth;
#[cfg(feature = "jwt")]
pub use auth::*;

// The authenticated `Principal`. Its subject is minted only by the server auth
// middleware and consumed by the DB secured (`…As`) operations, so it is shared
// by whichever of those surfaces a build enables. `principal_mint` stays
// crate-internal; only `principal_subject` is exposed as a kernel.
#[cfg(any(feature = "server", feature = "db", feature = "jwt"))]
pub mod principal;
#[cfg(any(feature = "server", feature = "db", feature = "jwt"))]
pub use principal::{Principal, principal_subject};
