// Sky Runtime — all modules (for standalone crate compilation).
// In generated projects, this file is overridden by the compiler.

pub mod config;
#[cfg(feature = "config")]
pub mod config_decode;
pub mod core;

#[cfg(feature = "crypto")]
pub mod crypto;
pub mod file;
#[cfg(feature = "tokio")]
pub mod log;
pub mod random;
// `system` is always compiled (not tokio-gated): it owns the process-global
// env RwLock + the `read_env_var` / `read_env_var_os` / `locked_set_var` /
// `locked_set_var_if_absent` / `locked_remove_var` accessors that EVERY module
// (always-compiled telemetry/core/file/csv/… included) must route process-env
// access through for the reader↔mutator serialisation to hold by construction.
// Its Sky-facing helpers return `SkyTask`/`SkyResult` (defined in `core`, no
// tokio dependency) and otherwise use only std, so it compiles without tokio.
pub mod system;
#[cfg(feature = "tokio")]
pub mod task;
pub mod time;
#[cfg(feature = "tokio")]
pub mod trace;
pub use file::*;

pub mod path;
pub use path::*;

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
#[cfg(feature = "tokio")]
pub use log::*;
pub use random::*;
#[cfg(feature = "tokio")]
pub use system::*;
#[cfg(feature = "tokio")]
pub use task::*;
pub use time::*;
#[cfg(feature = "tokio")]
pub use trace::*;

pub mod encoding;
pub use encoding::*;

// `Sky.Core.Bytes` — distinct `Vec<u8>` byte buffer.
// Divergence from Sky: Sky aliases Bytes = String; Rust maps Bytes to Vec<u8>.
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
// NB: no `pub use tui::*` — its `diff` module name collides with live's `diff`.
// Re-export only the kernels generated code calls unqualified: `tui_app`
// (String view, `Tui.program`) + `tui_app_ui` (Element view, `Tui.app`).
#[cfg(feature = "tui")]
pub use tui::{tui_app, tui_app_ui};

pub mod uuid_kernel;
pub use uuid_kernel::*;

// `Sky.Core.Secret` — opaque secret-string wrapper. Always
// compiled (no cfg gate): a plain newtype over `String` with only `subtle` /
// `zeroize` as deps (both non-optional base deps), so every feature subset
// gets the type.
pub mod secret;
pub use secret::*;

// Canonical HTTP header-name casing, shared by Sky.Live, Sky.Http.Server AND
// the outbound `http_client` response path (`http_client` does NOT
// imply `server`, so `server`-only gating would break an `http_client`-only
// build). Gated on the union of its consumers; a default-features build
// omits it (dead code otherwise). The EMITTED project's `mod.rs` declares it
// unconditionally (base module set — `http_client` is always emitted).
#[cfg(any(feature = "server", feature = "http_client"))]
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
// via the full `crate::ssrf::…` path; the fns are `pub(crate)`, so a
// `pub use ssrf::*;` glob would reexport nothing — intentionally omitted.
#[cfg(any(feature = "http_client", feature = "websocket_client"))]
pub mod ssrf;

#[cfg(feature = "http_client")]
pub mod http_client;
#[cfg(feature = "http_client")]
pub use http_client::*;
#[cfg(feature = "http_client")]
pub mod http_stream;
#[cfg(feature = "http_client")]
pub use http_stream::*;

#[cfg(feature = "email")]
pub mod email;
#[cfg(feature = "email")]
pub use email::*;

#[cfg(feature = "tokio")]
pub mod tea;
#[cfg(feature = "tokio")]
pub use tea::*;

#[cfg(feature = "websocket_client")]
pub mod ws_client;
#[cfg(feature = "websocket_client")]
pub use ws_client::*;

// Std.Html / Std.Ui render surface — the Html/Attribute/Event ADTs + renderer +
// htmlXxx kernel wrappers. Pure (std only), so always available; a non-Live
// Std.Ui app renders via Html.toString without the `live` server module. The
// live module re-exports from here.
pub mod html;
pub use html::*;

// Shared CSS/style injection-safety encoders (SafeCssValue / SafeCssPropertyName
// / SafeCssSelector / strip_style_close). One policy, one place — imported by the
// Std.Ui inline-style path (`ui/render.rs`), the `<style>` sink (`html.rs`),
// and the Std.Css renderers (`css.rs`). See design §Q5.
pub mod css_safety;

// Std.Css leaf security kernels (safe_value / safe_prop_name / safe_selector /
// strip_style_close_kernel) — the four primitive shims the compiled-source
// `Std.Css` funnels every free-string entry through. Re-exported at the
// crate root so the emitted `pub use ipe_runtime::*` resolves the bare kernel
// names that `naming::kernel_name` emits. Typed length/colour constructors +
// the render fold stay pure Sky in `Std/Css.sky`.
pub mod css;
pub use css::*;

// In-process telemetry sink (log/error rings + request counters) — always
// compiled so `Std.Log.*` can feed it; the Sky.Live `console` module serves it.
pub mod telemetry;

// Std.Ui shared element tree — the general UI abstraction (Element/Attribute/
// Length/Color/...). Backends (Live/Tui/Webview) each render it to their target.
// Referenced by qualified path (`ipe_runtime::ui::*`) from generated code; NOT
// glob-re-exported (its `Attribute` would collide with html's).
pub mod ui;

// Sky.Webview — native desktop window backend (a TEA app, so gated on the async
// runtime like `tea`). The cross-platform floor (a stub returning a graceful Err)
// keeps `import Std.Webview` linking everywhere; the real wry/tao window backend
// needs the system webview dev libs (staged behind the webview design doc).
// Mirrors Go's webview_stub.go.
#[cfg(feature = "tokio")]
pub mod webview;
#[cfg(feature = "tokio")]
pub use webview::{WebviewAppCfg, WebviewWindowCfg, webview_app};

#[cfg(feature = "live")]
pub mod live;
#[cfg(feature = "live")]
pub use live::*;

pub mod ffi_polyfills;
pub use ffi_polyfills::*;

pub mod money;
pub use money::*;

pub mod math;
pub use math::*;

pub mod dict;
pub use dict::*;
pub mod set;
pub use set::*;

pub mod string;
pub use string::*;

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

// auth.rs's external deps are `bcrypt` (crypto), `jsonwebtoken`/`serde_json`
// (json), AND `sqlx`/`Db` (db — register/login/setRole write the user table).
// Gate on ALL THREE: the old `all(db, json)` gate omitted `crypto`, so a
// `--features db` build (crypto off) compiled auth and failed on unresolved
// `bcrypt`. With `crypto` required, that build excludes auth instead.
#[cfg(all(feature = "crypto", feature = "db", feature = "json"))]
pub mod auth;
#[cfg(all(feature = "crypto", feature = "db", feature = "json"))]
pub use auth::*;
