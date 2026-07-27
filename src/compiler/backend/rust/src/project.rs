//! Project assembly: stitch the fixed templates and the genuinely-emitted user
//! types + functions into the final `src/main.rs`, and pair it with the project
//! `Cargo.toml`.
//!
//! Layout (matching the golden, line by line):
//! ```text
//! <preamble: 1..=30>           header, imports, basic aliases, USER TYPES banner
//! <user types: 31..=43>        emitted from the IR (emit_enum)
//! <blank: 44>
//! <runtime bindings: 45..=127> fixed kernel-wrapper prelude
//! <blank: 128>
//! <user functions: 129..=137>  emitted from the IR (emit_func)
//! <blank: 138>
//! <epilogue: 139..>            Ffi.kernel polyfill, list helpers, entry point
//! ```

use std::collections::{BTreeMap, BTreeSet};
// `rustfmt` is spawned as a subprocess (see `run_rustfmt`), which is a native-
// only path — a browser cannot spawn a process. On `wasm32` the fmt pass is
// disabled (`rust_fmt_disabled`), so these imports and `run_rustfmt` are not
// compiled there.
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
#[cfg(not(target_arch = "wasm32"))]
use std::process::{Command, Stdio};

use ipe_backend::{EmittedProject, RelPath};
use ipe_diagnostics::{DResult, Diagnostic};
use ipe_ir::{IrType, ModPath, Program};

use crate::EmitCtx;
use crate::crate_specs;
use crate::emit_expr::emit_func;
use crate::emit_types::{emit_enum, emit_record_struct};
use crate::preamble::{epilogue, preamble};
use crate::rust_file;
use crate::rust_file::{Partitioned, RustFileId, partition_items};

/// The Rust edition every emitted crate targets; passed to `rustfmt` so a piped
/// format matches what `cargo fmt` produces when it reads the emitted
/// `Cargo.toml`'s `edition = "2024"`.
const EMITTED_EDITION: &str = "2024";

/// Format every generated Rust source file in `files` in place, so the emitted
/// crate is `cargo fmt --check`-clean. Only `src/**.rs` files the backend itself
/// generated (`main.rs`, `ipe_mods/*.rs`, `ipe_runtime/mod.rs`, `config.rs`,
/// `ffi.rs`) are formatted; the vendored runtime source (copied verbatim from
/// the already-clean runtime tree) and non-Rust files (`Cargo.toml`, the wasm
/// `www/` shell) are left untouched.
///
/// The backend concatenates fixed templates with genuinely-emitted expressions,
/// which drift from `rustfmt`'s canonical form (line-length wrapping, `mod`/`use`
/// ordering, blank-line collapsing) — matching those rules by construction would
/// amount to reimplementing `rustfmt`, so the canonical formatter is the source
/// of truth. Runs `rustfmt` over each file's bytes on stdin (no path argument, so
/// it cannot recurse into `mod` files) with the emitted crate's edition and style
/// edition, making the piped result byte-identical to `cargo fmt`.
///
/// Opting out (`IPE_RUST_FMT=0`) skips the pass entirely, for latency-sensitive
/// callers that do not need canonical output — the `ipe watch` hot loop, whose
/// only contract is that the emitted crate compiles and runs, not that it is
/// `rustfmt`-clean. The golden byte-comparison and the example sweep leave the
/// variable unset, so they always format.
///
/// # Errors
///
/// Returns a [`Diagnostic`] if `rustfmt` cannot be spawned (absent toolchain
/// component), exits non-zero, or produces non-UTF-8 output. Formatting fails
/// closed rather than shipping unformatted output: the emitted crate would then
/// fail `cargo fmt --check`, and the byte-compared goldens would drift silently.
fn format_generated_rust_files(files: &mut BTreeMap<RelPath, String>) -> DResult<()> {
    if rust_fmt_disabled() {
        return Ok(());
    }
    // Unreachable on `wasm32` (`rust_fmt_disabled` is always `true` there); the
    // `run_rustfmt` subprocess path is not compiled for wasm.
    #[cfg(not(target_arch = "wasm32"))]
    for (path, text) in files.iter_mut() {
        if !is_generated_rust_path(path.as_str()) {
            continue;
        }
        *text = run_rustfmt(text)?;
    }
    Ok(())
}

/// Whether the post-emit `rustfmt` pass is disabled.
///
/// On `wasm32` (the in-browser compiler) it is ALWAYS disabled: `rustfmt` is a
/// separate process, and a browser cannot spawn one. This is a platform
/// limitation, not a correctness compromise — the emitted Rust is valid, just
/// not canonically formatted. The playground shows source, it does not
/// byte-compare against goldens.
///
/// Off `wasm32` it honours `IPE_RUST_FMT=0`; any other value (or unset) leaves
/// formatting enabled, so the fmt-clean invariant the goldens and sweep depend
/// on holds by default.
fn rust_fmt_disabled() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("IPE_RUST_FMT").is_ok_and(|v| v == "0")
    }
}

/// A generated Rust source path the backend authored and must format: any `.rs`
/// file under `src/`. The vendored `ipe_runtime` source files are copied over the
/// top on disk by the driver and are already clean; the two the backend
/// GENERATES (`mod.rs`, `config.rs`) are the only `ipe_runtime` entries in
/// `files`.
///
/// Native only: its sole caller is the `rustfmt` loop, which is not compiled on
/// `wasm32`.
#[cfg(not(target_arch = "wasm32"))]
fn is_generated_rust_path(rel: &str) -> bool {
    rel.starts_with("src/")
        && std::path::Path::new(rel)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
}

/// Pipe `source` through `rustfmt` on stdin and return the formatted bytes.
///
/// Native only: spawns the `rustfmt` subprocess. On `wasm32` the fmt pass is
/// disabled upstream (`rust_fmt_disabled` returns `true`), so this is never
/// reached and is not compiled in.
#[cfg(not(target_arch = "wasm32"))]
fn run_rustfmt(source: &str) -> DResult<String> {
    let fmt_bug = |detail: String| Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::project::run_rustfmt",
        detail,
    };

    let child = Command::new("rustfmt")
        .arg("--edition")
        .arg(EMITTED_EDITION)
        .arg("--style-edition")
        .arg(EMITTED_EDITION)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    // `rustfmt` is an optional normalization pass — the emitter already produces
    // well-formed, cargo-buildable Rust. When the component is absent (a machine
    // without `rustfmt` installed), skip formatting and return the emitted source
    // unchanged rather than failing the build; only a genuine spawn error remains
    // a compiler bug.
    let mut child = match child {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(source.to_owned()),
        Err(e) => {
            return Err(fmt_bug(format!(
                "cannot spawn `rustfmt` (a pinned-toolchain component): {e}"
            )));
        }
    };

    child
        .stdin
        .take()
        .ok_or_else(|| fmt_bug("rustfmt child stdin was not piped".to_owned()))?
        .write_all(source.as_bytes())
        .map_err(|e| fmt_bug(format!("cannot write source to rustfmt stdin: {e}")))?;

    let output = child
        .wait_with_output()
        .map_err(|e| fmt_bug(format!("cannot read rustfmt output: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(fmt_bug(format!(
            "rustfmt exited {:?}: {}",
            output.status.code(),
            stderr.trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| fmt_bug(format!("rustfmt produced non-UTF-8 output: {e}")))
}

/// The golden program, embedded at compile time. The fixed runtime-bindings
/// block (kernel wrappers, golden lines 45–127) is an exact substring of it.
const GOLDEN: &str = include_str!("../../../../../tests/golden/basics/main.rs");

/// The project `Cargo.toml`, embedded verbatim from the golden. The backend
/// emits the same manifest for every program (dependency set is fixed by the
/// runtime).
const CARGO_TOML: &str = include_str!("../../../../../tests/golden/basics/Cargo.toml");

/// The generated `ipe_runtime/mod.rs` — the curated set of runtime modules whose
/// dependencies are satisfied by [`CARGO_TOML`]. The vendored runtime source
/// ships a fuller `mod.rs` (declaring `uuid` / `live` / `db` / … modules that
/// pull crates outside the base manifest); the driver overwrites it with this
/// trimmed version. The backend emits a fixed base module set, then appends the
/// modules a program's kernels require.
const RUNTIME_MOD_RS: &str = include_str!("../../../../../tests/golden/basics/ipe_runtime/mod.rs");

/// The generated `ipe_runtime/config.rs` (DB/config bindings — empty by default).
const RUNTIME_CONFIG_RS: &str =
    include_str!("../../../../../tests/golden/basics/ipe_runtime/config.rs");

// ── Browser-WASM manifest + runtime module set ─────────────────────────────

/// The `--target wasm` project manifest. A fourth template beside
/// base/db/server: `cdylib` + the wasm-bindgen glue, and — load-bearing for
/// the security gate's dependency floor — NO tokio/axum/sqlx/reqwest/TLS and
/// no `server`/`db`/`live` feature. Dep set = the runtime's proven wasm
/// floor (default + json) plus the browser sink's glue crates.
/// `wasm-bindgen` is pinned exact: the glue is generated by the
/// `wasm-bindgen` CLI, which requires a byte-matching crate version.
const WASM_CARGO_TOML: &str = r#"[package]
name = "ipe-app"
version = "0.1.0"
edition = "2024"
# The crate root is `src/main.rs` (shared layout with the binary targets);
# without this cargo would ALSO infer a `[[bin]]` from that path.
autobins = false

[features]
# Selects the browser-target impls of the shared form-submit helpers in the
# vendored runtime (`cfg(any(feature = "live", feature = "wasm-client"))`).
default = ["wasm-client"]
wasm-client = []

[lib]
# Browser WASM module. Same source layout as the binary targets; the crate
# root stays `src/main.rs`.
name = "ipe_app"
crate-type = ["cdylib"]
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_urlencoded = "0.7"
regex = "1"
unicode-general-category = "1"
base64 = "0.22"
uuid = { version = "1", features = ["v4", "v7", "js"] }
hex = "0.4"
percent-encoding = "2"
chrono = "0.4"
chrono-tz = "0.10"
rust_decimal = { version = "1", features = ["serde"] }
hmac = "0.12"
sha1 = "0.10"
sha2 = "0.10"
md-5 = "0.10"
subtle = "2"
zeroize = "1"
wasm-bindgen = "=0.2.126"
wasm-bindgen-futures = "0.4"
js-sys = "0.3"
# M4 Cmd/Sub browser bridge: `Sub.every`/`Time.sleep`/`Time.every`.
gloo-timers = { version = "0.3", features = ["futures"] }
# `Random.*` / `Crypto.randomBytes` / `Crypto.randomToken` browser substitute
# (`crypto.getRandomValues` via getrandom's `js` backend) — see
# `crypto.rs`/`random.rs`'s `cfg(target_arch = "wasm32")` arms.
getrandom = { version = "0.2", features = ["js"] }
web-sys = { version = "0.3", features = [
  "Window", "Document", "Element", "HtmlElement", "Node", "Text",
  "Event", "EventTarget", "console", "HtmlInputElement",
  "HtmlTextAreaElement", "HtmlSelectElement", "HtmlFormElement",
  "FormData", "KeyboardEvent", "Location", "HtmlDocument",
  "Request", "RequestInit", "RequestMode", "RequestRedirect",
  "Response", "Headers", "AbortController", "AbortSignal",
  "WebSocket", "MessageEvent", "CloseEvent", "ErrorEvent", "BinaryType",
] }
console_error_panic_hook = "0.1"

[profile.dev]
debug = 0
incremental = true
overflow-checks = false

[profile.release]
opt-level = "z"
lto = true
panic = "abort"
strip = true

# Detach this generated crate from any ancestor cargo workspace so it builds
# hermetically even when emitted inside another workspace tree.
[workspace]
"#;

/// The `--target wasm` `ipe_runtime/mod.rs`: exactly the vendored modules
/// that compile on `wasm32-unknown-unknown` under the wasm manifest above —
/// the proven pure floor, the whole `Ipe.Ui` render surface, the
/// target-neutral `dom` data path, the TEA types, the browser sink, and (M4)
/// the Cmd/Sub browser-effects substitutes (`log`, `crypto`'s entropy pair,
/// `http_client`'s `fetch` arm, `ws_client`'s `web_sys::WebSocket` arm — each
/// cfg-split `target_arch = "wasm32"` internally; see their module docs).
/// `trace`, the tokio-bound half of `task` (`block_on`/`Task.run`/
/// `Task.parallel`/`Task.retryWith` — `cfg(not(target_arch = "wasm32"))`
/// inside `task.rs`), the reqwest/tokio-tungstenite-coupled halves of
/// `http_client`/`ws_client`, and every server/db surface stay absent BY
/// CONSTRUCTION (Layer 3 of the security gate).
const WASM_RUNTIME_MOD_RS: &str = "\
// GENERATED by Ipê — do not edit (browser-WASM module set)
pub mod basics;
pub mod bytes;
pub mod char_kernel;
pub mod config;
pub mod core;
pub mod crypto;
pub mod decimal;
pub mod task;
pub mod dict;
pub mod encoding;
pub mod error;
pub mod ffi_polyfills;
pub mod file;
pub mod http_header;
pub mod http_client;
pub mod io;
pub mod json;
pub mod list;
pub mod log;
pub mod math;
pub mod money;
pub mod path;
pub mod random;
pub mod regex_kernel;
pub mod secret;
pub mod set;
pub mod string;
pub mod stringify;
pub mod system;
pub mod telemetry;
pub mod time;
pub mod uuid_kernel;
pub mod css_safety;
pub mod css;
pub mod html;
pub mod dom;
pub mod tea;
pub mod ui;
pub mod wasm;
pub mod ws_client;
pub use basics::*;
pub use bytes::*;
pub use char_kernel::*;
pub use config::*;
pub use core::*;
pub use crypto::*;
pub use decimal::*;
pub use dict::*;
pub use encoding::*;
pub use error::*;
pub use ffi_polyfills::*;
pub use file::*;
pub use http_client::*;
pub use io::*;
pub use json::*;
pub use list::*;
pub use log::*;
pub use math::*;
pub use money::*;
pub use path::*;
pub use random::*;
pub use regex_kernel::*;
pub use secret::*;
pub use set::*;
pub use string::*;
pub use stringify::*;
pub use system::*;
pub use task::*;
pub use time::*;
pub use uuid_kernel::*;
pub use css::*;
pub use html::*;
pub use tea::*;
pub use ws_client::*;
// `Cmd.publish` / `Cmd.publishNoEcho` / `PubSub.publish` / `PubSub.publishNoEcho` /
// `Sub.subscribeTopic` resolve to `ipe_runtime::live::pubsub::*` natively; the
// wasm target has no `live` module (Layer 3 — no tokio/axum to link), so its
// in-tab broker (`wasm::pubsub`) exports the SAME bare kernel names. Selective
// re-export (not `pub use wasm::pubsub::*;`) so the broker's internal `Broker`/
// `Listener` types stay unexported, matching the native `live/pubsub.rs` re-export.
pub use wasm::pubsub::{
    cmd_publish, cmd_publish_no_echo, pubsub_publish, pubsub_publish_no_echo, sub_subscribe_topic,
};
";

/// Prelude module paths with no denotation in the wasm module set — a
/// kernel-wrapper block referencing one of these is dropped from the wasm
/// prelude by [`wasm_runtime_bindings`], UNLESS the block also matches
/// [`WASM_PRESENT_OVERRIDES`] (a landed M4 substitute inside an otherwise
/// mostly-native module). Keyed on module paths (structural), so a prelude
/// drift auto-adapts.
const WASM_ABSENT_MODULE_PATHS: &[&str] = &[
    "ipe_runtime::task::",
    "ipe_runtime::http_client::",
    "ipe_runtime::crypto::",
    // `time.rs` is vendored (its pure calendar kernels are allowlisted), but
    // the clock/sleep entry points inside it need the browser substitute
    // below — most of the module (chrono/tokio helpers) still isn't wasm-safe
    // by default, so the module path stays broadly excluded here too.
    "ipe_runtime::time::",
];

/// Exact wrapper-call substrings RETAINED even though their module path is in
/// [`WASM_ABSENT_MODULE_PATHS`] — the M4 substitute functions, each
/// `cfg(target_arch = "wasm32")`-gated in their own file (`crypto.rs`,
/// `http_client.rs`, `time.rs`). The rest of each of those modules
/// (AEAD/RSA crypto, the reqwest client, tokio clock/sleep) has no wasm32
/// arm and stays excluded — this is a per-function allowlist, not a
/// per-module one, so an un-substituted sibling kernel in the same file can
/// never silently become wasm-reachable.
const WASM_PRESENT_OVERRIDES: &[&str] = &[
    "ipe_runtime::crypto::crypto_random_bytes",
    "ipe_runtime::crypto::crypto_random_token",
    "ipe_runtime::http_client::http_get",
    "ipe_runtime::http_client::http_post",
    "ipe_runtime::http_client::http_request",
    "ipe_runtime::http_client::http_parse_query",
    "ipe_runtime::time::time_sleep",
    "ipe_runtime::time::time_now",
    "ipe_runtime::time::time_unix_millis",
    // `Task.*` pure future combinators (`task.rs`'s ungated half — no tokio
    // dependency). `task_run`/`task_parallel`/`task_retry_with` stay excluded
    // (tokio-bound; no wasm arm).
    "ipe_runtime::task::task_succeed",
    "ipe_runtime::task::task_fail",
    "ipe_runtime::task::task_map",
    "ipe_runtime::task::task_and_then",
    "ipe_runtime::task::task_map_error",
    "ipe_runtime::task::task_on_error",
    "ipe_runtime::task::task_from_result",
    "ipe_runtime::task::task_and_then_result",
    "ipe_runtime::task::task_sequence",
];

/// The wasm-target kernel-wrapper prelude: [`runtime_bindings`] filtered to
/// the blocks whose runtime modules exist in [`WASM_RUNTIME_MOD_RS`] (or are
/// individually allowlisted by [`WASM_PRESENT_OVERRIDES`]). The Layer-1 gate
/// already denies the kernels behind the dropped wrappers, so no emitted call
/// site can reference them.
fn wasm_runtime_bindings() -> DResult<String> {
    let full = runtime_bindings()?;
    let mut out = String::with_capacity(full.len());
    let mut first = true;
    for block in full.split("\npub fn ") {
        let keep = WASM_PRESENT_OVERRIDES.iter().any(|p| block.contains(p))
            || !WASM_ABSENT_MODULE_PATHS.iter().any(|p| block.contains(p));
        if first {
            // The head segment (IpeError re-export + type aliases).
            out.push_str(block);
            first = false;
        } else if keep {
            out.push_str("\npub fn ");
            out.push_str(block);
        }
    }
    Ok(out)
}

/// The `--target wasm` entry: `#[wasm_bindgen(start)]` replacing `fn main`.
/// The panic hook makes a residual trap die with a classified console error;
/// the entry task runs on the browser microtask queue.
const WASM_ENTRY: &str = "\
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn ipe_start() {
    ipe_runtime::wasm::install_panic_hook();
    ipe_runtime::wasm::run_start(ipe_main());
}
";

/// The `[wasm] mode = "hydrate"` second entry: parses island JSON as the
/// user-declared `HydrationState` type (convention: `MainHydrationState`),
/// converts via `fromHydrationState` (convention: `main_from_hydration_state`),
/// and adopts the server-rendered DOM.  On any parse error it falls back to
/// a clean `ipe_main()` init (fault-tolerant: no white screen on a tampered
/// or stale island blob).
const WASM_HYDRATE_ENTRY: &str = "\
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate(model_json: &str) {
    match serde_json::from_str::<crate::MainHydrationState>(model_json) {
        Ok(hs) => {
            let model = crate::main_from_hydration_state(hs);
            ipe_runtime::wasm::run_start(ipe_runtime::wasm::wasm_adopt_app::<
                String, _, _, _, _, _,
            >(
                model,
                crate::main_update,
                crate::main_view,
                crate::main_subscriptions,
            ));
        }
        Err(e) => {
            ipe_runtime::wasm::console_warn(&format!(
                \"hydrate: island JSON rejected ({e}); falling back to clean init\"
            ));
            ipe_runtime::wasm::run_start(ipe_main());
        }
    }
}
";

/// [`epilogue`] with the native `fn main` block replaced by [`WASM_ENTRY`],
/// and — when `ctx.wasm_hydrate_mode` — a second `hydrate` wasm-bindgen
/// export for fault-tolerant SSR takeover (M7 §"Fault-tolerant hydrate").
///
/// Convention-based naming: the entry module is always `Main`, so the Rust
/// names are `MainHydrationState` (the `HydrationState` type alias) and
/// `main_from_hydration_state` (the `fromHydrationState` projection).
fn epilogue_wasm(ctx: &EmitCtx) -> DResult<String> {
    const BANNER: &str = "// ===========================================\n// ENTRY POINT\n";
    let full = epilogue()?;
    let head = full
        .split(BANNER)
        .next()
        .ok_or_else(|| anchor_missing(BANNER))?;
    if head.len() == full.len() {
        return Err(anchor_missing(BANNER));
    }
    let mut out = head.to_owned();
    out.push_str(BANNER);
    out.push_str("// ===========================================\n\n");
    out.push_str(WASM_ENTRY);
    if ctx.wasm_hydrate_mode {
        out.push_str(WASM_HYDRATE_ENTRY);
    }
    Ok(out)
}

/// Layer-3 defence-in-depth: a server-surface flag under the wasm target is
/// unreachable (the Layer-1 gate denies every kernel that sets one); reaching
/// here means the gate and the emitter disagree — fail loud, never emit.
///
/// `websocket` is deliberately NOT in this list (as of M4): `Ipe.WebSocket`'s
/// Task-tier client (connect/connectWith/send/sendBinary/close/closeWithCode)
/// now has a real `web_sys::WebSocket` substitute (`ws_client.rs`'s
/// `cfg(target_arch = "wasm32")` arm), tagged `WasmClient` in the Layer-1
/// registry — `ctx.uses_websocket` is therefore an EXPECTED wasm-reachable
/// flag, not a gate/emitter disagreement.
fn assert_wasm_admissible(ctx: &EmitCtx) -> DResult<()> {
    let denied = [
        ("db", ctx.uses_db),
        ("server", ctx.uses_server),
        ("tui", ctx.uses_tui),
        ("webview", ctx.uses_webview),
        ("email", ctx.uses_email),
        ("auth", ctx.uses_auth),
        ("ffi", ctx.uses_ffi),
    ];
    for (name, used) in denied {
        if used {
            return Err(Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::assert_wasm_admissible",
                detail: format!(
                    "program reached emission with server-only surface `{name}` under \
                     --target wasm — the Layer-1 kernel gate should have rejected it"
                ),
            });
        }
    }
    Ok(())
}

// ── db-enabled manifest fragments ──────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Db kernels.
///
/// The full runtime source tree is copied into the emitted project by the
/// driver; this addition wires `db.rs` (which lives in that tree) into the
/// module namespace so the generated `main.rs` can call the db functions.
const RUNTIME_MOD_RS_DB_APPEND: &str = "pub mod db;\npub use db::*;\npub mod telemetry_spill;\n";

// ── TEA Cmd / Sub ─────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses TEA kernels
/// (`Cmd.none / batch / perform`, `Sub.none / batch / every`, `Time.every`).
///
/// `tea.rs` lives in the runtime source tree (ungated — no cargo feature
/// needed); this addition makes `cmd_none` / `sub_every` / … available in the
/// emitted `main.rs` namespace via `pub use ipe_runtime::*`.
const RUNTIME_MOD_RS_TEA_APPEND: &str = "pub mod tea;\npub use tea::*;\n";

// ── Ipe.Http.Server ──────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses
/// Ipe.Http.Server kernels.
///
/// `server.rs` and `server_stream.rs` are gated by the `server` Cargo
/// feature in the runtime source; `http_stream.rs` (the client-side
/// streaming reader) is always usable when `reqwest` is present.
/// The generated Cargo.toml's default features include `"server"` when
/// these lines are appended.
const RUNTIME_MOD_RS_SERVER_APPEND: &str = "pub mod server;\npub use server::*;\n\
    pub mod server_stream;\npub use server_stream::*;\n\
    pub mod http_stream;\npub use http_stream::*;\n";

// ── Shared transitive dep: http_header ──────────────────────────────────────
//
// `http_header.rs` (a dependency-free leaf exposing `canonical_header`) is part
// of the base `mod.rs` (`tests/golden/basics/ipe_runtime/mod.rs`), because the
// outbound `http_client.rs` response path always calls it. It must NOT be
// conditionally appended here — a conditional `pub mod http_header;` would
// duplicate the base declaration (E0428) for server/live programs.

// ── Ipe.Auth ──────────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Ipe.Auth
/// kernels (`Auth.hashPassword` / `verifyPassword` / `signToken` /
/// `verifyToken` / `register` / `login` / `setRole` etc.).
///
/// `auth.rs` requires `bcrypt` (password hashing) and `jsonwebtoken` (JWT
/// signing/verification); both are unconditional deps in the generated
/// project's `Cargo.toml` (included in the `crypto` and `json` default
/// features), so no manifest surgery is needed — only a `mod.rs` declaration.
const RUNTIME_MOD_RS_AUTH_APPEND: &str = "pub mod auth;\npub use auth::*;\n";

// ── Ipe.WebSocket — outbound WebSocket client ──────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses outbound
/// `Ipe.WebSocket` client kernels.
///
/// `ws_client.rs` is gated by the `websocket_client` Cargo feature in the
/// runtime source; this addition wires it into the module namespace so the
/// generated `main.rs` can call `web_socket_connect` / `web_socket_send` / … and
/// the `sub_subscribe_ws_*` subscription fns via `pub use ipe_runtime::*`.
///
/// `ssrf.rs` (`ws_client`'s SSRF validators) is already part of the base
/// `mod.rs` (the always-present `http_client` module also needs it), so no
/// `ssrf` append is required here. `tea.rs` (whose `IpeSub<M>` the
/// `sub_subscribe_ws_*` fns return) is force-appended alongside this in
/// [`assemble_project_files`], mirroring the `uses_server` rule.
const RUNTIME_MOD_RS_WEBSOCKET_APPEND: &str = "pub mod ws_client;\npub use ws_client::*;\n";
// ── Ipe.Email ───────────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses the `Ipe.Email`
/// `Email.send` kernel.
///
/// `email.rs` is in the runtime source tree (vendored into every emitted crate)
/// but declared only on demand. It depends on `http_client` (in the base
/// `mod.rs`) plus the `base64` / `hmac` / `sha2` / `serde_json` / `reqwest` /
/// `url` crates (all unconditional deps in the base manifest) and `lettre` (the
/// one extra dep added by [`email_cargo_toml`] when `uses_email` is set). No
/// runtime feature flag is involved — the emitted crate vendors the source
/// directly, so declaring the module + adding `lettre` is sufficient.
const RUNTIME_MOD_RS_EMAIL_APPEND: &str = "pub mod email;\npub use email::*;\n";

// ── Ipe.Env ────────────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses the `Ipe.Env`
/// `Env.public` kernel.
///
/// Unlike `ws_client`/`email`, `env_public.rs` is NOT vendored from the
/// source tree — it is generated per-project by [`render_env_public_rs`]
/// (its content is project-specific: the `ipe.toml` `[wasm] publicEnv`
/// allowlist). No extra Cargo dependency or feature flag: `option_env!`/
/// `std::env::var` are both `std`-only.
const RUNTIME_MOD_RS_ENV_PUBLIC_APPEND: &str = "pub mod env_public;\npub use env_public::*;\n";

/// Render the per-project `ipe_runtime/env_public.rs`: `Env.public`'s runtime
/// backing, generated from `allowlist` (`ipe.toml`'s `[wasm] publicEnv`,
/// already validated against the secret-name denylist at PARSE time — see
/// `ipe_cli::project::is_denylisted_public_env_name`).
///
/// One `env_public` fn per target, `#[cfg]`-split: wasm32 embeds each
/// allowlisted key's value at BUILD time via `option_env!` (a browser has no
/// live process environment to read at runtime); native reads the SAME
/// allowlisted key from the live environment via `std::env::var`, so a
/// module shared between a native SSR path and the wasm client behaves
/// identically against both — same allowlist, same set of readable keys,
/// only the READ MECHANISM differs. A key absent from `allowlist` has no
/// match arm on EITHER target and therefore always yields `None` — there is
/// no code path from an arbitrary runtime string back to the raw host/process
/// environment, on either target.
///
/// Each key is emitted via `{key:?}` (Rust's `Debug` string-literal escaping)
/// on BOTH the match-arm pattern and the `option_env!`/`std::env::var`
/// argument, so a key containing a quote or backslash (an unusual but
/// syntactically legal env-var name) round-trips through the generated
/// source safely rather than corrupting it.
#[must_use]
fn render_env_public_rs(allowlist: &[String]) -> String {
    use std::fmt::Write as _;

    let mut wasm_arms = String::new();
    let mut native_arms = String::new();
    for key in allowlist {
        let lit = format!("{key:?}");
        // `write!` into an owned `String` buffer is infallible; the `Result`
        // exists only for the generic `fmt::Write` trait, never actually
        // produced here — discard it rather than `.unwrap()` (clippy's
        // `format_push_string` lint prefers this over `push_str(&format!(..))`,
        // which allocates twice).
        let _ = writeln!(
            wasm_arms,
            "        {lit} => option_env!({lit}).map_or(IpeMaybe::Nothing, \
             |v| IpeMaybe::Just(v.to_owned())),"
        );
        let _ = writeln!(
            native_arms,
            "        {lit} => std::env::var({lit}).map_or(IpeMaybe::Nothing, IpeMaybe::Just),"
        );
    }
    format!(
        "// GENERATED by Ipê — do not edit ([wasm] publicEnv allowlist)\n\
         //\n\
         // `Ipe.Env.public \"KEY\"` resolves ONLY for a name on this project's\n\
         // `ipe.toml` `[wasm] publicEnv` allowlist; every other key returns\n\
         // `Nothing` by construction (no match arm reaches it). wasm32 embeds\n\
         // each value at BUILD time (`option_env!`); native reads the SAME\n\
         // allowlisted key from the live environment (`std::env::var`).\n\
         use super::core::IpeMaybe;\n\
         \n\
         #[cfg(target_arch = \"wasm32\")]\n\
         pub fn env_public(key: String) -> IpeMaybe<String> {{\n\
         \x20   match key.as_str() {{\n\
         {wasm_arms}\
         \x20       _ => IpeMaybe::Nothing,\n\
         \x20   }}\n\
         }}\n\
         \n\
         #[cfg(not(target_arch = \"wasm32\"))]\n\
         pub fn env_public(key: String) -> IpeMaybe<String> {{\n\
         \x20   match key.as_str() {{\n\
         {native_arms}\
         \x20       _ => IpeMaybe::Nothing,\n\
         \x20   }}\n\
         }}\n"
    )
}

// ── Ipe.Ui / Ipe.Html ───────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Ipe.Ui /
/// Ipe.Html render kernels.
///
/// `html.rs` and `ui/mod.rs` are in the runtime source tree; this addition
/// wires both into the module namespace. `html` is always paired with `ui`
/// because the `ui::element` and `ui::render` modules import from `html`.
/// Note: intentionally NOT `pub use ui::*;` because `ui::Attribute` collides
/// with `html::Attribute` (T2 soundness trap) — callers use the fully-qualified
/// `ipe_runtime::ui::element::Attribute` path instead.
///
/// The `css_safety` / `css` declarations are NOT here — they live in
/// [`RUNTIME_MOD_RS_CSS_APPEND`], which is pushed BEFORE this append whenever
/// `uses_ui || uses_css` holds. `html.rs` (`use super::css_safety;`),
/// `ui/render.rs` (`SafeCssPropertyName`/`SafeCssValue`), and
/// `live/style_inject.rs` (`strip_style_close`) all import `css_safety` from the
/// `ipe_runtime` top level, so it MUST be declared before this UI append or
/// those imports fail (E0432) — the caller preserves that ordering. Splitting
/// css out lets a pure-`Ipe.Css` program (no render kernel ⇒ no `uses_ui`) still
/// get the css declarations via `uses_css` alone.
///
/// `dom` (the target-neutral DOM data path) is declared here too, NOT in the
/// base module set: it is mutually referential with `html` (`html.rs` calls
/// `crate::dom::form::decode_form_or_warn`; `dom/{diff,dispatch,form}.rs`
/// import `crate::html::*`), so the two MUST appear together. A non-render
/// program (a plain CLI / headless server) declares neither — declaring `dom`
/// unconditionally in the base while `html` was append-only left `dom`'s
/// `use crate::html::*` unresolved, an `ipe`-exit-0-then-cargo-fail (E0432).
const RUNTIME_MOD_RS_UI_APPEND: &str =
    "pub mod html;\npub use html::*;\npub mod dom;\npub mod ui;\n";

/// Lines appended to `ipe_runtime/mod.rs` when the program uses the `Ipe.Css`
/// leaf security kernels (`Ipe.CssSafety.safeValue` / `safePropName` /
/// `safeSelector` / `stripStyleClose`) — OR any `Ipe.Ui` / `Ipe.Html`
/// render kernel (whose runtime modules import `css_safety` at the top level).
///
/// `css_safety.rs` is a dependency-free, audited leaf; `css.rs` (the four
/// `Ipe.Css` leaf kernels — `safe_value` / `safe_prop_name` / `safe_selector` /
/// `strip_style_close_kernel`) depends only on `css_safety`, and is glob-re-
/// exported (`pub use css::*;`) so the emitted `pub use ipe_runtime::*;`
/// surfaces those bare kernel names that `naming::kernel_name` emits. Both live
/// in the runtime source tree (copied into every emitted project); this append
/// wires them into the trimmed `mod.rs`.
///
/// Pushed BEFORE [`RUNTIME_MOD_RS_UI_APPEND`] because `html.rs` and friends
/// import `css_safety` — it must be declared first. Guarded on
/// `uses_ui || uses_css` and appended AT MOST ONCE, so a program that uses both
/// `Ipe.Css` and `Ipe.Ui` does not emit a duplicate `pub mod css_safety;`
/// (`E0428`).
const RUNTIME_MOD_RS_CSS_APPEND: &str = "pub mod css_safety;\npub mod css;\npub use css::*;\n";

// ── Ipe.Tui / Ipe.Tui ───────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Ipe.Tui /
/// Ipe.Tui app-entry kernels.
///
/// Both `tui/app.rs` and `tui/layout.rs` (and their dependencies `cell.rs`,
/// `diff.rs`, `focus.rs`, `key.rs`) are gated by the `tui` Cargo feature in the
/// runtime source.  This addition wires `tui::tui_app` and `tui::tui_app_ui`
/// into the module namespace so the generated `main.rs` can call them via
/// `ipe_runtime::tui::tui_app_ui`.
///
/// The `ui` module must also be loaded (tui/layout.rs imports `super::ui::Element`)
/// — but `uses_ui` is set whenever `uses_tui` is set (a Tui app always references
/// Ipe.Ui Element/attribute kernels), so `RUNTIME_MOD_RS_UI_APPEND` is already
/// appended by the time this addition fires.
const RUNTIME_MOD_RS_TUI_APPEND: &str = "#[cfg(feature = \"tui\")]\npub mod tui;\n\
     #[cfg(feature = \"tui\")]\npub use tui::{tui_app, tui_app_ui};\n";

// ── Ipe.Webview / Ipe.Webview ───────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Ipe.Webview /
/// Ipe.Webview app-entry kernels.
///
/// `webview.rs` is gated by the `webview` Cargo feature in the runtime source
/// (wry + tao deps). This addition wires `webview::webview_app` and
/// `webview::WebviewWindowCfg` into the module namespace so the generated
/// `main.rs` can call them.
///
/// The `live` module must also be loaded (webview's real backend imports
/// `ipe_runtime::live::dispatch::build_index` and `ipe_runtime::html::*`)
/// — but `uses_live` is forced true when `uses_webview` is true
/// (see `emit_program`), so `RUNTIME_MOD_RS_LIVE_APPEND` is already appended
/// by the time this addition fires.
const RUNTIME_MOD_RS_WEBVIEW_APPEND: &str = "#[cfg(feature = \"webview\")]\npub mod webview;\n\
     #[cfg(feature = \"webview\")]\npub use webview::{webview_app, WebviewWindowCfg};\n";

// ── Ipe.Live / Ipe.Live ─────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Ipe.Live /
/// Ipe.Live app-entry kernels.
///
/// `live/mod.rs` is gated by the `live` Cargo feature in the runtime source;
/// this addition wires the `live` module (and its public re-exports `live_app`,
/// `live_app_routed`, `live_render_static`, `live::route::Route`,
/// `sub_subscribe_topic`, `LiveReq`) into the module namespace so the generated
/// `main.rs` can call them.
///
/// `sub_subscribe_topic` is the `Sub.subscribeTopic` runtime kernel; it
/// lives in `live/pubsub.rs` because it needs the session-aware broker.
///
/// `LiveReq` MUST be re-exported here (transitive-closure invariant). The
/// runtime's `db.rs` module contains a `#[cfg(feature = "live")] impl IpeRow for
/// super::LiveReq` block — `super::LiveReq` means `ipe_runtime::LiveReq`. In the
/// real runtime source `mod.rs` uses `pub use live::*;` which surfaces `LiveReq`
/// (via `live/mod.rs`'s own `pub use req::*;`), but the emitted project uses a
/// selective export list.  Without `LiveReq` here, any program that uses BOTH Db
/// and Live kernels fails with E0412 (`LiveReq in super` not found) at
/// `db.rs:impl IpeRow for super::LiveReq`.
///
/// The `route` sub-module is referenced by path (`ipe_runtime::live::route::Route`)
/// not via `pub use live::*;` (to avoid surfacing the internal `store` / `req`
/// internals in the top-level namespace).
const RUNTIME_MOD_RS_LIVE_APPEND: &str = "#[cfg(feature = \"live\")]\npub mod live;\n\
     #[cfg(feature = \"live\")]\npub use live::{live_app, live_app_routed, live_render_static, sub_subscribe_topic, cmd_publish, cmd_publish_no_echo, pubsub_publish, pubsub_publish_no_echo, LiveReq};\n";

/// The `IpeCmd<M>` and `IpeSub<M>` project-level type aliases emitted when the
/// program uses TEA kernels. Placed immediately after `runtime_bindings()` (the
/// block that also contains `IpeTask<A>` and `Decoder<T>`).
const TEA_TYPE_ALIASES: &str = "pub type IpeCmd<M> = ipe_runtime::tea::IpeCmd<M>;\n\
     pub type IpeSub<M> = ipe_runtime::tea::IpeSub<M>;\n";

// ── Ipe.Auth — concrete wrappers emitted when uses_auth is true ────────

/// Concrete wrappers appended to `main.rs` when the program uses Ipe.Auth
/// kernels.  Each wrapper specialises the generic `E` type parameter to
/// `IpeError` so call sites in user function bodies compile without requiring
/// a turbofish annotation.
///
/// `auth_sign_token` / `auth_verify_token` take a Ipê-typed
/// `ipe_runtime::secret::Secret` (not `String`) at this boundary — "secrets
/// are typed, never `fmt`-stringified" (`PRINCIPLES.md`). The wrapper reveals
/// it via `ipe_runtime::secret::secret_reveal` immediately before delegating
/// to the runtime's `String`-typed `ipe_runtime::auth::{auth_sign_token,
/// auth_verify_token}` — the runtime crate's own low-level signature is left
/// unchanged (it has no dependency on `secret.rs`); the typed boundary lives
/// entirely at this Ipê-facing wrapper, matching the fix spec's design.
///
/// `auth_register`, `auth_login`, and `auth_set_role` are gated on
/// `#[cfg(feature = "db")]` in the runtime source, so the three wrappers
/// here are also gated.  A non-db Auth-only program (using only `hashPassword`
/// / `verifyPassword` / `signToken` / `verifyToken`) will compile the four
/// ungated wrappers + the `passwordStrength` helper and ignore the db-gated
/// three.  When `uses_db` is also true the `db` feature is in the generated
/// project's defaults and the db-gated wrappers become active.
const AUTH_WRAPPERS: &str = "\
pub fn auth_hash_password(pw: String) -> IpeResult<IpeError, String> {\n    \
    ipe_runtime::auth::auth_hash_password(pw)\n\
}\n\
pub fn auth_hash_password_cost(pw: String, cost: i64) -> IpeResult<IpeError, String> {\n    \
    ipe_runtime::auth::auth_hash_password_cost(pw, cost)\n\
}\n\
pub fn auth_verify_password(pw: String, hash: String) -> IpeResult<IpeError, bool> {\n    \
    ipe_runtime::auth::auth_verify_password(pw, hash)\n\
}\n\
pub fn auth_password_strength(pw: String) -> IpeResult<IpeError, String> {\n    \
    ipe_runtime::auth::auth_password_strength(pw)\n\
}\n\
pub fn auth_sign_token(\n    \
    secret: ipe_runtime::secret::Secret, claims: HashMap<String, String>, expiry_seconds: i64,\n\
) -> IpeResult<IpeError, String> {\n    \
    ipe_runtime::auth::auth_sign_token(ipe_runtime::secret::secret_reveal(secret), claims, expiry_seconds)\n\
}\n\
pub fn auth_verify_token(secret: ipe_runtime::secret::Secret, token: String) -> IpeResult<IpeError, HashMap<String, String>> {\n    \
    ipe_runtime::auth::auth_verify_token(ipe_runtime::secret::secret_reveal(secret), token)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_register(conn: Db, email: String, password: String) -> IpeTask<i64> {\n    \
    ipe_runtime::auth::auth_register(conn, email, password)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_login(conn: Db, email: String, password: String) -> IpeTask<i64> {\n    \
    ipe_runtime::auth::auth_login(conn, email, password)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_set_role(conn: Db, user_id: i64, role: String) -> IpeTask<()> {\n    \
    ipe_runtime::auth::auth_set_role(conn, user_id, role)\n\
}\n\
";

/// The `ipe_runtime/config.rs` emitted for db-enabled programs targeting
/// `SQLite` (the default driver). Replaces the no-op default stub with the `SQLite`
/// type aliases + helper fns the `db.rs` module requires. Mirrors
/// `src/runtime/rust/src/config.rs` verbatim, keeping the
/// `#[cfg(feature = "db")]` / `#[cfg(not(feature = "db"))]` guards so a
/// non-db build (hypothetically possible via feature flag override) degrades
/// gracefully rather than failing with undefined types.
const RUNTIME_CONFIG_RS_DB_SQLITE: &str =
    include_str!("../../../../../src/runtime/rust/src/config.rs");

/// The `ipe_runtime/config.rs` emitted for db-enabled programs targeting
/// Postgres (`ipe.toml`'s `[database] driver = "postgres"`). Same symbol
/// surface as [`RUNTIME_CONFIG_RS_DB_SQLITE`] (`DbPool`/`DbRow`/`ipe_db_url`/
/// `db_last_insert_id`/`db_format_sql`/`DB_USES_RETURNING_ID`/
/// `db_auto_id_column`), so `db.rs` is byte-identical across both driver
/// builds.
const RUNTIME_CONFIG_RS_DB_POSTGRES: &str =
    include_str!("../../../../../src/runtime/rust/src/config_postgres.rs");

/// The `Diagnostic::CompilerBug` raised when a golden anchor is absent — a
/// drifted-golden invariant violation, surfaced (IPE-I0203) instead of a silent
/// empty slice.
fn anchor_missing(anchor: &str) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "backend.golden_anchor",
        detail: format!("golden anchor {anchor:?} not found in the embedded M0 golden"),
    }
}

/// Look up `file_id`'s bucket in `buckets` via `.get` (never `[]` — indexing
/// panics on a missing key, and `clippy::indexing_slicing` is denied in this
/// workspace). Every `file_id` this is called with comes from
/// `Partitioned::type_order`/`func_order` themselves (built by
/// `partition_items` from the SAME map, `emit_program`'s only caller), so a
/// miss here can only mean an internal invariant violation — surfaced as
/// [`Diagnostic::CompilerBug`], never a panic.
fn bucket_or_bug<'p>(
    buckets: &'p BTreeMap<RustFileId, (Vec<&'p ipe_ir::EnumDef>, Vec<&'p ipe_ir::Func>)>,
    file_id: &RustFileId,
) -> DResult<&'p (Vec<&'p ipe_ir::EnumDef>, Vec<&'p ipe_ir::Func>)> {
    buckets.get(file_id).ok_or_else(|| Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::project::emit_program",
        detail: "type_order/func_order references a home missing from partition_items' own \
                 buckets — internal invariant violation"
            .to_owned(),
    })
}

/// The fixed kernel-wrapper prelude emitted between the user types and the user
/// functions (golden lines 45–127).
///
/// These bindings (`IpeError`, the `log_*` / `system_*` / `time_*` / … wrappers)
/// are identical for every program, so they are sliced out of the embedded
/// golden rather than hand-retyped — the same drift-free strategy the
/// preamble/epilogue use. The slice is anchored entirely on its *own* content
/// (the first alias and the final `http_parse_query` wrapper), independent of
/// the surrounding user code.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] (IPE-I0203) if either anchor is absent
/// from the embedded golden — a drifted-golden invariant violation, surfaced
/// instead of a silent empty slice.
fn runtime_bindings() -> DResult<&'static str> {
    const START: &str = "pub use ipe_runtime::error::IpeError;";
    const END: &str = "    ipe_runtime::http_client::http_parse_query(raw)\n}\n";
    let start = GOLDEN.find(START).ok_or_else(|| anchor_missing(START))?;
    let rest = GOLDEN.get(start..).ok_or_else(|| anchor_missing(START))?;
    let end_in_rest = rest.find(END).ok_or_else(|| anchor_missing(END))?;
    let end = start + end_in_rest + END.len();
    GOLDEN.get(start..end).ok_or_else(|| anchor_missing(END))
}

/// Emit the complete project for `program`.
#[allow(clippy::too_many_lines)]
pub fn emit_program(ctx: &EmitCtx, program: &Program) -> DResult<EmittedProject> {
    // Partition every user item by the Rust file it belongs in. The
    // number of DISTINCT `RustFileId::IpeModule` buckets — NEVER counting the
    // always-possible `Spine` bucket (§3.3: "counts `IpeModule` buckets only,
    // never `Spine`") — is the trigger for the real per-module split:
    //   • 0 or 1 distinct IpeModule bucket → the Spine-collapse invariant
    //     fires and we emit today's byte-identical single `src/main.rs`.
    //   • 2+ → the real split materialises (`emit_spine` + one
    //     `emit_module_file` per bucket + the `main.rs` barrel lines).
    let partition = partition_items(program, ctx.interner);

    // The DISTINCT `IpeModule` homes, in first-encounter (linker/topological)
    // order — the SAME warm/cold-stable order `type_order`/`func_order` use
    // (see [`Partitioned`]). A module can appear in `func_order` but not
    // `type_order` (e.g. a func-only module like `mm_diamond`'s `D`), so the
    // union is taken with `type_order` first, then any func-only home appended
    // in its own first-encounter position. This ordered list drives BOTH the
    // deterministic barrel lines and the per-module file emission.
    let mut module_homes: Vec<RustFileId> = Vec::new();
    let mut seen: BTreeSet<RustFileId> = BTreeSet::new();
    for id in partition
        .type_order
        .iter()
        .chain(partition.func_order.iter())
    {
        if seen.insert(id.clone()) {
            module_homes.push(id.clone());
        }
    }

    // Fail closed if two DISTINCT module homes fold to the same `mod_ident`
    // BEFORE any `mod` decl / source file is written. The `home -> mod_ident`
    // fold is injective (`naming::module_prefix` escapes in-segment `_`), so
    // this can only fire on a genuine internal bug — but it MUST be wired: an
    // unwired gate would let a collision write two identical `mod` decls (E0428)
    // and silently overwrite the first module's source file.
    rust_file::assert_mod_idents_unique(&module_homes, ctx.interner)?;

    // (design doc §2.2): fail closed if a
    // synthesised record struct's name collides with a user enum's name, a
    // function name, or a `mod_ident`. In the single-file collapse case no
    // `mod` declarations are written, so the honest set is empty; in the real
    // split every `IpeModule` bucket contributes its `mod_ident` — its
    // intra-set uniqueness is proven by the gate above; this check is the
    // DISJOINTNESS obligation against the record-struct namespace.
    let mod_idents: BTreeSet<String> = if module_homes.len() >= 2 {
        module_homes
            .iter()
            .filter_map(|id| match id {
                RustFileId::IpeModule(home) => {
                    Some(rust_file::resolve_mod_ident(home, ctx.interner))
                }
                RustFileId::Spine => None,
            })
            .collect::<DResult<BTreeSet<String>>>()?
    } else {
        BTreeSet::new()
    };
    ctx.assert_record_structs_disjoint_from_type_namespace(&mod_idents)?;

    // The emitted Rust source files (`src/main.rs` plus, in the real split,
    // one `src/ipe_mods/<ident>.rs` per module). The manifest + runtime-module
    // files below are file-count-agnostic and shared by both branches.
    let mut rust_sources: Vec<(RelPath, String)> = Vec::new();

    if module_homes.len() >= 2 {
        // ── The real per-Ipê-module split (§2.1/§3.3) ────────────────────────
        // `main.rs` = the Spine tier (preamble, SqlValue/SqlField enums,
        // record structs, DB-projection impls, kernel-wrapper prelude, epilogue,
        // `fn main()`) + the flat glob barrel that re-exports every module's
        // items at the crate root.
        let mut main_rs = emit_spine(ctx, program)?;
        // Barrel lines (§2.1), one pair per distinct IpeModule home, in the
        // deterministic first-encounter order computed above:
        //   #[path = "ipe_mods/ipe_mod_<home>.rs"]
        //   mod ipe_mod_<home>;
        //   pub(crate) use ipe_mod_<home>::*;
        // The `#[path]` attribute is load-bearing: `main.rs` is the crate root,
        // so a BARE `mod ipe_mod_<home>;` would resolve to a crate-root sibling
        // `src/ipe_mod_<home>.rs`, NOT the `src/ipe_mods/<ident>.rs` file this
        // design places (§2.1). `#[path]` is resolved relative to the declaring
        // file's directory (`src/`), so it points the module at the real file
        // under `ipe_mods/` — closing an E0583 "file not found for module"
        // exit-0-then-cargo-fail (THE SEAL) that a bare `mod` decl would ship.
        // Because every user name is already globally unique (§1.3) and this
        // re-exports every module at the crate root, each per-module file's
        // `use crate::*;` sees every Spine item and every other module's item.
        main_rs.push('\n');
        for id in &module_homes {
            let RustFileId::IpeModule(home) = id else {
                continue;
            };
            let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
            // Built via `push_str` fragments rather than one `push_str(&format!)`
            // to satisfy `clippy::format_push_string` (denied via pedantic) —
            // no intermediate allocation, same bytes.
            main_rs.push_str("#[path = \"ipe_mods/");
            main_rs.push_str(&ident);
            main_rs.push_str(".rs\"]\nmod ");
            main_rs.push_str(&ident);
            main_rs.push_str(";\npub(crate) use ");
            main_rs.push_str(&ident);
            main_rs.push_str("::*;\n");
        }
        rust_sources.push((RelPath::new("src/main.rs")?, main_rs));

        // One `src/ipe_mods/<ident>.rs` per module, carrying ONLY that home's
        // `pub(crate)` items behind a `use crate::*;` glob header.
        for id in &module_homes {
            let RustFileId::IpeModule(home) = id else {
                continue;
            };
            let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
            let file = emit_module_file(ctx, program, id)?;
            rust_sources.push((RelPath::new(format!("src/ipe_mods/{ident}.rs"))?, file));
        }
    } else {
        // ── The Spine-collapse invariant (§3.3) ──────────────────────────────
        // Exactly ONE distinct `IpeModule` bucket (or none): inline that one
        // module's types/funcs into a single `src/main.rs`. THIS BRANCH IS
        // LOAD-BEARING — every single-module golden must stay byte-identical to
        // this inline layout: preamble, user types (via `type_order`), Spine
        // enums, record structs, DB-projection impls, kernel-wrapper prelude,
        // user funcs (via `func_order`), epilogue, G3.
        let Partitioned {
            buckets,
            type_order,
            func_order,
        } = &partition;

        // Capacity hint only — bytes pushed are identical. `GOLDEN` (the
        // embedded reference main.rs the preamble/epilogue are cut from) is a
        // sound floor for the fixed sections; user code grows beyond it via the
        // usual doubling.
        let mut out = String::with_capacity(GOLDEN.len() + 4096);
        out.push_str(&preamble()?);
        // The preamble ends with the USER-TYPES banner and its single closing
        // blank line. Anything emitted below (types, record structs, Db
        // projections) is that section's body; the runtime bindings that follow
        // need one blank line of separation from it. When the section is empty,
        // the banner's own closing blank already provides that separation, so a
        // second blank must NOT be pushed (rustfmt collapses runs of blank lines
        // to one — emitting two would fail `cargo fmt --check`).
        let after_banner = out.len();

        // User types, walked via `type_order` — `partition_items`'s
        // FIRST-ENCOUNTER order over `program.modules[..].types`, a
        // warm/cold-stable linker topological order (NOT alphabetical, NOT
        // symbol-id — see [`Partitioned`]'s doc comment). A single-bucket
        // program has nothing to reorder; this is a byte-identical no-op.
        for file_id in type_order {
            let (enums, _) = bucket_or_bug(buckets, file_id)?;
            for &def in enums {
                out.push_str(&emit_enum(ctx, def)?);
            }
        }
        if let Some((spine_enums, _)) = buckets.get(&RustFileId::Spine) {
            for &def in spine_enums {
                out.push_str(&emit_enum(ctx, def)?);
            }
        }
        // Synthesised record structs, one per distinct closed record shape.
        // Item order is irrelevant in Rust, so these can reference one another
        // freely; a program with no records emits nothing here.
        for rec in ctx.record_structs() {
            out.push_str(&emit_record_struct(ctx, rec)?);
        }

        // boundary-projection impl blocks.  When the program uses Db
        // kernels, the lowerer injected synthetic `SqlValue` / `SqlField`
        // enums.  The Db call sites need to project Ipê ADT values to the
        // runtime's concrete `SqlParam` / `Option<SqlParam>`.
        if ctx.uses_db {
            out.push_str(&emit_db_projection_impls(ctx)?);
        }

        if out.len() != after_banner {
            out.push('\n');
        }

        // Fixed kernel-wrapper prelude (IpeError, IpeTask<A>, Decoder<T>, …);
        // the wasm target takes the floor-filtered subset.
        match ctx.target {
            ipe_ir::Target::Native => out.push_str(runtime_bindings()?),
            ipe_ir::Target::WasmClient => out.push_str(&wasm_runtime_bindings()?),
        }

        // TEA kernels → the IpeCmd<M> / IpeSub<M> type aliases.
        if ctx.uses_tea {
            out.push_str(TEA_TYPE_ALIASES);
        }
        // Ipe.Auth kernels → concrete E = IpeError wrappers.
        if ctx.uses_auth {
            out.push_str(AUTH_WRAPPERS);
        }
        out.push('\n');

        // User functions, walked via `func_order` (its OWN first-encounter
        // order over `program.modules[..].funcs`). `partition_items` never
        // routes a `Func` into `Spine`, so funcs land purely in `IpeModule`
        // buckets.
        let before_funcs = out.len();
        for file_id in func_order {
            let (_, funcs) = bucket_or_bug(buckets, file_id)?;
            for &func in funcs {
                out.push_str(&emit_func(ctx, func)?);
            }
        }
        // Separate the user functions from the epilogue with one blank line. The
        // blank pushed above already separates an EMPTY function section from the
        // epilogue, so a second blank is added only when functions were emitted
        // (rustfmt collapses blank-line runs to one; two would fail fmt-check).
        if out.len() != before_funcs {
            out.push('\n');
        }

        match ctx.target {
            ipe_ir::Target::Native => out.push_str(&epilogue()?),
            ipe_ir::Target::WasmClient => out.push_str(&epilogue_wasm(ctx)?),
        }

        // ── G3: Webview main-thread entry switch ──────────────────────────────
        // Ipe.Webview's `tao` event loop requires the process's TRUE main
        // thread on every OS. The standard entry uses `block_on`; Webview MUST
        // use `block_on_current_thread`. (In the real split, `emit_spine`
        // performs this same switch on its own — the anchor lives in the
        // epilogue, which is Spine-only.)
        if ctx.uses_webview {
            const BLOCK_ON_ANCHOR: &str = "block_on(ipe_main())";
            const BLOCK_ON_THREAD_REPLACEMENT: &str = "block_on_current_thread(ipe_main())";
            let replaced = out.replacen(BLOCK_ON_ANCHOR, BLOCK_ON_THREAD_REPLACEMENT, 1);
            if replaced == out {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::project::emit_program::G3_block_on",
                    detail: format!(
                        "G3 webview entry-switch: anchor {BLOCK_ON_ANCHOR:?} not found in \
                         emitted output — epilogue golden has drifted; Ipe.Webview REQUIRES \
                         block_on_current_thread (tao/Cocoa NSApplication mandates the \
                         process main thread on macOS; omitting the switch is a runtime crash)"
                    ),
                });
            }
            out = replaced;
        }

        rust_sources.push((RelPath::new("src/main.rs")?, out));
    }

    assemble_project_files(ctx, rust_sources)
}

/// Assemble the final [`EmittedProject`] from the already-rendered Rust source
/// files (`src/main.rs` plus, in the real split, each `src/ipe_mods/<ident>.rs`)
/// — appending the manifest (`Cargo.toml`) and the trimmed runtime module
/// files (`ipe_runtime/mod.rs` + `config.rs`).
///
/// **Factored out of [`emit_program`] (design doc §4.4).** This block
/// is file-count-agnostic — it depends ONLY on `ctx`'s used-kernel flags, never
/// on how many Rust source files `rust_sources` carries — so the salsa
/// `emit_manifest` query (`ipe_db`) reuses it verbatim after assembling
/// `rust_sources` from the per-file [`emit_spine`]/[`emit_module_file`] query
/// outputs, guaranteeing byte-identity with the single-file `emit_program`
/// path. Kept a shared helper rather than duplicated, exactly as §4.4 requires.
///
/// # Errors
///
/// Propagates any [`Diagnostic`] from the `Cargo.toml`/runtime-module
/// construction (e.g. a drifted server/db/tui/webview manifest anchor).
#[allow(clippy::too_many_lines)] // one linear manifest/runtime assembly pass
fn assemble_project_files(
    ctx: &EmitCtx,
    rust_sources: Vec<(RelPath, String)>,
) -> DResult<EmittedProject> {
    // ── Browser-WASM branch: fixed manifest + module set ─────────────────────
    // The wasm manifest/runtime shape is a closed template (Layer 3 of the
    // security gate: no tokio/axum/sqlx/TLS to link a credential through), so
    // none of the native manifest surgeries below apply.
    if ctx.target == ipe_ir::Target::WasmClient {
        assert_wasm_admissible(ctx)?;
        let mut files = BTreeMap::new();
        for (path, text) in rust_sources {
            files.insert(path, text);
        }
        // Host/global cargo configs may carry native-linker rustflags (e.g.
        // mold), which rust-lld rejects for wasm32. A target-scoped set here
        // takes precedence — and it must be NON-empty (cargo treats an empty
        // array as unset and falls back to `build.rustflags`).
        files.insert(
            RelPath::new(".cargo/config.toml")?,
            "[target.wasm32-unknown-unknown]\n\
             # Non-empty on purpose: an empty array would not override a host\n\
             # config's native `build.rustflags` (e.g. a mold link-arg).\n\
             rustflags = [\"-C\", \"debuginfo=0\"]\n"
                .to_owned(),
        );
        let wasm_mod_rs = if ctx.uses_env_public {
            let mut m = WASM_RUNTIME_MOD_RS.to_owned();
            m.push_str(RUNTIME_MOD_RS_ENV_PUBLIC_APPEND);
            m
        } else {
            WASM_RUNTIME_MOD_RS.to_owned()
        };
        files.insert(RelPath::new("src/ipe_runtime/mod.rs")?, wasm_mod_rs);
        files.insert(
            RelPath::new("src/ipe_runtime/config.rs")?,
            RUNTIME_CONFIG_RS.to_owned(),
        );
        if ctx.uses_env_public {
            files.insert(
                RelPath::new("src/ipe_runtime/env_public.rs")?,
                render_env_public_rs(&ctx.wasm_public_env),
            );
        }
        // The static browser shell (CSP: `script-src 'self' 'wasm-unsafe-eval'`
        // — wasm instantiation allowed, JS eval not; boot module external so
        // no inline allowance is needed). The wasm-bindgen CLI drops the
        // bundle beside them under `www/pkg/`.
        files.insert(
            RelPath::new("www/index.html")?,
            "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
             <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'self'; \
             script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
             connect-src 'self'\">\n\
             <title>Ip\u{ea} App</title>\n</head>\n<body>\n\
             <script type=\"module\" src=\"./boot.js\"></script>\n</body>\n</html>\n"
                .to_owned(),
        );
        files.insert(
            RelPath::new("www/boot.js")?,
            "import init from \"./pkg/ipe_app.js\";\ninit();\n".to_owned(),
        );
        format_generated_rust_files(&mut files)?;
        return Ok(EmittedProject {
            files,
            cargo_toml: WASM_CARGO_TOML.to_owned(),
        });
    }

    // ── Manifest + runtime module files ──────────────────────────────────────
    // The driver (ipe) first copies the full runtime source tree into
    // `<out>/src/ipe_runtime/`, then writes the emitted files over the top.
    // So we only need to emit the files that differ from the raw source tree:
    //
    //   • `mod.rs` — trimmed to the kernel set the program uses (non-db path
    //     keeps the default; db path appends `pub mod db; pub use db::*;`).
    //   • `config.rs` — the stub for non-db; the full db-type-alias file
    //     for db programs (provides `DbPool`, `DbRow`, `IPE_DB_URL`, …).
    //   • `Cargo.toml` — adds `db` to default features + `sqlx` dep for db.
    // Build the manifest + runtime module selection based on which kernel groups
    // are used. Db, TEA, and Server are independent features; a program may use
    // any combination. The order: db first, then server; both modify the same
    // base manifest so we chain the transformations.
    let (cargo_toml, runtime_config_rs) = if ctx.uses_db {
        let cfg = match ctx.db_driver {
            crate::DbDriver::Sqlite => RUNTIME_CONFIG_RS_DB_SQLITE,
            crate::DbDriver::Postgres => RUNTIME_CONFIG_RS_DB_POSTGRES,
        };
        (db_cargo_toml(ctx.db_driver)?, cfg.to_owned())
    } else {
        (CARGO_TOML.to_owned(), RUNTIME_CONFIG_RS.to_owned())
    };
    // Apply server manifest extension on top of whichever base was chosen above.
    // Live also needs axum + tower-http (the live runtime uses axum
    // internally).  Apply server_cargo_toml for both `uses_server`, `uses_live`,
    // and `uses_webview` (Webview's real backend imports from the live module,
    // which uses axum; the function is idempotent when multiple flags are set).
    let cargo_toml = if ctx.uses_server || ctx.uses_live || ctx.uses_webview {
        server_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // When the program uses Live, add "live" to the default features.
    // The base manifest already declares `live = []` as a non-default feature;
    // we just need to promote it to the `default` list so the compiled binary
    // includes the `live` module.
    // Webview's real backend imports `ipe_runtime::live::dispatch`
    // (for `build_index`) and `ipe_runtime::html::render_html` — both gated
    // behind the `live` feature. Force-promote `live` for Webview as well.
    let cargo_toml = if ctx.uses_live || ctx.uses_webview {
        live_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // When the program uses Tui, add "tui" to the default features
    // and inject the crossterm + unicode-width deps required by the tui runtime.
    // The base manifest declares `tui = []` as a non-default feature; we promote
    // it and add the deps so the compiled binary includes the `tui` module.
    let cargo_toml = if ctx.uses_tui {
        tui_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // When the program uses Webview, add "webview" to the default
    // features and inject the wry + tao deps required by the real native-window
    // backend. The base manifest declares `webview = []` as a non-default feature;
    // this function promotes it, wires it to wry + tao, and adds those deps.
    let cargo_toml = if ctx.uses_webview {
        webview_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // Ipe.WebSocket client: promote the `websocket_client` feature +
    // add tokio-tungstenite + tokio `"sync"`. Applied last; idempotent on the
    // tokio `"sync"` step so it composes with any prior server/live/tui surgery.
    let cargo_toml = if ctx.uses_websocket {
        websocket_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // Ipe.Email: `email.rs` needs the `lettre` crate for the SMTP transport
    // (every other crate it uses — `base64` / `hmac` / `sha2` / `serde_json` /
    // `reqwest` / `url` — is already an unconditional base-manifest dep). Add
    // `lettre` only when the program uses `Email.send`.
    let cargo_toml = if ctx.uses_email {
        email_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // Foreign-crate FFI: append the bound crates' pinned [dependencies] lines
    // (exact versions + effective feature sets, pre-merged by the driver).
    let cargo_toml = if ctx.uses_ffi {
        ffi_cargo_toml(&cargo_toml, ctx)?
    } else {
        cargo_toml
    };
    // mod.rs starts from the base default and gains extra `pub mod` lines for
    // each kernel group the program uses.
    let runtime_mod_rs = {
        let mut mod_rs = RUNTIME_MOD_RS.to_owned();
        if ctx.uses_db {
            mod_rs.push_str(RUNTIME_MOD_RS_DB_APPEND);
        }
        // `tea` must be declared whenever any included module's `use crate::tea`
        // closure references it — NOT only when user code names a TEA kernel
        // directly. Every appended module that imports `IpeCmd`/`IpeSub` forces it:
        //   • `uses_server` → `http_stream.rs` (`use super::*;` → `IpeSub`);
        //   • `uses_websocket` → `ws_client.rs`'s `sub_subscribe_ws_*` (`IpeSub<M>`);
        //   • `uses_live` → `live/mod.rs` + `live/pubsub.rs` (`use crate::tea::{IpeCmd, IpeSub}`);
        //   • `uses_tui` → `tui/app.rs` (`use super::super::tea::{…, IpeCmd, IpeSub, …}`);
        //   • `uses_webview` → `webview.rs` (`use super::tea::{IpeCmd, IpeSub}`).
        // These imports are unconditional in the runtime source (not feature-gated),
        // so a live/tui/webview program with no explicit `Cmd`/`Sub` kernel (e.g.
        // `Live.renderStatic` from a CLI) still needs `tea`. Guarded as ONE union so
        // a program hitting several paths emits `pub mod tea;` exactly once (E0428).
        // This is the transitive-closure invariant: any module a declared module
        // depends on MUST itself be declared (same rule as `http_header`).
        if ctx.uses_tea
            || ctx.uses_server
            || ctx.uses_websocket
            || ctx.uses_live
            || ctx.uses_tui
            || ctx.uses_webview
        {
            mod_rs.push_str(RUNTIME_MOD_RS_TEA_APPEND);
        }
        if ctx.uses_server {
            mod_rs.push_str(RUNTIME_MOD_RS_SERVER_APPEND);
        }
        // Ipe.WebSocket client — declare `ws_client` (its `ssrf` dep is
        // already in the base, its `tea` dep forced above).
        if ctx.uses_websocket {
            mod_rs.push_str(RUNTIME_MOD_RS_WEBSOCKET_APPEND);
        }
        // Ipe.Auth — append auth module when any Auth kernel is used.
        if ctx.uses_auth {
            mod_rs.push_str(RUNTIME_MOD_RS_AUTH_APPEND);
        }
        // Ipe.Email — append email module when `Email.send` is used.
        if ctx.uses_email {
            mod_rs.push_str(RUNTIME_MOD_RS_EMAIL_APPEND);
        }
        // Ipe.Env — append env_public module when `Env.public` is used. Not
        // vendored from the source tree (its content is project-specific);
        // the file itself is inserted separately below, alongside the module
        // declaration here.
        if ctx.uses_env_public {
            mod_rs.push_str(RUNTIME_MOD_RS_ENV_PUBLIC_APPEND);
        }
        // `http_header` is part of the base `mod.rs` (the base `http_client`
        // module depends on it), so it needs no conditional append here — see
        // the note at the top of this file.
        // Ipe.Css leaf security kernels — declared for any render-capable
        // program (`uses_ui`, whose html/ui/live runtime modules import
        // `css_safety`) OR a pure-`Ipe.Css` program (`uses_css`, no render
        // kernel). Pushed BEFORE the UI append because `html.rs` /
        // `ui/render.rs` / `live/style_inject.rs` import `css_safety` at the
        // top level — it must be declared first. The single guard de-duplicates:
        // a program using both emits `pub mod css_safety;` exactly once (E0428).
        // Transitive closure: the `tui` runtime module unconditionally imports
        // `super::ui` (tui/app.rs, tui/layout.rs) and `super::html`
        // (tui/focus.rs), so a String-view Tui program (`uses_tui` without
        // `uses_ui`) still needs the css/ui/html appends — same invariant as
        // the `http_header` leaf above.
        // `live/mod.rs` unconditionally does
        // `pub use crate::ipe_runtime::html::*` and `html.rs` imports
        // `css_safety`, so a `uses_live`-only program (e.g. PubSub-only, no
        // Ipe.Ui kernels) still needs `css_safety` and `html` declared.
        if ctx.uses_ui || ctx.uses_css || ctx.uses_tui || ctx.uses_live || ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_CSS_APPEND);
        }
        // Ipe.Ui / Ipe.Html render kernels (+ Tui + Live transitive dep).
        // `live/mod.rs` unconditionally re-exports `crate::ipe_runtime::html::*`;
        // `live/style_inject.rs` imports `super::html` — so `html` must be
        // declared whenever live is enabled, even without explicit Ipe.Ui use.
        if ctx.uses_ui || ctx.uses_tui || ctx.uses_live || ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_UI_APPEND);
        }
        // Ipe.Live / Ipe.Live app-entry kernels.
        if ctx.uses_live || ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_LIVE_APPEND);
        }
        // Ipe.Tui / Ipe.Tui app-entry kernels.
        if ctx.uses_tui {
            mod_rs.push_str(RUNTIME_MOD_RS_TUI_APPEND);
        }
        // Ipe.Webview / Ipe.Webview app-entry kernel.
        if ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_WEBVIEW_APPEND);
        }
        mod_rs
    };

    let mut files = BTreeMap::new();
    // The emitted Rust source files: `src/main.rs` always, plus one
    // `src/ipe_mods/<ident>.rs` per module in the real-split case. In the
    // single-file collapse case `rust_sources` holds exactly the one
    // byte-identical `src/main.rs`.
    for (path, text) in rust_sources {
        files.insert(path, text);
    }
    files.insert(RelPath::new("src/ipe_runtime/mod.rs")?, runtime_mod_rs);
    files.insert(
        RelPath::new("src/ipe_runtime/config.rs")?,
        runtime_config_rs,
    );
    if ctx.uses_env_public {
        files.insert(
            RelPath::new("src/ipe_runtime/env_public.rs")?,
            render_env_public_rs(&ctx.wasm_public_env),
        );
    }
    // Foreign-crate FFI: write the wrapper module and declare it from the
    // crate root. File-count-agnostic (shared by the single-file and split
    // assembly paths), so the two stay byte-identical.
    if ctx.uses_ffi {
        let ffi = ctx.ffi.as_ref().ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::assemble_project_files",
            detail: "program lowers foreign-wrapper calls but the driver supplied no FFI \
                     emission inputs (RustBackend::with_ffi)"
                .to_owned(),
        })?;
        // S4 sentinel DCE (design D7): keep only the wrapper regions the
        // program REACHES. Reachability is read straight off the emitted
        // source — every `Callee::Ffi` renders as `crate::ffi::<ident>`, so a
        // scan of the already-emitted files is exhaustive by construction.
        // This is what lets a program bind a 76k-symbol crate yet compile only
        // the handful of wrappers it calls — and keeps a generator gap in some
        // UNUSED wrapper (an exotic lifetime/borrow shape the emitter renders
        // wrong) from breaking a build that never calls it.
        //
        // The interface FORWARDER modules must shake FIRST: every forwarder
        // references its wrapper, so an unshaken forwarder barrel would mark
        // every wrapper reached and defeat the slice below.
        shake_interface_forwarder_files(&mut files, &ffi.interface_modules);
        let reached = reached_ffi_idents(&files);
        let shaken = shake_ffi_by_fn_ident(&ffi.bindings_source, &reached);
        files.insert(RelPath::new("src/ffi.rs")?, shaken);
        let main = files
            .get_mut("src/main.rs")
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::assemble_project_files",
                detail: "no src/main.rs in the assembled file set".to_owned(),
            })?;
        main.push_str("\nmod ffi;\n");
    }
    format_generated_rust_files(&mut files)?;
    Ok(EmittedProject { files, cargo_toml })
}

/// Return `true` when `ty` (or any type it structurally contains) is a
/// server-surface or non-serde opaque type that must not appear as a field in
/// a `HydrationState` record.
///
/// The gate is an **allowlist**: only data-only, serialisable `IrType`s pass.
/// Function types, runtime handles, secret/SQL-fragment/crypto opaques, UI
/// element types, and TEA-runtime opaques all fail.
fn ir_type_contains_non_serde(ty: &IrType) -> bool {
    match ty {
        // ── Primitive data types — serialisable, no recursion needed ─────
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        | IrType::Order
        | IrType::Decimal
        | IrType::Error
        | IrType::ErrorKind
        | IrType::ErrorDetails
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        | IrType::Generic(_) => false,

        // ── Serialisable container types — recurse into inner types ───────
        IrType::Maybe(inner) | IrType::List(inner) | IrType::Set(inner) => {
            ir_type_contains_non_serde(inner)
        }
        IrType::Result(a, b) => ir_type_contains_non_serde(a) || ir_type_contains_non_serde(b),
        IrType::Dict(k, v) => ir_type_contains_non_serde(k) || ir_type_contains_non_serde(v),
        IrType::Tuple(elems) => elems.iter().any(ir_type_contains_non_serde),
        IrType::Record(fields) => fields.values().any(ir_type_contains_non_serde),
        IrType::Enum { args, .. } => args.iter().any(ir_type_contains_non_serde),

        // ── Never serialisable ────────────────────────────────────────────
        // Function types, UI element types, and the non-serde server-surface
        // / runtime-opaque types: handles to server resources, async
        // primitives, or types explicitly documented as non-serde
        // (Secret, SqlFragment).
        IrType::Fun(..)
        | IrType::SharedFun(..)
        | IrType::FnOnceChain(..)
        | IrType::Ui { .. }
        | IrType::UiPlain(_)
        | IrType::Task(_)
        | IrType::Cmd(_)
        | IrType::Sub(_)
        | IrType::Decoder(_)
        | IrType::Db
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        | IrType::StreamWriter
        | IrType::HttpRequest
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        | IrType::LiveReq
        | IrType::LiveRoute(_)
        | IrType::Secret
        | IrType::SqlFragment
        | IrType::CacheCfg
        | IrType::CacheStats
        | IrType::WebSocketClientCfg
        | IrType::CsvDoc
        | IrType::EmailMessage
        | IrType::EmailAttachment
        | IrType::EmailSesConfig
        | IrType::EmailSmtpConfig
        | IrType::EmailProvider => true,
    }
}

/// Gate: when `ctx.wasm_hydrate_mode`, find the type named `HydrationState`
/// in module `Main` and verify that every field is serialisation-safe.
///
/// A `HydrationState` with a non-serde field type (e.g. `Secret`, `Db`,
/// `Task`, a function type) is a compile error — the emitted `hydrate` export
/// serialises this type as JSON, so any such field would silently leak a
/// server-side secret or produce a `cargo` type error.
///
/// The gate fires at compile time (during backend emission), giving the user
/// a clear diagnostic rather than a mysterious `serde` bound failure from
/// `rustc`.
fn check_hydration_state_fields(ctx: &EmitCtx, program: &Program) -> DResult<()> {
    if !ctx.wasm_hydrate_mode {
        return Ok(());
    }

    // Find the module named `Main` in the program.
    let main_sym = ctx.interner.lookup("Main");
    let Some(main_sym) = main_sym else {
        // No `Main` symbol at all — the program is not a Live app; the
        // `hydrate` emit path will fail elsewhere with a clearer error.
        return Ok(());
    };

    let main_module = program.modules.iter().find(|m| m.name.0 == [main_sym]);
    let Some(main_module) = main_module else {
        return Ok(());
    };

    // Find the `HydrationState` type def in `Main`.
    let hs_sym = ctx.interner.lookup("HydrationState");
    let Some(hs_sym) = hs_sym else {
        // No `HydrationState` type — also valid (the hydrate path will use
        // the convention-based name; if it is absent the emitted code will
        // fail with a rust compile error, not a silent miscompile).
        return Ok(());
    };

    let hs_type = main_module.types.iter().find(|td| {
        let ipe_ir::TypeDef::Enum(def) = td;
        def.name == hs_sym
    });
    let Some(ipe_ir::TypeDef::Enum(hs_def)) = hs_type else {
        return Ok(());
    };

    // Walk every field of every variant.  `HydrationState` is expected to be
    // a record alias (single unit variant with named fields), but we check all
    // variants for completeness.
    for variant in &hs_def.variants {
        for field_ty in &variant.fields {
            if ir_type_contains_non_serde(field_ty) {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::project::check_hydration_state_fields",
                    detail: format!(
                        "`HydrationState` has a non-serialisable field type \
                         `{field_ty:?}`. \
                         `HydrationState` is serialised as JSON in the WASM \
                         hydration island; server-surface types (Db, Secret, \
                         Task, function types, etc.) must not appear as fields. \
                         Declare a separate client-safe type that contains only \
                         the data the client needs."
                    ),
                });
            }
        }
    }

    Ok(())
}

/// Render the `Spine` tier's text for `program` — everything that is
/// program-wide rather than Ipê-module-owned (design doc §2.1/§2.3):
/// the preamble banner, the `Spine` bucket's `EnumDef`s (the synthetic
/// `SqlValue`/`SqlField` Db built-ins — §2.2), the synthesised record
/// structs, the DB boundary-projection impls, the fixed kernel-wrapper
/// prelude, the TEA/Auth alias blocks, the epilogue, and `fn main()`.
///
/// Deliberately does NOT emit either module's own `Func`/`EnumDef` — those
/// belong to their [`emit_module_file`] output. The `Spine`-bucket enums are
/// rendered immediately before the record structs, reproducing the ordering
/// rule [`emit_program`] already established (user types then `SqlValue` then
/// `SqlField` then record structs then the DB-projection impls).
///
/// **This function is NOT on the public emission path** — [`emit_program`]
/// (still single-file) does not call it. It is an additive rendering entry
/// point kept separate so `emit_program` stays byte-for-byte unchanged while
/// this output tier is proven in isolation (`tests/split_emit.rs`).
///
/// # Errors
///
/// Propagates any [`Diagnostic`] from the reused `preamble`/`emit_enum`/
/// `emit_record_struct`/`emit_db_projection_impls`/`runtime_bindings`/
/// `epilogue` rendering, and the G3 Webview anchor assertion.
pub fn emit_spine(ctx: &EmitCtx, program: &Program) -> DResult<String> {
    check_hydration_state_fields(ctx, program)?;

    let mut out = String::with_capacity(GOLDEN.len() + 4096);
    out.push_str(&preamble()?);
    // See the single-file emit path: the banner's closing blank already
    // separates an EMPTY user-types section from the runtime bindings, so the
    // second blank is pushed only when this section emitted content.
    let after_banner = out.len();

    let Partitioned { buckets, .. } = partition_items(program, ctx.interner);

    // The Spine bucket's `SqlValue`/`SqlField` enums, in insertion order —
    // rendered where the user types would sit in the single-file layout, i.e.
    // immediately before the record structs (§2.2's ordering rule). No
    // `IpeModule` bucket enums are emitted here — those are `emit_module_file`.
    if let Some((spine_enums, _)) = buckets.get(&RustFileId::Spine) {
        for &def in spine_enums {
            out.push_str(&emit_enum(ctx, def)?);
        }
    }
    for rec in ctx.record_structs() {
        out.push_str(&emit_record_struct(ctx, rec)?);
    }
    if ctx.uses_db {
        out.push_str(&emit_db_projection_impls(ctx)?);
    }

    if out.len() != after_banner {
        out.push('\n');
    }

    match ctx.target {
        ipe_ir::Target::Native => out.push_str(runtime_bindings()?),
        ipe_ir::Target::WasmClient => out.push_str(&wasm_runtime_bindings()?),
    }
    if ctx.uses_tea {
        out.push_str(TEA_TYPE_ALIASES);
    }
    if ctx.uses_auth {
        out.push_str(AUTH_WRAPPERS);
    }
    // The spine carries NO user functions — they are `emit_module_file`'s — so a
    // single blank line separates the runtime bindings from the epilogue.
    // (rustfmt collapses blank-line runs to one; the two blanks the single-file
    // layout emits around its function block would collapse here anyway, and
    // emitting them would fail `cargo fmt --check` on the raw output.)
    out.push('\n');

    match ctx.target {
        ipe_ir::Target::Native => out.push_str(&epilogue()?),
        ipe_ir::Target::WasmClient => out.push_str(&epilogue_wasm(ctx)?),
    }

    // ── G3: Webview main-thread entry switch ──────────────────────────────
    // The `block_on(ipe_main())` anchor lives in the epilogue, which is
    // Spine-only — so under the split this scan sees a strictly smaller
    // haystack than the whole concatenated `main.rs` (design doc §2.3).
    if ctx.uses_webview {
        const BLOCK_ON_ANCHOR: &str = "block_on(ipe_main())";
        const BLOCK_ON_THREAD_REPLACEMENT: &str = "block_on_current_thread(ipe_main())";
        let replaced = out.replacen(BLOCK_ON_ANCHOR, BLOCK_ON_THREAD_REPLACEMENT, 1);
        if replaced == out {
            return Err(Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::emit_spine::G3_block_on",
                detail: format!(
                    "G3 webview entry-switch: anchor {BLOCK_ON_ANCHOR:?} not found in \
                     emitted spine — epilogue golden has drifted; Ipe.Webview REQUIRES \
                     block_on_current_thread"
                ),
            });
        }
        out = replaced;
    }

    Ok(out)
}

/// Render the `IpeModule(home)` file's text for one Ipê module's OWN
/// declarations (design doc §2.1): ONLY that `home`'s `EnumDef`s + `Func`s,
/// each `pub(crate)`-visible (not the bare `pub` the single-file layout uses,
/// since these now live inside a `mod` block), opening with the flat-barrel
/// `use crate::*;` glob so every `Spine`/other-module item is in scope.
///
/// A `home` with no items in `program` (never the real driver path — every
/// `IpeModule` file materialises FROM a non-empty bucket) yields just the
/// `use crate::*;` header.
///
/// **This function is NOT on the public emission path** — see [`emit_spine`].
///
/// # Errors
///
/// Propagates any [`Diagnostic`] from the reused `emit_enum`/`emit_func`
/// rendering.
pub fn emit_module_file(ctx: &EmitCtx, program: &Program, home: &RustFileId) -> DResult<String> {
    let Partitioned { buckets, .. } = partition_items(program, ctx.interner);

    let mut out = String::new();
    // Every module file opens with the flat glob barrel (§2.1): because
    // `main.rs` re-exports every module's items at the crate root and every
    // name is already globally unique, `use crate::*;` gives this file every
    // Spine item and every other module's item with zero per-symbol
    // bookkeeping.
    out.push_str("use crate::*;\n\n");

    if let Some((enums, funcs)) = buckets.get(home) {
        for &def in enums {
            out.push_str(&pub_crate_item(&emit_enum(ctx, def)?));
        }
        for &func in funcs {
            out.push_str(&pub_crate_item(&emit_func(ctx, func)?));
        }
    }

    Ok(out)
}

/// Assemble the full split [`EmittedProject`] from ALREADY-RENDERED per-file
/// texts (design doc §4.4 — the `emit_manifest` assembly seam).
///
/// `spine_text` is [`emit_spine`]'s output; `module_texts` maps each Ipê-module
/// `home` to its [`emit_module_file`] output. This function performs ONLY the
/// file-count-dependent assembly the single-file [`emit_program`] path also
/// does in its `>= 2` branch — computing the deterministic first-encounter
/// module order, the fail-closed `mod_ident` uniqueness gate, the record-struct
/// disjointness gate, the `main.rs` barrel lines, and the `src/ipe_mods/*.rs`
/// file list — then delegates the file-count-AGNOSTIC manifest/runtime block to
/// the shared [`assemble_project_files`]. It never re-renders any user item;
/// the texts are taken verbatim, so the salsa `emit_manifest` query's output is
/// byte-identical to `emit_program`'s split output for the same program.
///
/// PRECONDITION (`>= 2` distinct `IpeModule` homes): this is the real-split
/// path. The single-home / zero-home collapse case never reaches here —
/// `emit_manifest` routes it straight to `emit_program` for the byte-identical
/// single-`main.rs` output (§4.4).
///
/// # Errors
///
/// Propagates [`Diagnostic`]s from `mod_ident` resolution, the fail-closed
/// duplicate-`mod`/record-struct-collision gates, [`RelPath`] validation, and
/// the shared manifest/runtime assembly.
pub fn assemble_split_manifest(
    ctx: &EmitCtx,
    program: &Program,
    spine_text: &str,
    module_texts: &BTreeMap<ModPath, String>,
) -> DResult<EmittedProject> {
    let partition = partition_items(program, ctx.interner);

    // The distinct `IpeModule` homes in first-encounter (linker/topological)
    // order — the SAME union `emit_program`'s split branch computes, driving
    // both the barrel lines and the per-module file list.
    let mut module_homes: Vec<RustFileId> = Vec::new();
    let mut seen: BTreeSet<RustFileId> = BTreeSet::new();
    for id in partition
        .type_order
        .iter()
        .chain(partition.func_order.iter())
    {
        if seen.insert(id.clone()) {
            module_homes.push(id.clone());
        }
    }

    // Fail closed if two distinct homes fold to the same `mod_ident` before any
    // file is written (same fail-closed gate as `emit_program`'s split branch).
    rust_file::assert_mod_idents_unique(&module_homes, ctx.interner)?;

    // Fail closed if a synthesised record struct's name collides with a
    // `mod_ident` (every IpeModule home contributes its ident in the split).
    let mod_idents: BTreeSet<String> = module_homes
        .iter()
        .filter_map(|id| match id {
            RustFileId::IpeModule(home) => Some(rust_file::resolve_mod_ident(home, ctx.interner)),
            RustFileId::Spine => None,
        })
        .collect::<DResult<BTreeSet<String>>>()?;
    ctx.assert_record_structs_disjoint_from_type_namespace(&mod_idents)?;

    let mut rust_sources: Vec<(RelPath, String)> = Vec::new();

    // `main.rs` = the given spine text + the flat glob barrel, one pair per
    // distinct IpeModule home in first-encounter order (byte-identical to
    // `emit_program`'s split branch).
    let mut main_rs = spine_text.to_owned();
    main_rs.push('\n');
    for id in &module_homes {
        let RustFileId::IpeModule(home) = id else {
            continue;
        };
        let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
        main_rs.push_str("#[path = \"ipe_mods/");
        main_rs.push_str(&ident);
        main_rs.push_str(".rs\"]\nmod ");
        main_rs.push_str(&ident);
        main_rs.push_str(";\npub(crate) use ");
        main_rs.push_str(&ident);
        main_rs.push_str("::*;\n");
    }
    rust_sources.push((RelPath::new("src/main.rs")?, main_rs));

    // One `src/ipe_mods/<ident>.rs` per module, its text taken verbatim from
    // the demanded per-file query output.
    for id in &module_homes {
        let RustFileId::IpeModule(home) = id else {
            continue;
        };
        let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
        let text = module_texts
            .get(home)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::assemble_split_manifest",
                detail: format!(
                    "no rendered text supplied for IpeModule home ident {ident:?} — \
                 emit_manifest must demand emit_rust_file for every home in \
                 program_rust_file_ids"
                ),
            })?;
        rust_sources.push((
            RelPath::new(format!("src/ipe_mods/{ident}.rs"))?,
            text.clone(),
        ));
    }

    assemble_project_files(ctx, rust_sources)
}

/// Narrow a rendered top-level item's leading `pub ` visibility to
/// `pub(crate) `, for emission inside a `mod` block (design doc §2.1).
///
/// `emit_enum`/`emit_func` render user items with a bare `pub ` prefix (a
/// top-level `main.rs` declaration). Inside a per-module `mod` file the crate
/// root re-exports them via a glob barrel, so `pub(crate)` is both sufficient
/// and correct. Operates on the FIRST `pub enum `/`pub fn ` occurrence only —
/// an enum's trailing `impl … IpeStringify` block carries no `pub`, and a
/// rendered item's declaration keyword is always at its head (or immediately
/// after a leading `#[derive(...)]` line for an enum) — so this narrows
/// exactly the one declaration keyword, never a substring inside a body.
fn pub_crate_item(rendered: &str) -> String {
    if let Some(rest) = rendered.strip_prefix("pub enum ") {
        return format!("pub(crate) enum {rest}");
    }
    if let Some(rest) = rendered.strip_prefix("pub fn ") {
        return format!("pub(crate) fn {rest}");
    }
    // An enum whose derivability gate emitted a `#[derive(...)]` line before
    // `pub enum` — narrow the first `\npub enum ` after that attribute.
    if let Some(pos) = rendered.find("\npub enum ") {
        let mut result = String::with_capacity(rendered.len() + 8);
        result.push_str(rendered.get(..pos + 1).unwrap_or(""));
        result.push_str("pub(crate) enum ");
        result.push_str(rendered.get(pos + 1 + "pub enum ".len()..).unwrap_or(""));
        return result;
    }
    rendered.to_owned()
}

/// Build the db-enabled `Cargo.toml` from the base manifest by:
///
/// 1. Adding `"db"` to the `default` feature list.
/// 2. Appending the `sqlx` dependency line, with `"sqlite"` ALWAYS enabled
///    plus `"postgres"` ADDITIONALLY when `driver` is Postgres — the
///    structural fix that makes a Postgres-driver program's sqlx dependency
///    actually enable Postgres support (a `driver = "postgres"` build with
///    only the `"sqlite"` sqlx feature fails to compile
///    `sqlx::postgres::PgPool`, which is exactly the "Postgres driver
///    structurally unreachable" gap this closes).
///
///    `"sqlite"` can never be dropped regardless of `driver`: the always-
///    emitted `telemetry_spill`/`live::hub`/`live::store` runtime modules
///    hardcode `SqlitePool` for their local spill/session persistence
///    (independent of the app's `[database]` driver choice), so an
///    exclusive sqlite-vs-postgres feature selection made every Postgres
///    build fail `cargo build` downstream of `db_cargo_toml` even though
///    `ipe` itself exited 0 — an exit-0-then-cargo-fail SEAL violation.
///    Additive, not exclusive, is the only sound selection here.
///
/// String surgery rather than a second static file: the manifest content is
/// small and the two edits are unambiguous anchors.
fn db_cargo_toml(driver: crate::DbDriver) -> DResult<String> {
    const DEFAULT_LINE: &str = r#"default = ["tokio", "crypto", "json"]"#;
    const DEFAULT_LINE_DB: &str = r#"default = ["tokio", "crypto", "json", "db"]"#;
    // The sqlx line is appended right before the dev/release profile sections.
    // Anchoring on `[profile.dev]` is stable (always present in the template).
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    // Version + crate name sourced from the SSOT (`crate_specs`); the feature
    // list stays inline (it depends on usage AND driver). `"sqlite"` is
    // unconditional — see the doc comment above for why.
    let sqlx_features = match driver {
        crate::DbDriver::Sqlite => r#""sqlite""#.to_owned(),
        crate::DbDriver::Postgres => r#""sqlite", "postgres""#.to_owned(),
    };
    // `bincode` rides with `sqlx`: the vendored live session store's
    // checkpoint body is bincode-encoded under the same `db` feature gate.
    let sqlx_line = format!(
        "{} = {{ version = \"{}\", features = [\"runtime-tokio-rustls\", {sqlx_features}] }}\n{} = \"{}\"\n\n",
        crate_specs::SQLX.name,
        crate_specs::SQLX.version,
        crate_specs::BINCODE.name,
        crate_specs::BINCODE.version,
    );

    let step1 = CARGO_TOML.replacen(DEFAULT_LINE, DEFAULT_LINE_DB, 1);
    if step1 == CARGO_TOML {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::db_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_LINE:?} not found — golden drifted"),
        });
    }
    let anchor_pos = step1
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::db_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step1.len() + sqlx_line.len());
    result.push_str(step1.get(..anchor_pos).unwrap_or(""));
    result.push_str(&sqlx_line);
    result.push_str(step1.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Build the server-enabled `Cargo.toml` from the given base manifest by:
///
/// 1. Adding `"server"` to the `default` feature list.
/// 2. Extending the `tokio` dependency line with the `"net"` and `"sync"`
///    features (required by `server.rs`'s `TcpListener` and `mpsc` usage).
/// 3. Appending `axum` and `tower-http` dependency lines before `[profile.dev]`.
///
/// Takes the current manifest string so it can be composed with
/// [`db_cargo_toml`] when a program uses both Db and Server kernels.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] if any anchor string is absent —
/// a golden-drift invariant violation.
fn server_cargo_toml(base: &str) -> DResult<String> {
    // All anchor consts up front — items_after_statements lint requires items
    // to precede any `let` statement in the same scope.
    const DEFAULT_PREFIX: &str = "default = [";
    const DB_FEATURE: &str = "db = []";
    const DB_SERVER_FEATURE: &str = "db = []\nserver = []";
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    // tokio version + name from the SSOT; the two feature-list forms (the
    // replacen anchor and its net+sync successor) share the one version so the
    // anchor and the golden base manifest cannot skew independently.
    let tokio_time = format!(
        "{} = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\"] }}",
        crate_specs::TOKIO.name,
        crate_specs::TOKIO.version,
    );
    let tokio_net_sync = format!(
        "{} = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\", \"net\", \"sync\"] }}",
        crate_specs::TOKIO.name,
        crate_specs::TOKIO.version,
    );
    let server_deps = format!(
        "{} = {{ version = \"{}\", features = [\"ws\"] }}\n\
         {} = {{ version = \"{}\", features = [\"fs\", \"catch-panic\"] }}\n\n",
        crate_specs::AXUM.name,
        crate_specs::AXUM.version,
        crate_specs::TOWER_HTTP.name,
        crate_specs::TOWER_HTTP.version,
    );

    // Step 1a — insert `"server"` as the LAST element of the `default = [...]`
    // feature list, immediately before its closing `]`.
    //
    // This generic anchor handles every composition:
    //   non-db:  `default = ["tokio", "crypto", "json"]`
    //            → `default = ["tokio", "crypto", "json", "server"]`
    //   db:      `default = ["tokio", "crypto", "json", "db"]`
    //            → `default = ["tokio", "crypto", "json", "db", "server"]`
    //   any future feature:  likewise, without needing a new anchor string.
    //
    // Fail-closed: if the prefix or the closing `]` is absent the manifest
    // has drifted from the golden and we surface a CompilerBug rather than
    // silently emitting an invalid manifest.
    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::server_cargo_toml",
            detail: "default feature list has no closing ']' — golden drifted".to_owned(),
        })?;
    let close = search_from + rel;
    let mut step1a = String::with_capacity(base.len() + 12);
    step1a.push_str(base.get(..close).unwrap_or(""));
    step1a.push_str(r#", "server""#);
    step1a.push_str(base.get(close..).unwrap_or(""));

    // Step 1b — define the `server = []` feature flag so that "server" in
    // `default = [...]` refers to a declared feature rather than an undeclared
    // name (Cargo rejects an undeclared feature reference with E0015).
    // Anchor on the `db = []` line which is always present in the base manifest.
    let step1 = step1a.replacen(DB_FEATURE, DB_SERVER_FEATURE, 1);
    if step1 == step1a {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {DB_FEATURE:?} not found — golden drifted"),
        });
    }

    // Step 2 — extend the tokio dependency line with "net" and "sync".
    let step2 = step1.replacen(&tokio_time, &tokio_net_sync, 1);
    if step2 == step1 {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {tokio_time:?} not found — golden drifted"),
        });
    }

    // Step 3 — append axum + tower-http before `[profile.dev]`.
    let anchor_pos = step2
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step2.len() + server_deps.len());
    result.push_str(step2.get(..anchor_pos).unwrap_or(""));
    result.push_str(&server_deps);
    result.push_str(step2.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Build the live-enabled `Cargo.toml` from the given base manifest by:
///
/// 1. Adding `"live"` to the `default` feature list.
/// 2. Inserting `async-trait` and `serde_urlencoded` as explicit dependencies
///    before the `[profile.dev]` section.
///
/// These two crates are required because the runtime's `live` feature enables
/// code that imports them (`async-trait` in `live/store.rs` and
/// `serde_urlencoded` in `live/form.rs`).  The emitted project vendors the
/// runtime source directly, so these must appear as explicit `[dependencies]`
/// in the emitted manifest.
///
/// The base manifest already declares `live = []` as a non-default feature
/// (present in the golden's `[features]` section).  This function promotes
/// it by inserting `"live"` immediately before the closing `]` of the
/// `default = [...]` line — the same generic anchor used by `server_cargo_toml`.
///
/// Called AFTER `server_cargo_toml` when both flags are set, so it is composed
/// on top of a manifest that may already contain `"server"` in `default`.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] if any anchor is absent — a
/// golden-drift invariant violation.
fn live_cargo_toml(base: &str) -> DResult<String> {
    // All consts must precede the first `let` — `items_after_statements` (pedantic).
    const DEFAULT_PREFIX: &str = "default = [";
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    // `async-trait` and `serde_urlencoded` are pulled by the runtime's `live`
    // feature gate; the emitted project vendors the runtime source directly, so
    // they must appear as explicit `[dependencies]` entries.
    // `libc` is pulled by the runtime's `live` console-proxy (`libc::prctl`).
    // The `live` runtime mainline uses `tokio::signal` + `tokio::process`; the base
    // golden emits `net`+`sync` for the HTTP server, so add the two missing features.
    const TOKIO_NET_SYNC_FEATURES: &str = "\"time\", \"net\", \"sync\"]";
    const TOKIO_LIVE_FEATURES: &str = "\"time\", \"net\", \"sync\", \"signal\", \"process\"]";
    // Transitive-closure invariant: the runtime's `live/store.rs` defines
    // `PostgresStore` gated on `#[cfg(feature = "db")]`, which uses `sqlx::PgPool`.
    // `sqlx::PgPool` requires the `postgres` sqlx feature.  When a program uses
    // BOTH Db and Live, `db_cargo_toml` has already injected the sqlx dep with
    // `["runtime-tokio-rustls", "sqlite"]`; this step extends it with `"postgres"`
    // so that the `PostgresStore` code compiles.
    //
    // `SQLX_SQLITE_FEATURES` uniquely identifies the sqlx dep written by
    // `db_cargo_toml` — `runtime-tokio-rustls` is a sqlx-specific feature, so
    // this pattern never collides with another dep's feature list.
    //
    // Fail-open (no CompilerBug guard): when the program uses Live but NOT Db, the
    // sqlx dep is absent from the manifest and the `replacen` is a no-op, which is
    // correct — a Live-only program does not need the `postgres` feature.
    const SQLX_SQLITE_FEATURES: &str = "features = [\"runtime-tokio-rustls\", \"sqlite\"]";
    const SQLX_POSTGRES_FEATURES: &str =
        "features = [\"runtime-tokio-rustls\", \"sqlite\", \"postgres\"]";
    // Versions + names from the SSOT; these three are bare `name = "ver"` deps.
    // `serde_urlencoded` is NOT appended here: it is an unconditional base
    // manifest dep now (`dom/form.rs` is always vendored).
    let live_deps = format!(
        "{} = \"{}\"\n{} = \"{}\"\n\n",
        crate_specs::ASYNC_TRAIT.name,
        crate_specs::ASYNC_TRAIT.version,
        crate_specs::LIBC.name,
        crate_specs::LIBC.version,
    );

    // Step 1 — promote the `live` feature.
    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::live_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::live_cargo_toml",
            detail: "default feature list has no closing ']' — golden drifted".to_owned(),
        })?;
    let close = search_from + rel;
    let mut step1 = String::with_capacity(base.len() + 64);
    step1.push_str(base.get(..close).unwrap_or(""));
    step1.push_str(r#", "live""#);
    step1.push_str(base.get(close..).unwrap_or(""));

    // Step 1b — add the tokio `signal` + `process` features the live runtime needs.
    // Fail loud (like the sibling anchors) if the anchor drifted — a silent no-op
    // here would ship a manifest without `signal`/`process` and cargo-fail on the
    // live runtime's `tokio::signal` usage (a fresh exit-0-then-cargo-fail).
    let step1_tokio = step1.replace(TOKIO_NET_SYNC_FEATURES, TOKIO_LIVE_FEATURES);
    if step1_tokio == step1 {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::live_cargo_toml",
            detail: format!(
                "tokio features anchor {TOKIO_NET_SYNC_FEATURES:?} not found — golden drifted; \
                 the live runtime requires the tokio signal + process features"
            ),
        });
    }
    let step1 = step1_tokio;

    // Step 2 — inject live-specific deps before `[profile.dev]`.
    let anchor_pos = step1
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::live_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step1.len() + live_deps.len());
    result.push_str(step1.get(..anchor_pos).unwrap_or(""));
    result.push_str(&live_deps);
    result.push_str(step1.get(anchor_pos..).unwrap_or(""));

    // Step 3 — extend the sqlx dep with the `postgres` feature when the
    // program also uses Db (db_cargo_toml ran before live_cargo_toml and
    // injected the sqlite-only sqlx dep; we promote it here so the live
    // session-store's `PostgresStore` — which references `sqlx::PgPool` — can
    // compile).  No-op when sqlx is absent (Live-only, no Db).
    let result = result.replacen(SQLX_SQLITE_FEATURES, SQLX_POSTGRES_FEATURES, 1);
    Ok(result)
}

/// Build the tui-enabled `Cargo.toml` from the given base manifest by:
///
/// 1. Adding `"tui"` to the `default` feature list.
/// 2. Adding `"sync"` to the `tokio` dependency's feature list (`tui/app.rs`
///    uses `tokio::sync::mpsc::unbounded_channel`).
/// 3. Appending `crossterm` and `unicode-width` dependencies before
///    `[profile.dev]`.  These two crates are the compile-time gates on the
///    runtime's `tui` feature; the emitted project vendors the runtime source
///    directly, so they MUST appear as explicit `[dependencies]` entries.
///
/// Called AFTER any server/live manifest extension so it composes on top of an
/// already-modified manifest.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] if any anchor string is absent — a
/// golden-drift invariant violation (fail-loud, never a silent no-op that
/// ships a broken manifest).
fn tui_cargo_toml(base: &str) -> DResult<String> {
    // All anchor consts must precede the first `let` statement.
    const DEFAULT_PREFIX: &str = "default = [";
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    // Versions + names from the SSOT. `tui_deps` are bare `name = "ver"` lines;
    // the two tokio forms (the replacen anchor and its sync successor) share the
    // one SSOT version so the anchor cannot skew from the golden base manifest.
    //
    // The tui runtime uses `tokio::sync::mpsc`; add `"sync"` when it is not
    // yet present.  The base golden has only `"time"` in the tokio feature list;
    // server_cargo_toml adds `"net", "sync"`; live_cargo_toml extends to include
    // `"signal", "process"`.  We gate on the SMALLEST known form that lacks
    // `"sync"` and replace it with the tui-extended form.  If `"sync"` is
    // already present (because server_cargo_toml ran first) the replacen is a
    // no-op and we do NOT error — `"sync"` is idempotent to add.
    //
    // Three anchors: non-server base, server-only (no live), live (superset).
    // We check for the presence of `"sync"` and insert it only if absent.
    let tui_deps = format!(
        "{} = \"{}\"\n{} = \"{}\"\n\n",
        crate_specs::CROSSTERM.name,
        crate_specs::CROSSTERM.version,
        crate_specs::UNICODE_WIDTH.name,
        crate_specs::UNICODE_WIDTH.version,
    );
    let tokio_time_only = format!(
        "{} = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\"] }}",
        crate_specs::TOKIO.name,
        crate_specs::TOKIO.version,
    );
    let tokio_time_sync = format!(
        "{} = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\", \"sync\"] }}",
        crate_specs::TOKIO.name,
        crate_specs::TOKIO.version,
    );

    // Step 1 — promote the `tui` feature (generic closing-`]` anchor, same
    // strategy as server_cargo_toml / live_cargo_toml).
    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::tui_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::tui_cargo_toml",
            detail: "default feature list has no closing ']' — golden drifted".to_owned(),
        })?;
    let close = search_from + rel;
    let mut step1 = String::with_capacity(base.len() + 64);
    step1.push_str(base.get(..close).unwrap_or(""));
    step1.push_str(r#", "tui""#);
    step1.push_str(base.get(close..).unwrap_or(""));

    // Step 2 — add `"sync"` to tokio if not already present.
    // If the manifest already has `"sync"` (from server_cargo_toml or
    // live_cargo_toml) the `contains` check short-circuits and no change is
    // made.  Only when the base tokio line lacks `"sync"` do we replace the
    // known-anchor form (non-server base) with the sync-extended form.
    let step2 = if step1.contains(r#""sync""#) {
        // `"sync"` already present — idempotent, no change needed.
        step1
    } else {
        // The only tokio line that can lack `"sync"` on a valid manifest is
        // the non-server, non-live base form.  Fail-loud if the anchor drifted.
        let replaced = step1.replacen(&tokio_time_only, &tokio_time_sync, 1);
        if replaced == step1 {
            return Err(Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::tui_cargo_toml",
                detail: format!(
                    "tokio anchor {tokio_time_only:?} not found and no \"sync\" present — \
                     golden drifted; tui runtime requires tokio sync"
                ),
            });
        }
        replaced
    };

    // Step 3 — append crossterm + unicode-width before `[profile.dev]`.
    let anchor_pos = step2
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::tui_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step2.len() + tui_deps.len());
    result.push_str(step2.get(..anchor_pos).unwrap_or(""));
    result.push_str(&tui_deps);
    result.push_str(step2.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Build the webview-enabled `Cargo.toml` from the given base manifest by:
///
/// 1. Adding `"webview"` to the `default` feature list.
/// 2. Wiring the `webview = []` feature declaration to actually pull `wry` and
///    `tao` (changes it to `webview = ["dep:wry", "dep:tao"]`).
/// 3. Appending `wry` and `tao` as optional dependencies before `[profile.dev]`.
///
/// Called AFTER `server_cargo_toml` and `live_cargo_toml` so the live feature
/// (which the webview backend imports from) is already promoted.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] if any anchor string is absent — a
/// golden-drift invariant violation (fail-loud, never a silent no-op that
/// ships a broken manifest or links the wrong backend).
fn webview_cargo_toml(base: &str) -> DResult<String> {
    // All anchor consts must precede the first `let` statement.
    const DEFAULT_PREFIX: &str = "default = [";
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    // Change the empty `webview = []` feature to pull wry + tao when active.
    // The `dep:` prefix is Cargo's "explicit dep" syntax (Cargo ≥1.60), so
    // `webview = ["dep:wry", "dep:tao"]` activates the optional deps ONLY when
    // the `webview` feature is in the default list — the stub path gets no
    // heavy system deps.
    const WEBVIEW_EMPTY: &str = "webview = []";
    const WEBVIEW_WITH_DEPS: &str = r#"webview = ["dep:wry", "dep:tao"]"#;
    // wry + tao are declared optional so the stub path (no `webview` feature)
    // never downloads or links them. Versions + names from the SSOT; the
    // `optional = true` gate stays inline.
    let webview_native_deps = format!(
        "{} = {{ version = \"{}\", optional = true }}\n{} = {{ version = \"{}\", optional = true }}\n\n",
        crate_specs::WRY.name,
        crate_specs::WRY.version,
        crate_specs::TAO.name,
        crate_specs::TAO.version,
    );

    // Step 1 — promote `webview` to the default feature list (generic
    // closing-`]` anchor, same strategy as server/live/tui_cargo_toml).
    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::webview_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::webview_cargo_toml",
            detail: "default feature list has no closing ']' — golden drifted".to_owned(),
        })?;
    let close = search_from + rel;
    let mut step1 = String::with_capacity(base.len() + 64);
    step1.push_str(base.get(..close).unwrap_or(""));
    step1.push_str(r#", "webview""#);
    step1.push_str(base.get(close..).unwrap_or(""));

    // Step 2 — wire the `webview` feature to its deps (`dep:wry` + `dep:tao`).
    // Fail-loud if the empty anchor drifted — a silent no-op here would promote
    // `webview` to defaults without pulling wry/tao, shipping a build where the
    // real backend compiles as stub (exit-0-then-runtime-Err).
    let step2 = step1.replacen(WEBVIEW_EMPTY, WEBVIEW_WITH_DEPS, 1);
    if step2 == step1 {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::webview_cargo_toml",
            detail: format!(
                "Cargo.toml anchor {WEBVIEW_EMPTY:?} not found — golden drifted; \
                 the webview feature declaration must be present to wire wry + tao"
            ),
        });
    }

    // Step 3 — append wry + tao as optional deps before `[profile.dev]`.
    let anchor_pos = step2
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::webview_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step2.len() + webview_native_deps.len());
    result.push_str(step2.get(..anchor_pos).unwrap_or(""));
    result.push_str(&webview_native_deps);
    result.push_str(step2.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Build the websocket-client-enabled `Cargo.toml` from the given base manifest
/// by:
///
/// 1. Adding `"websocket_client"` to the `default` feature list — the empty
///    `websocket_client = []` feature is a pure `#[cfg]` gate over `ws_client.rs`
///    (its deps are plain, not `dep:`-activated, so promoting it activates the
///    module without any feature→dep wiring).
/// 2. Adding `"sync"` to tokio (the `ws_client` writer/reader tasks use
///    `tokio::sync::mpsc` / `broadcast`) when not already present — idempotent,
///    same strategy as `tui_cargo_toml`.
/// 3. Appending `tokio-tungstenite` as a plain dependency before `[profile.dev]`
///    (`futures-util` and `url`, its other deps, are already in the base).
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] if any anchor string is absent — a
/// golden-drift invariant violation (fail-loud, never a silent no-op that ships
/// a manifest where `ws_client` compiles without `tokio-tungstenite`).
fn websocket_cargo_toml(base: &str) -> DResult<String> {
    const DEFAULT_PREFIX: &str = "default = [";
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    let tokio_time_only = format!(
        "{} = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\"] }}",
        crate_specs::TOKIO.name,
        crate_specs::TOKIO.version,
    );
    let tokio_time_sync = format!(
        "{} = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\", \"sync\"] }}",
        crate_specs::TOKIO.name,
        crate_specs::TOKIO.version,
    );
    let ws_dep = format!(
        "{} = \"{}\"\n",
        crate_specs::TOKIO_TUNGSTENITE.name,
        crate_specs::TOKIO_TUNGSTENITE.version,
    );

    // Step 1 — promote `websocket_client` to the default feature list.
    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::websocket_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::websocket_cargo_toml",
            detail: "default feature list has no closing ']' — golden drifted".to_owned(),
        })?;
    let close = search_from + rel;
    let mut step1 = String::with_capacity(base.len() + 64);
    step1.push_str(base.get(..close).unwrap_or(""));
    step1.push_str(r#", "websocket_client""#);
    step1.push_str(base.get(close..).unwrap_or(""));

    // Step 2 — add `"sync"` to tokio if not already present (idempotent when a
    // prior server/live/tui surgery already added it).
    let step2 = if step1.contains(r#""sync""#) {
        step1
    } else {
        let replaced = step1.replacen(&tokio_time_only, &tokio_time_sync, 1);
        if replaced == step1 {
            return Err(Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::websocket_cargo_toml",
                detail: format!(
                    "tokio anchor {tokio_time_only:?} not found and no \"sync\" present — \
                     golden drifted; the ws_client runtime requires tokio sync"
                ),
            });
        }
        replaced
    };

    // Step 3 — append tokio-tungstenite before `[profile.dev]`.
    let anchor_pos = step2
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::websocket_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step2.len() + ws_dep.len());
    result.push_str(step2.get(..anchor_pos).unwrap_or(""));
    result.push_str(&ws_dep);
    result.push_str(step2.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Build the email-enabled `Cargo.toml` by appending the `lettre` dependency
/// before `[profile.dev]`.
///
/// `email.rs` (the vendored `Ipe.Email` runtime module) needs `lettre` for the
/// SMTP transport; every other crate it uses (`base64` / `hmac` / `sha2` /
/// `serde_json` / `reqwest` / `url`) is already an unconditional base-manifest
/// dependency. No feature promotion is required — the emitted crate declares the
/// `email` module unconditionally (via the `mod.rs` append), so the module is
/// always compiled once its one extra dep is present. `lettre`'s feature list +
/// `default-features = false` mirror `runtime/Cargo.toml` (the vendored source
/// was tested against exactly that shape). The version comes from the
/// [`crate_specs`] SSOT (drift-guarded against `runtime/Cargo.toml`).
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] if the `[profile.dev]` anchor is absent —
/// a golden-drift invariant violation (fail-loud, never a silent no-op).
fn email_cargo_toml(base: &str) -> DResult<String> {
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    let lettre_dep = format!(
        "{} = {{ version = \"{}\", default-features = false, features = [\"builder\", \
         \"hostname\", \"smtp-transport\", \"pool\", \"tokio1\", \"tokio1-rustls-tls\"] }}\n\n",
        crate_specs::LETTRE.name,
        crate_specs::LETTRE.version,
    );
    let anchor_pos = base
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::email_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(base.len() + lettre_dep.len());
    result.push_str(base.get(..anchor_pos).unwrap_or(""));
    result.push_str(&lettre_dep);
    result.push_str(base.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Slice each compiler-generated FFI interface-forwarder module down to the
/// forwarders the rest of the program references.
///
/// The targets are ONLY the modules the driver names in
/// `FfiEmit::interface_modules` (the reserved `Rust.*` namespace) — a user
/// module can never be shaken. Within a target file everything before the
/// first `pub(crate) fn` (the `use crate::*;` header) is kept
/// unconditionally; each forwarder region (one `pub(crate) fn` to the next)
/// is kept iff its identifier occurs anywhere OUTSIDE the target files.
/// Conservative-keep: an unparseable region shape keeps the whole file, so
/// the shake can never under-keep a called forwarder; over-keep is dead code.
fn shake_interface_forwarder_files(
    files: &mut BTreeMap<RelPath, String>,
    interface_modules: &[String],
) {
    const FN_MARK: &str = "pub(crate) fn ";
    if interface_modules.is_empty() {
        return;
    }
    let target_paths: std::collections::BTreeSet<String> = interface_modules
        .iter()
        .map(|m| {
            let segs: Vec<&str> = m.split('.').collect();
            format!("src/ipe_mods/{}.rs", rust_file::mod_ident(&segs))
        })
        .collect();
    // The reachability haystack: every emitted file that is NOT a forwarder
    // module (forwarders reference only `crate::ffi::` wrappers, never each
    // other, so excluding them is exact).
    let mut haystack = String::new();
    for (path, text) in files.iter() {
        if !target_paths.contains(path.as_str()) {
            haystack.push_str(text);
        }
    }
    for (path, text) in files.iter_mut() {
        if !target_paths.contains(path.as_str()) {
            continue;
        }
        let Some(first) = text.find(FN_MARK) else {
            continue; // no forwarders — keep verbatim
        };
        let (header, mut rest) = text.split_at(first);
        let mut out = String::with_capacity(text.len());
        out.push_str(header);
        // Each region starts at a FN_MARK occurrence and runs to the next.
        while !rest.is_empty() {
            let region_end = rest
                .get(FN_MARK.len()..)
                .and_then(|tail| tail.find(FN_MARK).map(|i| i + FN_MARK.len()))
                .unwrap_or(rest.len());
            let (region, next) = rest.split_at(region_end);
            let ident: String = region
                .get(FN_MARK.len()..)
                .unwrap_or("")
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            // Empty ident = unrecognised shape → conservative keep.
            if ident.is_empty() || haystack.contains(&ident) {
                out.push_str(region);
            }
            rest = next;
        }
        *text = out;
    }
}

/// The `crate::ffi::<ident>` wrapper identifiers referenced anywhere in the
/// emitted Rust sources — the program's reached FFI wrapper set.
///
/// Every `ipe_ir::Callee::Ffi` lowers to a `crate::ffi::<ident>(` call
/// (`emit_expr::callee_name`), so scanning the emitted text is an exhaustive,
/// parse-free reachability oracle.
fn reached_ffi_idents(files: &BTreeMap<RelPath, String>) -> std::collections::BTreeSet<String> {
    const MARK: &str = "crate::ffi::";
    let mut out = std::collections::BTreeSet::new();
    for text in files.values() {
        let mut rest: &str = text;
        while let Some(pos) = rest.find(MARK) {
            let after = &rest[pos + MARK.len()..];
            let ident: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() {
                out.insert(ident);
            }
            rest = after;
        }
    }
    out
}

/// The FFI wrapper-module sentinel bounds (mirror of `ipe_ffi::naming`; the
/// backend may not depend on `ipe_ffi`, so the wire-format literals are
/// re-stated here — a pure text protocol, stable by contract).
const FFI_WRAPPER_BEGIN: &str = "// IPE-FFI-WRAPPER BEGIN ";
const FFI_WRAPPER_END: &str = "// IPE-FFI-WRAPPER END";

/// Text-slice the wrapper module on its BEGIN/END sentinels, keeping preamble
/// unconditionally and only the regions whose `pub fn <ident>(` is reached.
///
/// Conservative-keep: a region whose `pub fn` ident cannot be read (a shape
/// the scan does not recognise) is KEPT, so the shake never drops a wrapper
/// the program calls (an under-bind); over-keep is dead code cargo strips.
fn shake_ffi_by_fn_ident(source: &str, reached: &std::collections::BTreeSet<String>) -> String {
    let mut out = String::with_capacity(source.len());
    // Buffer one wrapper region until its `pub fn` ident is known, then
    // decide keep/drop for the whole region.
    let mut region: Option<(String, bool)> = None; // (buffered text, reached?)
    for line in source.lines() {
        if line.trim_end().starts_with(FFI_WRAPPER_BEGIN) {
            region = Some((String::new(), false));
        }
        if let Some((buf, keep)) = region.as_mut() {
            buf.push_str(line);
            buf.push('\n');
            if let Some(rest) = line.trim_start().strip_prefix("pub fn ") {
                let ident: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                // Accumulate, never overwrite: a region with more than one
                // `pub fn` (not produced by today's generator, but not
                // structurally forbidden either) must stay kept once ANY of
                // its fns is reached — overwriting on the LAST fn seen would
                // drop a region whose FIRST fn is reached but whose last one
                // is not, an under-bind (the reached fn's own wrapper
                // vanishes, an E0425 the linker reports far from this
                // decision point). An ident we cannot read is conservatively
                // kept.
                *keep = *keep || ident.is_empty() || reached.contains(&ident);
            }
            if line.trim_end() == FFI_WRAPPER_END {
                if *keep {
                    out.push_str(buf);
                }
                region = None;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    // A dangling unterminated region (malformed) is kept whole.
    if let Some((buf, _)) = region {
        out.push_str(&buf);
    }
    out
}

/// Append the bound FFI crates' pinned `[dependencies]` lines (driver-merged,
/// exact versions, effective feature sets) before the `[profile.dev]` anchor.
///
/// # Errors
///
/// [`Diagnostic::CompilerBug`] when the FFI emission inputs are absent while
/// the program uses FFI, or when the manifest anchor drifted.
fn ffi_cargo_toml(base: &str, ctx: &EmitCtx) -> DResult<String> {
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    let ffi = ctx.ffi.as_ref().ok_or_else(|| Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::project::ffi_cargo_toml",
        detail: "program lowers foreign-wrapper calls but the driver supplied no FFI \
                 emission inputs (RustBackend::with_ffi)"
            .to_owned(),
    })?;
    // Keys the base manifest already declares under a `[...dependencies]`
    // table: re-declaring one (uuid's transitive `futures-util`, say) is a
    // hard `cargo` duplicate-key error, so those lines are skipped — the
    // base pin governs and cargo's resolver unifies the shared graph.
    let mut base_keys: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut in_deps = false;
    for raw in base.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_deps = line.contains("dependencies");
            continue;
        }
        if in_deps && let Some((name, _)) = line.split_once('=') {
            base_keys.insert(name.trim());
        }
    }
    let mut dep_block = String::new();
    for line in &ffi.dep_lines {
        let key = line.split('=').next().unwrap_or(line).trim();
        if base_keys.contains(key) {
            continue;
        }
        dep_block.push_str(line);
        dep_block.push('\n');
    }
    dep_block.push('\n');
    let anchor_pos = base
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::ffi_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(base.len() + dep_block.len());
    result.push_str(base.get(..anchor_pos).unwrap_or(""));
    result.push_str(&dep_block);
    result.push_str(base.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Emit the `into_sql_param` impl for `SqlValue` and `into_field_param` impl
/// for `SqlField`.
///
/// These are fixed-shape impls — the variant names are always the same Ipê
/// names (`SqlString`, `SqlInt`, …) and the mapping to `ipe_runtime::db::SqlParam`
/// variants is 1-to-1.  Only the enum's Rust *type name* (e.g. `MainSqlValue`)
/// varies per program (depends on the module name prefix).
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] when `ctx.uses_db` is `true` but the
/// Rust names were not computed — an internal invariant violation (the detection
/// in `EmitCtx::build` and the injection in `Lowerer::run` must agree).
fn emit_db_projection_impls(ctx: &EmitCtx) -> DResult<String> {
    let sv = ctx
        .sqlvalue_rust_name
        .as_deref()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::emit_db_projection_impls",
            detail: "uses_db is true but sqlvalue_rust_name is None — \
                 SqlValue was not injected into enum_names"
                .to_owned(),
        })?;
    let sf = ctx
        .sqlfield_rust_name
        .as_deref()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::project::emit_db_projection_impls",
            detail: "uses_db is true but sqlfield_rust_name is None — \
                 SqlField was not injected into enum_names"
                .to_owned(),
        })?;

    // `SqlTime` stores a Unix-millisecond timestamp as `i64` — maps to
    // `SqlParam::Int`.  `SqlDecimal` and `SqlMoney` carry lossless string
    // representations (decimal digits for Decimal, "ISO_CODE AMOUNT" for
    // Money) — both map to `SqlParam::Text`.  `SqlNull` carries a SqlValue
    // type-witness — threaded through (NOT discarded) into
    // `SqlParam::Null(Box<SqlParam>)` so the bind site (`bind_sql_param`)
    // can pick the correctly-typed `Option::<T>::None`, which matters on
    // Postgres (sqlx's extended query protocol validates a per-param
    // type-OID hint against the target column) even though it's a no-op on
    // SQLite's dynamic typing.
    Ok(format!(
        "\
impl {sv} {{
    /// Convert this `SqlValue` into the runtime-nameable `SqlParam`.
    /// Used by `into_field_param` and by legacy call sites that name
    /// this method directly.  New call sites should prefer the `From`
    /// impl below so the emitter can use the uniform `SqlParam::from`
    /// projection for polymorphic `List a` params.
    pub fn into_sql_param(self) -> ipe_runtime::db::SqlParam {{
        match self {{
            Self::SqlString(v) => ipe_runtime::db::SqlParam::Text(v),
            Self::SqlInt(v) => ipe_runtime::db::SqlParam::Int(v),
            Self::SqlFloat(v) => ipe_runtime::db::SqlParam::Float(v),
            Self::SqlBool(v) => ipe_runtime::db::SqlParam::Bool(v),
            Self::SqlBytes(v) => ipe_runtime::db::SqlParam::Bytes(v),
            Self::SqlTime(v) => ipe_runtime::db::SqlParam::Int(v),
            Self::SqlDecimal(v) => ipe_runtime::db::SqlParam::Text(v),
            Self::SqlMoney(v) => ipe_runtime::db::SqlParam::Text(v),
            Self::SqlNull(inner) => ipe_runtime::db::SqlParam::Null(Box::new(inner.into_sql_param())),
        }}
    }}
}}
/// Allow `SqlParam::from(sql_value)` so the emitter can use the same
/// `ipe_runtime::db::SqlParam::from` projection for ALL element types in
/// the polymorphic `Db.exec`/`query` params list (`List a` where `a` may
/// be `String`, `Int`, `Float`, `Bool`, or `SqlValue`).
impl From<{sv}> for ipe_runtime::db::SqlParam {{
    fn from(v: {sv}) -> Self {{
        v.into_sql_param()
    }}
}}
impl {sf} {{
    pub fn into_field_param(self) -> Option<ipe_runtime::db::SqlParam> {{
        match self {{
            Self::SetField(v) => Some(v.into_sql_param()),
            Self::OmitField => None,
        }}
    }}
}}
"
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        CARGO_TOML, RUNTIME_CONFIG_RS_DB_POSTGRES, RUNTIME_CONFIG_RS_DB_SQLITE,
        RUNTIME_MOD_RS_LIVE_APPEND, db_cargo_toml, live_cargo_toml, server_cargo_toml,
        shake_ffi_by_fn_ident,
    };
    use crate::DbDriver;
    use crate::crate_specs;

    /// Helper: extract the `default = [...]` line from a manifest string.
    fn default_line(manifest: &str) -> &str {
        manifest
            .lines()
            .find(|l| l.starts_with("default = ["))
            .expect("manifest must contain a default = [...] line")
    }

    /// `server_cargo_toml` on the NON-db base manifest inserts "server" and does
    /// not insert "db" into the default list.
    #[test]
    fn server_toml_non_db_inserts_server() {
        let out = server_cargo_toml(CARGO_TOML).expect("server_cargo_toml must succeed");
        let def = default_line(&out);
        assert!(
            def.contains(r#""server""#),
            r#"default line must contain "server": {def}"#
        );
        assert!(
            !def.contains(r#""db""#),
            r#"non-db: default line must NOT contain "db": {def}"#
        );
        // Feature declaration must be present.
        assert!(
            out.contains("server = []"),
            "manifest must declare the server feature: {out}"
        );
        // tokio net + sync features must be added.
        assert!(
            out.contains(r#""net""#) && out.contains(r#""sync""#),
            "tokio must gain net + sync features: {out}"
        );
        // axum + tower-http deps must be present.
        assert!(out.contains("axum"), "axum dep must be present: {out}");
        assert!(
            out.contains("tower-http"),
            "tower-http dep must be present: {out}"
        );
    }

    /// `server_cargo_toml` on the DB-enabled manifest inserts "server" ALONGSIDE
    /// "db" — all of "tokio", "crypto", "json", "db", "server" are present in
    /// the default list, and neither overwrites the other.
    #[test]
    fn server_toml_db_compose_inserts_both() {
        let db_base = db_cargo_toml(crate::DbDriver::Sqlite).expect("db_cargo_toml must succeed");
        let out = server_cargo_toml(&db_base).expect("server_cargo_toml on db base must succeed");
        let def = default_line(&out);
        for feat in &[
            r#""tokio""#,
            r#""crypto""#,
            r#""json""#,
            r#""db""#,
            r#""server""#,
        ] {
            assert!(
                def.contains(feat),
                "default line must contain {feat}: {def}"
            );
        }
        // Both feature declarations must be present.
        assert!(
            out.contains("db = []"),
            "manifest must declare the db feature: {out}"
        );
        assert!(
            out.contains("server = []"),
            "manifest must declare the server feature: {out}"
        );
        // sqlx dep (from db_cargo_toml) plus axum dep (from server_cargo_toml)
        // must both be present.
        assert!(out.contains("sqlx"), "sqlx dep must be present: {out}");
        assert!(out.contains("axum"), "axum dep must be present: {out}");
    }

    /// The emitted manifests must carry the SSOT versions — proves the surgery
    /// reads the table, not a stale literal. Closes the loop the drift test
    /// leaves open (SSOT ↔ manifests): this is SSOT ↔ emitted output.
    #[test]
    fn emitted_manifests_use_ssot_versions() {
        let db = db_cargo_toml(crate::DbDriver::Sqlite).expect("db_cargo_toml");
        assert!(
            db.contains(&format!(
                "{} = {{ version = \"{}\"",
                crate_specs::SQLX.name,
                crate_specs::SQLX.version
            )),
            "db manifest must emit SSOT sqlx version:\n{db}"
        );
        let srv = server_cargo_toml(CARGO_TOML).expect("server_cargo_toml");
        assert!(
            srv.contains(&format!(
                "{} = {{ version = \"{}\", features = [\"ws\"]",
                crate_specs::AXUM.name,
                crate_specs::AXUM.version
            )),
            "server manifest must emit SSOT axum version:\n{srv}"
        );
    }

    // ── seal tests: Db+Live closure ─────────────────────────────────────

    /// `live_cargo_toml` on a DB+Server base manifest must extend the sqlx dep
    /// with the `"postgres"` feature.
    ///
    /// Root cause: `live/store.rs`'s `PostgresStore` references `sqlx::PgPool`
    /// gated on `#[cfg(feature = "db")]`.  The runtime's own `Cargo.toml` has
    /// `["runtime-tokio-rustls", "sqlite", "postgres"]`; the emitted project must
    /// match.  Without this, a Db+Live program passes `ipe` then fails `cargo
    /// build` with E0433 (`use of undeclared crate or module sqlx` in
    /// `PgPool::connect`).
    ///
    /// Note: in `emit_program` the call chain is always
    /// `db_cargo_toml → server_cargo_toml → live_cargo_toml` when a program
    /// uses Db AND Live. `live_cargo_toml` expects the tokio `"net"/"sync"`
    /// features already present from `server_cargo_toml`, so the test must mirror
    /// that composition order.
    #[test]
    fn live_db_toml_includes_postgres() {
        let db_base = db_cargo_toml(crate::DbDriver::Sqlite).expect("db_cargo_toml must succeed");
        // server_cargo_toml always runs before live_cargo_toml when uses_live is
        // true (see emit_program).  It adds the tokio net+sync features that
        // live_cargo_toml's anchor requires.
        let server_base =
            server_cargo_toml(&db_base).expect("server_cargo_toml on db base must succeed");
        let out =
            live_cargo_toml(&server_base).expect("live_cargo_toml on db+server base must succeed");
        // The sqlx line must carry the postgres feature.
        let sqlx_line = out
            .lines()
            .find(|l| l.trim_start().starts_with(crate_specs::SQLX.name))
            .expect("sqlx dep must be present in a db+live manifest");
        assert!(
            sqlx_line.contains("\"postgres\""),
            "sqlx dep must include the postgres feature (E0433 fix): {sqlx_line}"
        );
        // Regression: sqlite must still be present.
        assert!(
            sqlx_line.contains("\"sqlite\""),
            "sqlx dep must still include the sqlite feature (no regression): {sqlx_line}"
        );
        // Regression: the live feature must be in the default list.
        let def_line = out
            .lines()
            .find(|l| l.starts_with("default = ["))
            .expect("manifest must contain a default = [...] line");
        assert!(
            def_line.contains(r#""live""#),
            "live feature must be in the default list: {def_line}"
        );
    }

    /// `live_cargo_toml` on a LIVE-ONLY (non-db) base manifest must NOT add a
    /// postgres dep (no sqlx line exists, no-op replace).
    #[test]
    fn live_only_toml_no_postgres() {
        let server_base = server_cargo_toml(CARGO_TOML).expect("server_cargo_toml must succeed");
        let out =
            live_cargo_toml(&server_base).expect("live_cargo_toml on non-db base must succeed");
        assert!(
            !out.contains("\"postgres\""),
            "a Live-only (no Db) manifest must NOT contain the postgres feature: {out}"
        );
    }

    // ── Class 7 §3: Postgres driver structural reachability ─────────────────

    /// `db_cargo_toml(DbDriver::Sqlite)` must be byte-identical to the
    /// pre-driver-selection output (non-regression: every existing db-enabled
    /// sqlite project's Cargo.toml is unaffected by the driver plumbing).
    #[test]
    fn db_cargo_toml_sqlite_driver_unchanged_sqlx_feature() {
        let out = db_cargo_toml(DbDriver::Sqlite).expect("db_cargo_toml(Sqlite) must succeed");
        assert!(
            out.contains(r#"features = ["runtime-tokio-rustls", "sqlite"]"#),
            "sqlite driver must keep the sqlite sqlx feature, not add postgres: {out}"
        );
        assert!(
            !out.contains(r#""postgres"]"#),
            "sqlite driver must not enable postgres: {out}"
        );
    }

    /// The actual structural fix under test: `driver = "postgres"` must
    /// produce a `Cargo.toml` whose sqlx dependency enables the `"postgres"`
    /// sqlx feature — closing the "Postgres driver structurally unreachable"
    /// gap (a `driver = "postgres"` build with only the sqlite sqlx feature
    /// fails to compile `sqlx::postgres::PgPool` at all).
    ///
    /// `"sqlite"` MUST stay enabled too — this is additive, not exclusive.
    /// An earlier version of this fix dropped `"sqlite"` when the driver was
    /// Postgres; that produced an exit-0-then-cargo-fail SEAL violation
    /// (found by independent review) because the always-emitted
    /// `telemetry_spill`/`live::hub`/`live::store` runtime modules hardcode
    /// `SqlitePool` for their local spill/session persistence, independent
    /// of the app's `[database]` driver choice.
    #[test]
    fn db_cargo_toml_postgres_driver_enables_postgres_sqlx_feature() {
        let out = db_cargo_toml(DbDriver::Postgres).expect("db_cargo_toml(Postgres) must succeed");
        assert!(
            out.contains(r#"features = ["runtime-tokio-rustls", "sqlite", "postgres"]"#),
            "postgres driver must enable both the sqlite sqlx feature (always \
             needed by telemetry_spill/hub/store) and the postgres feature: {out}"
        );
    }

    /// The sqlite `config.rs` template is unchanged by this feature (byte
    /// containment check on the two symbols that matter for driver
    /// dispatch — the full file is covered by the existing runtime crate's
    /// own build).
    #[test]
    fn runtime_config_rs_sqlite_template_has_sqlite_types() {
        assert!(RUNTIME_CONFIG_RS_DB_SQLITE.contains("sqlx::sqlite::SqlitePool"));
        assert!(RUNTIME_CONFIG_RS_DB_SQLITE.contains("DB_USES_RETURNING_ID: bool = false"));
    }

    /// The new Postgres `config.rs` template must declare `PgPool`/`PgRow`
    /// and `DB_USES_RETURNING_ID = true` — the two symbols
    /// `db_insert_row`/`db_insert_fields` (Class 7 §4b) key their
    /// `RETURNING id` branch on.
    #[test]
    fn runtime_config_rs_postgres_template_has_postgres_types() {
        assert!(RUNTIME_CONFIG_RS_DB_POSTGRES.contains("sqlx::postgres::PgPool"));
        assert!(RUNTIME_CONFIG_RS_DB_POSTGRES.contains("sqlx::postgres::PgRow"));
        assert!(RUNTIME_CONFIG_RS_DB_POSTGRES.contains("DB_USES_RETURNING_ID: bool = true"));
        assert!(RUNTIME_CONFIG_RS_DB_POSTGRES.contains("id BIGSERIAL PRIMARY KEY"));
    }

    /// `RUNTIME_MOD_RS_LIVE_APPEND` must re-export `LiveReq` from the `live`
    /// module.
    ///
    /// Root cause: `db.rs` has `#[cfg(feature = "live")] impl IpeRow for
    /// super::LiveReq` — `super::LiveReq` means `ipe_runtime::LiveReq`.  The
    /// runtime source's `mod.rs` uses `pub use live::*;` which surfaces `LiveReq`
    /// (via `live/mod.rs`'s `pub use req::*;`).  The emitted project uses a
    /// selective export list; without `LiveReq` a Db+Live program fails with E0412.
    #[test]
    fn live_mod_rs_exports_live_req() {
        assert!(
            RUNTIME_MOD_RS_LIVE_APPEND.contains("LiveReq"),
            "RUNTIME_MOD_RS_LIVE_APPEND must re-export LiveReq from the live module (E0412 fix): \
             {RUNTIME_MOD_RS_LIVE_APPEND}"
        );
    }

    /// `RUNTIME_MOD_RS_LIVE_APPEND` must re-export `cmd_publish`,
    /// `cmd_publish_no_echo`, `pubsub_publish`, and `pubsub_publish_no_echo` so
    /// that emitted call sites resolve.  Without this the emitted project fails
    /// with E0425 — a seal violation.
    #[test]
    fn live_mod_rs_exports_cmd_publish_fns() {
        assert!(
            RUNTIME_MOD_RS_LIVE_APPEND.contains("cmd_publish"),
            "RUNTIME_MOD_RS_LIVE_APPEND must re-export cmd_publish (E0425 fix): \
             {RUNTIME_MOD_RS_LIVE_APPEND}"
        );
        assert!(
            RUNTIME_MOD_RS_LIVE_APPEND.contains("cmd_publish_no_echo"),
            "RUNTIME_MOD_RS_LIVE_APPEND must re-export cmd_publish_no_echo (E0425 fix): \
             {RUNTIME_MOD_RS_LIVE_APPEND}"
        );
        assert!(
            RUNTIME_MOD_RS_LIVE_APPEND.contains("pubsub_publish"),
            "RUNTIME_MOD_RS_LIVE_APPEND must re-export pubsub_publish (E0425 fix, #215): \
             {RUNTIME_MOD_RS_LIVE_APPEND}"
        );
        assert!(
            RUNTIME_MOD_RS_LIVE_APPEND.contains("pubsub_publish_no_echo"),
            "RUNTIME_MOD_RS_LIVE_APPEND must re-export pubsub_publish_no_echo (E0425 fix, #215): \
             {RUNTIME_MOD_RS_LIVE_APPEND}"
        );
    }

    /// One wrapper region built from a single generator BEGIN/END span with
    /// two `pub fn`s: `first` then `second`, wrapped exactly like
    /// `shake_ffi_by_fn_ident`'s doc comment describes.
    fn two_fn_region(first: &str, second: &str) -> String {
        format!(
            "// preamble\n\
             // IPE-FFI-WRAPPER BEGIN region\n\
             pub fn {first}(x: i64) -> i64 {{\n    x\n}}\n\
             pub fn {second}(x: i64) -> i64 {{\n    x\n}}\n\
             // IPE-FFI-WRAPPER END\n\
             // trailer\n"
        )
    }

    /// CO-BACKEND-007: a region with TWO `pub fn`s where only the FIRST is
    /// reached must stay KEPT — the last-fn-wins bug dropped it because the
    /// decision was overwritten by the second (unreached) fn's verdict.
    #[test]
    fn shake_keeps_region_when_only_the_first_of_two_fns_is_reached() {
        let source = two_fn_region("reached_fn", "unreached_fn");
        let reached: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::from(["reached_fn".to_owned()]);
        let out = shake_ffi_by_fn_ident(&source, &reached);
        assert!(
            out.contains("pub fn reached_fn"),
            "the reached fn's own wrapper must survive: {out}"
        );
        assert!(
            out.contains("pub fn unreached_fn"),
            "the whole region (both fns) must be kept once ANY fn in it is reached: {out}"
        );
    }

    /// Mirror case: only the SECOND of two fns is reached. Already passed
    /// under the old last-wins logic (the second fn's verdict IS the final
    /// one), but pins the same invariant from the other direction.
    #[test]
    fn shake_keeps_region_when_only_the_second_of_two_fns_is_reached() {
        let source = two_fn_region("unreached_fn", "reached_fn");
        let reached: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::from(["reached_fn".to_owned()]);
        let out = shake_ffi_by_fn_ident(&source, &reached);
        assert!(
            out.contains("pub fn reached_fn") && out.contains("pub fn unreached_fn"),
            "the whole region must be kept once ANY fn in it is reached: {out}"
        );
    }

    /// A `define.enum` region carries the `enum` definition PLUS one ctor per
    /// variant in a SINGLE sentinel span. Reaching just ONE variant forwarder
    /// must keep the whole region — the enum def and EVERY sibling ctor — or the
    /// kept forwarder references a dropped ctor / a missing type (a cargo-fail
    /// far from here). Proves the multi-ctor define.enum region is shake-safe.
    #[test]
    fn shake_keeps_the_whole_define_enum_region_when_one_variant_is_reached() {
        let source = "// preamble\n\
             // IPE-FFI-WRAPPER BEGIN message_new\n\
             #[derive(Clone, Debug)]\n\
             pub enum Message { Increment, Decrement }\n\
             pub fn demo_message_new_increment() -> Message { Message::Increment }\n\
             pub fn demo_message_new_decrement() -> Message { Message::Decrement }\n\
             // IPE-FFI-WRAPPER END\n\
             // trailer\n";
        // Only the Increment forwarder is reached by user code.
        let reached: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::from(["demo_message_new_increment".to_owned()]);
        let out = shake_ffi_by_fn_ident(source, &reached);
        assert!(
            out.contains("pub enum Message"),
            "enum def must survive: {out}"
        );
        assert!(
            out.contains("pub fn demo_message_new_increment"),
            "the reached variant ctor must survive: {out}"
        );
        assert!(
            out.contains("pub fn demo_message_new_decrement"),
            "the sibling ctor must survive so the kept region compiles: {out}"
        );
    }

    /// A region whose fns are ALL unreached is still dropped — the fix must
    /// not turn the shake into a no-op.
    #[test]
    fn shake_drops_region_when_no_fn_is_reached() {
        let source = two_fn_region("unreached_a", "unreached_b");
        let reached: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let out = shake_ffi_by_fn_ident(&source, &reached);
        assert!(
            !out.contains("pub fn unreached_a") && !out.contains("pub fn unreached_b"),
            "a region with no reached fn must be dropped: {out}"
        );
        assert!(
            out.contains("// preamble") && out.contains("// trailer"),
            "surrounding non-region text must survive untouched: {out}"
        );
    }
}
