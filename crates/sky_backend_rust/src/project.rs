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

use std::collections::BTreeMap;

use sky_backend::{EmittedProject, RelPath};
use sky_diagnostics::{DResult, Diagnostic};
use sky_ir::{Program, TypeDef};

use crate::EmitCtx;
use crate::emit_expr::emit_func;
use crate::emit_types::{emit_enum, emit_record_struct};
use crate::preamble::{epilogue, preamble};

/// The golden M0 program, embedded at compile time. The fixed runtime-bindings
/// block (kernel wrappers, golden lines 45–127) is an exact substring of it.
const GOLDEN: &str = include_str!("../../../tests/golden/m0/main.rs");

/// The project `Cargo.toml`, embedded verbatim from the golden. M0 emits the
/// same manifest for every program (dependency set is fixed by the runtime).
const CARGO_TOML: &str = include_str!("../../../tests/golden/m0/Cargo.toml");

/// The generated `sky_runtime/mod.rs` — the curated set of runtime modules whose
/// dependencies are satisfied by [`CARGO_TOML`]. The vendored runtime source
/// ships a fuller `mod.rs` (declaring `uuid` / `live` / `db` / … modules that
/// pull crates outside the M0 manifest); the driver overwrites it with this
/// trimmed version. M0 emits a fixed module set; later milestones compute it
/// from the kernels a program actually uses.
const RUNTIME_MOD_RS: &str = include_str!("../../../tests/golden/m0/sky_runtime/mod.rs");

/// The generated `sky_runtime/config.rs` (DB/config bindings — empty for M0).
const RUNTIME_CONFIG_RS: &str = include_str!("../../../tests/golden/m0/sky_runtime/config.rs");

// ── M5b-db: db-enabled manifest fragments ──────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses Db kernels.
///
/// The full runtime source tree is copied into the emitted project by the
/// driver; this addition wires `db.rs` (which lives in that tree) into the
/// module namespace so the generated `main.rs` can call the db functions.
const RUNTIME_MOD_RS_DB_APPEND: &str = "pub mod db;\npub use db::*;\npub mod telemetry_spill;\n";

// ── M5c: TEA Cmd / Sub ─────────────────────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses TEA kernels
/// (`Cmd.none / batch / perform`, `Sub.none / batch / every`, `Time.every`).
///
/// `tea.rs` lives in the runtime source tree (ungated — no cargo feature needed
/// for M5c); this addition makes `cmd_none` / `sub_every` / … available in the
/// emitted `main.rs` namespace via `pub use sky_runtime::*`.
const RUNTIME_MOD_RS_TEA_APPEND: &str = "pub mod tea;\npub use tea::*;\n";

// ── M6: Sky.Http.Server ──────────────────────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses
/// Sky.Http.Server kernels.
///
/// Both `server.rs` and `server_stream.rs` are gated by the `server` Cargo
/// feature in the runtime source; the generated Cargo.toml's default features
/// include `"server"` when these lines are appended.
const RUNTIME_MOD_RS_SERVER_APPEND: &str =
    "pub mod server;\npub use server::*;\npub mod server_stream;\npub use server_stream::*;\n";

// ── M7: Std.Ui / Std.Html ───────────────────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses Std.Ui /
/// Std.Html render kernels.
///
/// `html.rs` and `ui/mod.rs` are in the runtime source tree; this addition
/// wires both into the module namespace. `html` is always paired with `ui`
/// because the `ui::element` and `ui::render` modules import from `html`.
/// Note: intentionally NOT `pub use ui::*;` because `ui::Attribute` collides
/// with `html::Attribute` (T2 soundness trap) — callers use the fully-qualified
/// `sky_runtime::ui::element::Attribute` path instead.
const RUNTIME_MOD_RS_UI_APPEND: &str = "pub mod html;\npub use html::*;\npub mod ui;\n";

// ── Phase-1c: Std.Tui / Sky.Tui ─────────────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses Std.Tui /
/// Sky.Tui app-entry kernels.
///
/// Both `tui/app.rs` and `tui/layout.rs` (and their dependencies `cell.rs`,
/// `diff.rs`, `focus.rs`, `key.rs`) are gated by the `tui` Cargo feature in the
/// runtime source.  This addition wires `tui::tui_app` and `tui::tui_app_ui`
/// into the module namespace so the generated `main.rs` can call them via
/// `sky_runtime::tui::tui_app_ui`.
///
/// The `ui` module must also be loaded (tui/layout.rs imports `super::ui::Element`)
/// — but `uses_ui` is set whenever `uses_tui` is set (a Tui app always references
/// Std.Ui Element/attribute kernels), so `RUNTIME_MOD_RS_UI_APPEND` is already
/// appended by the time this addition fires.
const RUNTIME_MOD_RS_TUI_APPEND: &str = "#[cfg(feature = \"tui\")]\npub mod tui;\n\
     #[cfg(feature = \"tui\")]\npub use tui::{tui_app, tui_app_ui};\n";

// ── Phase-1b: Std.Live / Sky.Live ───────────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses Std.Live /
/// Sky.Live app-entry kernels.
///
/// `live/mod.rs` is gated by the `live` Cargo feature in the runtime source;
/// this addition wires the `live` module (and its public re-exports `live_app`,
/// `live_app_routed`, `live_render_static`, `live::route::Route`) into the
/// module namespace so the generated `main.rs` can call them.
///
/// The `route` sub-module is referenced by path (`sky_runtime::live::route::Route`)
/// not via `pub use live::*;` (to avoid surfacing the internal `store` / `req`
/// internals in the top-level namespace).
const RUNTIME_MOD_RS_LIVE_APPEND: &str = "#[cfg(feature = \"live\")]\npub mod live;\n\
     #[cfg(feature = \"live\")]\npub use live::{live_app, live_app_routed, live_render_static};\n";

/// The `SkyCmd<M>` and `SkySub<M>` project-level type aliases emitted when the
/// program uses TEA kernels. Placed immediately after `runtime_bindings()` (the
/// block that also contains `SkyTask<A>` and `Decoder<T>`).
const TEA_TYPE_ALIASES: &str = "pub type SkyCmd<M> = sky_runtime::tea::SkyCmd<M>;\n\
     pub type SkySub<M> = sky_runtime::tea::SkySub<M>;\n";

/// The `sky_runtime/config.rs` emitted for db-enabled programs. Replaces the
/// no-op M0 stub with the `SQLite` type aliases + helper fns the `db.rs` module
/// requires. Mirrors `runtime/src/sky_runtime/config.rs` verbatim, keeping the
/// `#[cfg(feature = "db")]` / `#[cfg(not(feature = "db"))]` guards so a
/// non-db build (hypothetically possible via feature flag override) degrades
/// gracefully rather than failing with undefined types.
const RUNTIME_CONFIG_RS_DB: &str = include_str!("../../../runtime/src/sky_runtime/config.rs");

/// The `Diagnostic::CompilerBug` raised when a golden anchor is absent — a
/// drifted-golden invariant violation, surfaced (SKY-I0203) instead of a silent
/// empty slice.
fn anchor_missing(anchor: &str) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "backend.golden_anchor",
        detail: format!("golden anchor {anchor:?} not found in the embedded M0 golden"),
    }
}

/// The fixed kernel-wrapper prelude emitted between the user types and the user
/// functions (golden lines 45–127).
///
/// These bindings (`SkyError`, the `log_*` / `system_*` / `time_*` / … wrappers)
/// are identical for every M0 program, so they are sliced out of the embedded
/// golden rather than hand-retyped — the same drift-free strategy the
/// preamble/epilogue use. The slice is anchored entirely on its *own* content
/// (the first alias and the final `http_parse_query` wrapper), independent of
/// the surrounding user code.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] (SKY-I0203) if either anchor is absent
/// from the embedded golden — a drifted-golden invariant violation, surfaced
/// instead of a silent empty slice.
fn runtime_bindings() -> DResult<&'static str> {
    const START: &str = "type SkyError = String;";
    const END: &str = "    sky_runtime::http_client::http_parse_query(raw)\n}\n";
    let start = GOLDEN.find(START).ok_or_else(|| anchor_missing(START))?;
    let rest = GOLDEN.get(start..).ok_or_else(|| anchor_missing(START))?;
    let end_in_rest = rest.find(END).ok_or_else(|| anchor_missing(END))?;
    let end = start + end_in_rest + END.len();
    GOLDEN.get(start..end).ok_or_else(|| anchor_missing(END))
}

/// Emit the complete project for `program`.
pub fn emit_program(ctx: &EmitCtx, program: &Program) -> DResult<EmittedProject> {
    let mut out = String::new();
    out.push_str(&preamble()?);

    // User types, emitted from the IR.
    for module in &program.modules {
        for ty in &module.types {
            let TypeDef::Enum(def) = ty;
            out.push_str(&emit_enum(ctx, def)?);
        }
    }
    // Synthesised record structs, one per distinct closed record shape. Item
    // order is irrelevant in Rust, so these can reference one another freely;
    // a program with no records emits nothing here, keeping output unchanged.
    for rec in ctx.record_structs() {
        out.push_str(&emit_record_struct(ctx, rec)?);
    }

    // M5b-db: boundary-projection impl blocks.  When the program uses Db
    // kernels, the lowerer injected synthetic `SqlValue` / `SqlField` enums.
    // The Db call sites need to project Sky ADT values to the runtime's
    // concrete `SqlParam` / `Option<SqlParam>`.  These impls are emitted
    // immediately after the user types (and their record-struct companions) so
    // they are visible to every subsequent function body.
    if ctx.uses_db {
        out.push_str(&emit_db_projection_impls(ctx)?);
    }

    out.push('\n');

    // Fixed kernel-wrapper prelude (SkyError, SkyTask<A>, Decoder<T>, wrappers).
    out.push_str(runtime_bindings()?);

    // M5c: when the program uses TEA kernels, add the SkyCmd<M> / SkySub<M>
    // type aliases immediately after the other top-level type aliases.
    if ctx.uses_tea {
        out.push_str(TEA_TYPE_ALIASES);
    }
    out.push('\n');

    // User functions, emitted from the IR.
    for module in &program.modules {
        for func in &module.funcs {
            out.push_str(&emit_func(ctx, func)?);
        }
    }
    out.push('\n');

    out.push_str(&epilogue()?);

    // ── Manifest + runtime module files ──────────────────────────────────────
    // The driver (skyc) first copies the full runtime source tree into
    // `<out>/src/sky_runtime/`, then writes the emitted files over the top.
    // So we only need to emit the files that differ from the raw source tree:
    //
    //   • `mod.rs` — trimmed to the kernel set the program uses (non-db path
    //     keeps the M0 default; db path appends `pub mod db; pub use db::*;`).
    //   • `config.rs` — the M0 stub for non-db; the full db-type-alias file
    //     for db programs (provides `DbPool`, `DbRow`, `SKY_DB_URL`, …).
    //   • `Cargo.toml` — adds `db` to default features + `sqlx` dep for db.
    // Build the manifest + runtime module selection based on which kernel groups
    // are used. Db, TEA, and Server are independent features; a program may use
    // any combination. The order: db first, then server; both modify the same
    // base manifest so we chain the transformations.
    let (cargo_toml, runtime_config_rs) = if ctx.uses_db {
        (db_cargo_toml()?, RUNTIME_CONFIG_RS_DB.to_owned())
    } else {
        (CARGO_TOML.to_owned(), RUNTIME_CONFIG_RS.to_owned())
    };
    // Apply server manifest extension on top of whichever base was chosen above.
    // Phase-1b: Live also needs axum + tower-http (the live runtime uses axum
    // internally).  Apply server_cargo_toml for both `uses_server` and
    // `uses_live` (the function is idempotent when both flags are set because
    // `server = []` is only inserted once via `replacen`).
    let cargo_toml = if ctx.uses_server || ctx.uses_live {
        server_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // Phase-1b: when the program uses Live, add "live" to the default features.
    // The base manifest already declares `live = []` as a non-default feature;
    // we just need to promote it to the `default` list so the compiled binary
    // includes the `live` module.
    let cargo_toml = if ctx.uses_live {
        live_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // Phase-1c: when the program uses Tui, add "tui" to the default features
    // and inject the crossterm + unicode-width deps required by the tui runtime.
    // The base manifest declares `tui = []` as a non-default feature; we promote
    // it and add the deps so the compiled binary includes the `tui` module.
    let cargo_toml = if ctx.uses_tui {
        tui_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // mod.rs starts from the M0 default and gains extra `pub mod` lines for
    // each kernel group the program uses.
    let runtime_mod_rs = {
        let mut mod_rs = RUNTIME_MOD_RS.to_owned();
        if ctx.uses_db {
            mod_rs.push_str(RUNTIME_MOD_RS_DB_APPEND);
        }
        if ctx.uses_tea {
            mod_rs.push_str(RUNTIME_MOD_RS_TEA_APPEND);
        }
        if ctx.uses_server {
            mod_rs.push_str(RUNTIME_MOD_RS_SERVER_APPEND);
        }
        // M7: Std.Ui / Std.Html render kernels.
        if ctx.uses_ui {
            mod_rs.push_str(RUNTIME_MOD_RS_UI_APPEND);
        }
        // Phase-1b: Std.Live / Sky.Live app-entry kernels.
        if ctx.uses_live {
            mod_rs.push_str(RUNTIME_MOD_RS_LIVE_APPEND);
        }
        // Phase-1c: Std.Tui / Sky.Tui app-entry kernels.
        if ctx.uses_tui {
            mod_rs.push_str(RUNTIME_MOD_RS_TUI_APPEND);
        }
        mod_rs
    };

    let mut files = BTreeMap::new();
    files.insert(RelPath::new("src/main.rs")?, out);
    files.insert(RelPath::new("src/sky_runtime/mod.rs")?, runtime_mod_rs);
    files.insert(
        RelPath::new("src/sky_runtime/config.rs")?,
        runtime_config_rs,
    );
    Ok(EmittedProject { files, cargo_toml })
}

/// Build the db-enabled `Cargo.toml` from the base M0 manifest by:
///
/// 1. Adding `"db"` to the `default` feature list.
/// 2. Appending the `sqlx` dependency line.
///
/// String surgery rather than a second static file: the manifest content is
/// small and the two edits are unambiguous anchors.
fn db_cargo_toml() -> DResult<String> {
    const DEFAULT_LINE: &str = r#"default = ["tokio", "crypto", "json"]"#;
    const DEFAULT_LINE_DB: &str = r#"default = ["tokio", "crypto", "json", "db"]"#;
    // The sqlx line is appended right before the dev/release profile sections.
    // Anchoring on `[profile.dev]` is stable (always present in the template).
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    const SQLX_LINE: &str =
        "sqlx = { version = \"0.8\", features = [\"runtime-tokio-rustls\", \"sqlite\"] }\n\n";

    let step1 = CARGO_TOML.replacen(DEFAULT_LINE, DEFAULT_LINE_DB, 1);
    if step1 == CARGO_TOML {
        return Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::db_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_LINE:?} not found — golden drifted"),
        });
    }
    let anchor_pos = step1
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::db_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step1.len() + SQLX_LINE.len());
    result.push_str(step1.get(..anchor_pos).unwrap_or(""));
    result.push_str(SQLX_LINE);
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
    const TOKIO_TIME: &str =
        r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time"] }"#;
    const TOKIO_NET_SYNC: &str = r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time", "net", "sync"] }"#;
    const PROFILE_ANCHOR: &str = "[profile.dev]";
    const SERVER_DEPS: &str = "axum = { version = \"0.7\", features = [\"ws\"] }\n\
         tower-http = { version = \"0.5\", features = [\"fs\", \"catch-panic\"] }\n\n";

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
            where_: "sky_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::server_cargo_toml",
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
            where_: "sky_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {DB_FEATURE:?} not found — golden drifted"),
        });
    }

    // Step 2 — extend the tokio dependency line with "net" and "sync".
    let step2 = step1.replacen(TOKIO_TIME, TOKIO_NET_SYNC, 1);
    if step2 == step1 {
        return Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {TOKIO_TIME:?} not found — golden drifted"),
        });
    }

    // Step 3 — append axum + tower-http before `[profile.dev]`.
    let anchor_pos = step2
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step2.len() + SERVER_DEPS.len());
    result.push_str(step2.get(..anchor_pos).unwrap_or(""));
    result.push_str(SERVER_DEPS);
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
/// (present in the M0 golden's `[features]` section).  This function promotes
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
    const LIVE_DEPS: &str = "async-trait = \"0.1\"\nserde_urlencoded = \"0.7\"\nlibc = \"0.2\"\n\n";
    // The `live` runtime mainline uses `tokio::signal` + `tokio::process`; the base
    // golden emits `net`+`sync` for the HTTP server, so add the two missing features.
    const TOKIO_NET_SYNC_FEATURES: &str = "\"time\", \"net\", \"sync\"]";
    const TOKIO_LIVE_FEATURES: &str = "\"time\", \"net\", \"sync\", \"signal\", \"process\"]";

    // Step 1 — promote the `live` feature.
    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::live_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::live_cargo_toml",
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
            where_: "sky_backend_rust::project::live_cargo_toml",
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
            where_: "sky_backend_rust::project::live_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step1.len() + LIVE_DEPS.len());
    result.push_str(step1.get(..anchor_pos).unwrap_or(""));
    result.push_str(LIVE_DEPS);
    result.push_str(step1.get(anchor_pos..).unwrap_or(""));
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
    const TUI_DEPS: &str = "crossterm = \"0.28\"\nunicode-width = \"0.1\"\n\n";
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
    const TOKIO_TIME_ONLY: &str =
        r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time"] }"#;
    const TOKIO_TIME_SYNC: &str = r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time", "sync"] }"#;

    // Step 1 — promote the `tui` feature (generic closing-`]` anchor, same
    // strategy as server_cargo_toml / live_cargo_toml).
    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::tui_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::tui_cargo_toml",
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
        let replaced = step1.replacen(TOKIO_TIME_ONLY, TOKIO_TIME_SYNC, 1);
        if replaced == step1 {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_backend_rust::project::tui_cargo_toml",
                detail: format!(
                    "tokio anchor {TOKIO_TIME_ONLY:?} not found and no \"sync\" present — \
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
            where_: "sky_backend_rust::project::tui_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step2.len() + TUI_DEPS.len());
    result.push_str(step2.get(..anchor_pos).unwrap_or(""));
    result.push_str(TUI_DEPS);
    result.push_str(step2.get(anchor_pos..).unwrap_or(""));
    Ok(result)
}

/// Emit the `into_sql_param` impl for `SqlValue` and `into_field_param` impl
/// for `SqlField`.
///
/// These are fixed-shape impls — the variant names are always the same Sky
/// names (`SqlString`, `SqlInt`, …) and the mapping to `sky_runtime::db::SqlParam`
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
            where_: "sky_backend_rust::project::emit_db_projection_impls",
            detail: "uses_db is true but sqlvalue_rust_name is None — \
                 SqlValue was not injected into enum_names"
                .to_owned(),
        })?;
    let sf = ctx
        .sqlfield_rust_name
        .as_deref()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::emit_db_projection_impls",
            detail: "uses_db is true but sqlfield_rust_name is None — \
                 SqlField was not injected into enum_names"
                .to_owned(),
        })?;

    // `SqlTime` stores a Unix-millisecond timestamp as `i64` — maps to
    // `SqlParam::Int`.  `SqlDecimal` and `SqlMoney` carry lossless string
    // representations (decimal digits for Decimal, "ISO_CODE AMOUNT" for
    // Money) — both map to `SqlParam::Text`.  `SqlNull` carries a SqlValue
    // type-witness that is discarded here; the runtime sees just `SqlParam::Null`.
    Ok(format!(
        "\
impl {sv} {{
    pub fn into_sql_param(self) -> sky_runtime::db::SqlParam {{
        match self {{
            Self::SqlString(v) => sky_runtime::db::SqlParam::Text(v),
            Self::SqlInt(v) => sky_runtime::db::SqlParam::Int(v),
            Self::SqlFloat(v) => sky_runtime::db::SqlParam::Float(v),
            Self::SqlBool(v) => sky_runtime::db::SqlParam::Bool(v),
            Self::SqlBytes(v) => sky_runtime::db::SqlParam::Bytes(v),
            Self::SqlTime(v) => sky_runtime::db::SqlParam::Int(v),
            Self::SqlDecimal(v) => sky_runtime::db::SqlParam::Text(v),
            Self::SqlMoney(v) => sky_runtime::db::SqlParam::Text(v),
            Self::SqlNull(_) => sky_runtime::db::SqlParam::Null,
        }}
    }}
}}
impl {sf} {{
    pub fn into_field_param(self) -> Option<sky_runtime::db::SqlParam> {{
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
    use super::{CARGO_TOML, db_cargo_toml, server_cargo_toml};

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
        let db_base = db_cargo_toml().expect("db_cargo_toml must succeed");
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
}
