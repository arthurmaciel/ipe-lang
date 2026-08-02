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

// The always-on cryptographic floor: the entropy pair, the SHA-2 hash/HMAC
// family, the RSA sign/verify pair, the typed `Key`/`Mac` newtypes and the
// constant-time compare. wasm32 compiles only the entropy pair + pure hash
// family (the RSA arms are `cfg(feature = "crypto")`). Gated the same as
// `crypto` so the standalone crate's floor mirrors the emitted crate's.
#[cfg(any(
    feature = "crypto",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub mod crypto_core;
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
pub mod file;
// wasm32: `Log.*` routes to `console.{debug,info,warn,error}` (see `log.rs`'s
// `cfg(target_arch = "wasm32")` sink split).
#[cfg(any(
    feature = "tokio",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub mod log;
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
pub mod time;
#[cfg(feature = "tokio")]
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

pub mod url;
pub use url::*;

#[cfg(feature = "db")]
pub mod db;
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
#[cfg(any(
    feature = "tokio",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub use log::*;
pub use random::*;
#[cfg(feature = "tokio")]
pub use system::*;
#[cfg(any(
    feature = "tokio",
    all(target_arch = "wasm32", feature = "wasm-client")
))]
pub use task::*;
pub use time::*;
#[cfg(feature = "tokio")]
pub use trace::*;

pub mod encoding;
pub use encoding::*;

// `Ipe.Bytes` — distinct `Vec<u8>` byte buffer.
// Divergence from Ipê: Ipê aliases Bytes = String; Rust maps Bytes to Vec<u8>.
pub mod bytes;
pub use bytes::*;

pub mod regex_kernel;
pub use regex_kernel::*;

// JWT needs BOTH json (jsonwebtoken decode + the Go-parity JSON encoder for the
// token payload) AND crypto (the HMAC / RSA signing primitives the encode path
// reuses for byte-identical-to-Go tokens). Gating on both keeps a hypothetical
// json-only build sound; generated projects always enable both.
#[cfg(all(feature = "json", feature = "crypto"))]
pub mod jwt;
#[cfg(all(feature = "json", feature = "crypto"))]
pub use jwt::*;

pub mod decimal;
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

pub mod uuid_kernel;
pub use uuid_kernel::*;

// `Ipe.Secret` — opaque secret-string wrapper. Always
// compiled (no cfg gate): a plain newtype over `String` with only `subtle` /
// `zeroize` as deps (both non-optional base deps), so every feature subset
// gets the type.
pub mod secret;
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

// ssrf: reqwest-free SSRF deny-private validators, shared by http_client (reqwest)
// and ws_client (no reqwest). Present whenever either compiles. Consumers import
// via the full `super::ssrf::…` path; the fns are `pub(crate)`, so a
// `pub use ssrf::*;` glob would reexport nothing — intentionally omitted.
// wasm32: NOT pulled in — the browser fetch/WebSocket substitutes have no SSRF
// surface of their own (the sandboxed tab, not app code, owns DNS/socket
// access; see `http_client.rs`'s wasm32 doc comment for the full rationale).
#[cfg(any(feature = "http_client", feature = "websocket_client"))]
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

pub mod ffi_polyfills;
pub use ffi_polyfills::*;

pub mod money;
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

pub mod list;
pub use list::*;

pub mod io;
pub use io::*;

pub mod debug;
pub use debug::*;

// auth.rs's external deps are `bcrypt` (crypto), `jsonwebtoken`/`serde_json`
// (json), AND `sqlx`/`Db` (db — register/login/setRole write the user table).
// Gate on ALL THREE: the old `all(db, json)` gate omitted `crypto`, so a
// `--features db` build (crypto off) compiled auth and failed on unresolved
// `bcrypt`. With `crypto` required, that build excludes auth instead.
#[cfg(all(feature = "crypto", feature = "db", feature = "json"))]
pub mod auth;
#[cfg(all(feature = "crypto", feature = "db", feature = "json"))]
pub use auth::*;
