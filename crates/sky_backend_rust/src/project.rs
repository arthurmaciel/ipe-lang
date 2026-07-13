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

use sky_backend::{EmittedProject, RelPath};
use sky_diagnostics::{DResult, Diagnostic};
use sky_ir::Program;

use crate::EmitCtx;
use crate::crate_specs;
use crate::emit_expr::emit_func;
use crate::emit_types::{emit_enum, emit_record_struct};
use crate::preamble::{epilogue, preamble};
use crate::rust_file::{Partitioned, RustFileId, partition_items};

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
// `http_header.rs` (a dependency-free leaf exposing `canonical_header`) used
// to be a CONDITIONAL append here, guarded on
// `uses_server || uses_live || uses_webview` — it was referenced only by
// `server.rs` and `live/req.rs`. Since #33 §6.1 the outbound `http_client.rs`
// response path (part of the M0 BASE module set) also calls it, so it moved
// into the base `mod.rs` (`tests/golden/m0/sky_runtime/mod.rs`) and the
// conditional append was removed — re-adding one would emit a duplicate
// `pub mod http_header;` (E0428) for server/live programs.

// ── #111: Std.Auth ──────────────────────────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses Std.Auth
/// kernels (`Auth.hashPassword` / `verifyPassword` / `signToken` /
/// `verifyToken` / `register` / `login` / `setRole` etc.).
///
/// `auth.rs` requires `bcrypt` (password hashing) and `jsonwebtoken` (JWT
/// signing/verification); both are unconditional deps in the generated
/// project's `Cargo.toml` (included in the `crypto` and `json` default
/// features), so no manifest surgery is needed — only a `mod.rs` declaration.
const RUNTIME_MOD_RS_AUTH_APPEND: &str = "pub mod auth;\npub use auth::*;\n";

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
///
/// The `css_safety` / `css` declarations are NOT here — they live in
/// [`RUNTIME_MOD_RS_CSS_APPEND`], which is pushed BEFORE this append whenever
/// `uses_ui || uses_css` holds. `html.rs` (`use super::css_safety;`),
/// `ui/render.rs` (`SafeCssPropertyName`/`SafeCssValue`), and
/// `live/style_inject.rs` (`strip_style_close`) all import `css_safety` from the
/// `sky_runtime` top level, so it MUST be declared before this UI append or
/// those imports fail (E0432) — the caller preserves that ordering. Splitting
/// css out lets a pure-`Std.Css` program (no render kernel ⇒ no `uses_ui`) still
/// get the css declarations via `uses_css` alone (#47).
const RUNTIME_MOD_RS_UI_APPEND: &str = "pub mod html;\npub use html::*;\npub mod ui;\n";

/// Lines appended to `sky_runtime/mod.rs` when the program uses the `Std.Css`
/// leaf security kernels (`Sky.Core.CssSafety.safeValue` / `safePropName` /
/// `safeSelector` / `stripStyleClose`, #47) — OR any `Std.Ui` / `Std.Html`
/// render kernel (whose runtime modules import `css_safety` at the top level).
///
/// `css_safety.rs` is a dependency-free, audited leaf; `css.rs` (the four
/// `Std.Css` leaf kernels — `safe_value` / `safe_prop_name` / `safe_selector` /
/// `strip_style_close_kernel`) depends only on `css_safety`, and is glob-re-
/// exported (`pub use css::*;`) so the emitted `pub use sky_runtime::*;`
/// surfaces those bare kernel names that `naming::kernel_name` emits. Both live
/// in the runtime source tree (copied into every emitted project); this append
/// wires them into the trimmed `mod.rs`.
///
/// Pushed BEFORE [`RUNTIME_MOD_RS_UI_APPEND`] because `html.rs` and friends
/// import `css_safety` — it must be declared first. Guarded on
/// `uses_ui || uses_css` and appended AT MOST ONCE, so a program that uses both
/// `Std.Css` and `Std.Ui` does not emit a duplicate `pub mod css_safety;`
/// (`E0428`).
const RUNTIME_MOD_RS_CSS_APPEND: &str = "pub mod css_safety;\npub mod css;\npub use css::*;\n";

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

// ── Phase-1d: Std.Webview / Sky.Webview ─────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses Std.Webview /
/// Sky.Webview app-entry kernels.
///
/// `webview.rs` is gated by the `webview` Cargo feature in the runtime source
/// (wry + tao deps). This addition wires `webview::webview_app` and
/// `webview::WebviewWindowCfg` into the module namespace so the generated
/// `main.rs` can call them.
///
/// The `live` module must also be loaded (webview's real backend imports
/// `sky_runtime::live::dispatch::build_index` and `sky_runtime::html::*`)
/// — but `uses_live` is forced true when `uses_webview` is true
/// (see `emit_program`), so `RUNTIME_MOD_RS_LIVE_APPEND` is already appended
/// by the time this addition fires.
const RUNTIME_MOD_RS_WEBVIEW_APPEND: &str = "#[cfg(feature = \"webview\")]\npub mod webview;\n\
     #[cfg(feature = \"webview\")]\npub use webview::{webview_app, WebviewWindowCfg};\n";

// ── Phase-1b: Std.Live / Sky.Live ───────────────────────────────────────────

/// Lines appended to `sky_runtime/mod.rs` when the program uses Std.Live /
/// Sky.Live app-entry kernels.
///
/// `live/mod.rs` is gated by the `live` Cargo feature in the runtime source;
/// this addition wires the `live` module (and its public re-exports `live_app`,
/// `live_app_routed`, `live_render_static`, `live::route::Route`,
/// `sub_subscribe_topic`, `LiveReq`) into the module namespace so the generated
/// `main.rs` can call them.
///
/// `sub_subscribe_topic` is the `Sub.subscribeTopic` runtime kernel (M5d); it
/// lives in `live/pubsub.rs` because it needs the session-aware broker.
///
/// `LiveReq` MUST be re-exported here (transitive-closure invariant). The
/// runtime's `db.rs` module contains a `#[cfg(feature = "live")] impl SkyRow for
/// super::LiveReq` block — `super::LiveReq` means `sky_runtime::LiveReq`. In the
/// real runtime source `mod.rs` uses `pub use live::*;` which surfaces `LiveReq`
/// (via `live/mod.rs`'s own `pub use req::*;`), but the emitted project uses a
/// selective export list.  Without `LiveReq` here, any program that uses BOTH Db
/// and Live kernels fails with E0412 (`LiveReq in super` not found) at
/// `db.rs:impl SkyRow for super::LiveReq`.
///
/// The `route` sub-module is referenced by path (`sky_runtime::live::route::Route`)
/// not via `pub use live::*;` (to avoid surfacing the internal `store` / `req`
/// internals in the top-level namespace).
const RUNTIME_MOD_RS_LIVE_APPEND: &str = "#[cfg(feature = \"live\")]\npub mod live;\n\
     #[cfg(feature = \"live\")]\npub use live::{live_app, live_app_routed, live_render_static, sub_subscribe_topic, cmd_publish, cmd_publish_no_echo, LiveReq};\n";

/// The `SkyCmd<M>` and `SkySub<M>` project-level type aliases emitted when the
/// program uses TEA kernels. Placed immediately after `runtime_bindings()` (the
/// block that also contains `SkyTask<A>` and `Decoder<T>`).
const TEA_TYPE_ALIASES: &str = "pub type SkyCmd<M> = sky_runtime::tea::SkyCmd<M>;\n\
     pub type SkySub<M> = sky_runtime::tea::SkySub<M>;\n";

// ── #111: Std.Auth — concrete wrappers emitted when uses_auth is true ────────

/// Concrete wrappers appended to `main.rs` when the program uses Std.Auth
/// kernels.  Each wrapper specialises the generic `E` type parameter to
/// `SkyError` so call sites in user function bodies compile without requiring
/// a turbofish annotation.
///
/// backlog #44: `auth_sign_token` / `auth_verify_token` take a Sky-typed
/// `sky_runtime::secret::Secret` (not `String`) at this boundary — "secrets
/// are typed, never `fmt`-stringified" (`PRINCIPLES.md`). The wrapper reveals
/// it via `sky_runtime::secret::secret_reveal` immediately before delegating
/// to the runtime's `String`-typed `sky_runtime::auth::{auth_sign_token,
/// auth_verify_token}` — the runtime crate's own low-level signature is left
/// unchanged (it has no dependency on `secret.rs`); the typed boundary lives
/// entirely at this Sky-facing wrapper, matching the fix spec's design.
///
/// `auth_register`, `auth_login`, and `auth_set_role` are gated on
/// `#[cfg(feature = "db")]` in the runtime source, so the three wrappers
/// here are also gated.  A non-db Auth-only program (using only `hashPassword`
/// / `verifyPassword` / `signToken` / `verifyToken`) will compile the four
/// ungated wrappers + the `passwordStrength` helper and ignore the db-gated
/// three.  When `uses_db` is also true the `db` feature is in the generated
/// project's defaults and the db-gated wrappers become active.
const AUTH_WRAPPERS: &str = "\
pub fn auth_hash_password(pw: String) -> SkyResult<SkyError, String> {\n    \
    sky_runtime::auth::auth_hash_password(pw)\n\
}\n\
pub fn auth_hash_password_cost(pw: String, cost: i64) -> SkyResult<SkyError, String> {\n    \
    sky_runtime::auth::auth_hash_password_cost(pw, cost)\n\
}\n\
pub fn auth_verify_password(pw: String, hash: String) -> SkyResult<SkyError, bool> {\n    \
    sky_runtime::auth::auth_verify_password(pw, hash)\n\
}\n\
pub fn auth_password_strength(pw: String) -> SkyResult<SkyError, String> {\n    \
    sky_runtime::auth::auth_password_strength(pw)\n\
}\n\
pub fn auth_sign_token(\n    \
    secret: sky_runtime::secret::Secret, claims: HashMap<String, String>, expiry_seconds: i64,\n\
) -> SkyResult<SkyError, String> {\n    \
    sky_runtime::auth::auth_sign_token(sky_runtime::secret::secret_reveal(secret), claims, expiry_seconds)\n\
}\n\
pub fn auth_verify_token(secret: sky_runtime::secret::Secret, token: String) -> SkyResult<SkyError, HashMap<String, String>> {\n    \
    sky_runtime::auth::auth_verify_token(sky_runtime::secret::secret_reveal(secret), token)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_register(conn: Db, email: String, password: String) -> SkyTask<i64> {\n    \
    sky_runtime::auth::auth_register(conn, email, password)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_login(conn: Db, email: String, password: String) -> SkyTask<i64> {\n    \
    sky_runtime::auth::auth_login(conn, email, password)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_set_role(conn: Db, user_id: i64, role: String) -> SkyTask<()> {\n    \
    sky_runtime::auth::auth_set_role(conn, user_id, role)\n\
}\n\
";

/// The `sky_runtime/config.rs` emitted for db-enabled programs targeting
/// `SQLite` (the default driver). Replaces the no-op M0 stub with the `SQLite`
/// type aliases + helper fns the `db.rs` module requires. Mirrors
/// `runtime/src/sky_runtime/config.rs` verbatim, keeping the
/// `#[cfg(feature = "db")]` / `#[cfg(not(feature = "db"))]` guards so a
/// non-db build (hypothetically possible via feature flag override) degrades
/// gracefully rather than failing with undefined types.
const RUNTIME_CONFIG_RS_DB_SQLITE: &str =
    include_str!("../../../runtime/src/sky_runtime/config.rs");

/// The `sky_runtime/config.rs` emitted for db-enabled programs targeting
/// Postgres (`sky.toml`'s `[database] driver = "postgres"`). Same symbol
/// surface as [`RUNTIME_CONFIG_RS_DB_SQLITE`] (`DbPool`/`DbRow`/`sky_db_url`/
/// `db_last_insert_id`/`db_format_sql`/`DB_USES_RETURNING_ID`/
/// `db_auto_id_column`), so `db.rs` is byte-identical across both driver
/// builds.
const RUNTIME_CONFIG_RS_DB_POSTGRES: &str =
    include_str!("../../../runtime/src/sky_runtime/config_postgres.rs");

/// The `Diagnostic::CompilerBug` raised when a golden anchor is absent — a
/// drifted-golden invariant violation, surfaced (SKY-I0203) instead of a silent
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
    buckets: &'p BTreeMap<RustFileId, (Vec<&'p sky_ir::EnumDef>, Vec<&'p sky_ir::Func>)>,
    file_id: &RustFileId,
) -> DResult<&'p (Vec<&'p sky_ir::EnumDef>, Vec<&'p sky_ir::Func>)> {
    buckets.get(file_id).ok_or_else(|| Diagnostic::CompilerBug {
        where_: "sky_backend_rust::project::emit_program",
        detail: "type_order/func_order references a home missing from partition_items' own \
                 buckets — internal invariant violation"
            .to_owned(),
    })
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
    const START: &str = "pub use sky_runtime::error::SkyError;";
    const END: &str = "    sky_runtime::http_client::http_parse_query(raw)\n}\n";
    let start = GOLDEN.find(START).ok_or_else(|| anchor_missing(START))?;
    let rest = GOLDEN.get(start..).ok_or_else(|| anchor_missing(START))?;
    let end_in_rest = rest.find(END).ok_or_else(|| anchor_missing(END))?;
    let end = start + end_in_rest + END.len();
    GOLDEN.get(start..end).ok_or_else(|| anchor_missing(END))
}

/// Emit the complete project for `program`.
#[allow(clippy::too_many_lines)]
pub fn emit_program(ctx: &EmitCtx, program: &Program) -> DResult<EmittedProject> {
    // Task 3 (design doc §2.2/independent-review finding): fail closed if a
    // synthesised record struct's name collides with a user enum's name, a
    // function name, or (once real `mod` declarations exist — Milestone C)
    // a `mod_ident`. Milestone A never writes more than one file, so no
    // `mod` items exist yet — an empty set is the honest state of the
    // world, not a loophole; Milestone C passes the real `mod_ident` set.
    ctx.assert_record_structs_disjoint_from_type_namespace(&BTreeSet::new())?;

    // Capacity hint only — bytes pushed are identical. `GOLDEN` (the embedded
    // reference main.rs the preamble/epilogue are cut from) is a sound floor
    // for the fixed sections; user code grows beyond it via the usual
    // doubling (efficiency-audit §4 low: this one-shot buffer started at
    // zero capacity and re-doubled through every fixed-prelude push).
    let mut out = String::with_capacity(GOLDEN.len() + 4096);
    out.push_str(&preamble()?);

    // User types, emitted from the IR — routed through `partition_items`
    // (Task 5, design doc §3.1/§5): the partition function's output is
    // proven byte-identical in ORDER and CONTENT to the direct per-module
    // walk it replaces, for every existing (single-Sky-module) golden.
    // Milestone A does not yet write multiple files — everything still
    // lands in the one growing `out: String`.
    let Partitioned {
        buckets,
        type_order,
        func_order,
    } = partition_items(program, ctx.interner);

    // Determinism fix (regression found by `parity_multimodule_adversarial_
    // edits`'s `module-added` step, closed same session as Task 5). `buckets`'
    // OWN `BTreeMap<RustFileId, _>` key order sorts `ModPath` by its derived
    // `Ord`, which compares interned `Symbol`s by their RAW `u32` id. That id
    // is NOT stable between a warm (incrementally reused) database and a cold
    // (freshly rebuilt) one for the SAME final program state — the documented
    // warm-db symbol-numbering limitation `clean_vs_incremental_parity.rs`'s
    // own top doc comment records. Iterating `buckets` directly is fine for
    // lookups / the totality proof (Task 4), but is an UNSOUND final
    // byte-emission order the moment two or more distinct real Sky-module
    // `home`s exist in one program — the `module-added` adversarial step is
    // the first scenario in the suite to exercise exactly that shape
    // (`Lib.Util` + `Lib.Extra`), and warm vs. cold interned those two
    // modules' symbols in a different relative order, so the raw map order
    // silently flipped which module's functions landed first.
    //
    // Fix: walk `type_order` (below) instead of `buckets`' own key order —
    // `partition_items` builds it by FIRST-ENCOUNTER position over
    // `program.modules[..].types`'s own vector order, which is a
    // linker-computed topological order proven warm/cold-stable by
    // `parity_multimodule_adversarial_edits` itself (that gate ran directly
    // against the pre-Task-5 code, with no `partition_items` involved at
    // all, and passed). See [`Partitioned`]'s own doc comment for the full
    // story, including why this is NOT simply an alphabetical sort
    // (`tests/golden/mm_diamond`'s `D`-before-`C`-before-`B` topological
    // order is neither symbol-id nor lexical order). A single-bucket
    // program (every existing golden pre-this-fixture) has nothing to
    // reorder, so this is a byte-identical no-op for them.
    for file_id in &type_order {
        let (enums, _) = bucket_or_bug(&buckets, file_id)?;
        for &def in enums {
            out.push_str(&emit_enum(ctx, def)?);
        }
    }
    if let Some((spine_enums, _)) = buckets.get(&RustFileId::Spine) {
        for &def in spine_enums {
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
    // #111: when the program uses Std.Auth kernels, append concrete wrappers
    // that specialise E = SkyError so call sites compile without turbofish.
    if ctx.uses_auth {
        out.push_str(AUTH_WRAPPERS);
    }
    out.push('\n');

    // User functions, emitted from the IR — routed through the SAME
    // `partition_items` result computed above, walked via `func_order` (its
    // OWN first-encounter order over `program.modules[..].funcs`, tracked
    // independently of `type_order` — see [`Partitioned`]'s doc comment for
    // why the two orders can genuinely differ). No `Spine` ordering rule is
    // needed here: `partition_items` never routes a `Func` into the `Spine`
    // bucket (only the SqlValue/SqlField enum special case does, and enums
    // and funcs are disjoint Rust namespaces — design doc §2.2), so funcs
    // land purely in `SkyModule` buckets. A byte-identical no-op reordering
    // for every existing single-Sky-module golden, where there is only one
    // bucket.
    for file_id in &func_order {
        let (_, funcs) = bucket_or_bug(&buckets, file_id)?;
        for &func in funcs {
            out.push_str(&emit_func(ctx, func)?);
        }
    }
    out.push('\n');

    out.push_str(&epilogue()?);

    // ── G3: Webview main-thread entry switch ──────────────────────────────────
    // Sky.Webview's `tao` event loop requires the process's TRUE main thread on
    // every OS (macOS: Cocoa NSApplication hard-requires it; Windows: expected;
    // Linux/GTK: safe on main thread). The standard entry uses `block_on` (which
    // spawns a detached OS thread); Webview MUST use `block_on_current_thread`
    // (a current-thread tokio runtime polled inline on the calling — main —
    // thread).
    //
    // Implementation: anchor-asserted `replacen` that emits `CompilerBug` and
    // aborts if the anchor is absent (fail-loud, never a silent no-op). A silent
    // no-op here would ship a well-typed Webview app that silently runs the event
    // loop off the main thread, crashing at first paint on macOS.
    if ctx.uses_webview {
        const BLOCK_ON_ANCHOR: &str = "block_on(sky_main())";
        const BLOCK_ON_THREAD_REPLACEMENT: &str = "block_on_current_thread(sky_main())";
        let replaced = out.replacen(BLOCK_ON_ANCHOR, BLOCK_ON_THREAD_REPLACEMENT, 1);
        if replaced == out {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_backend_rust::project::emit_program::G3_block_on",
                detail: format!(
                    "G3 webview entry-switch: anchor {BLOCK_ON_ANCHOR:?} not found in \
                     emitted output — epilogue golden has drifted; Sky.Webview REQUIRES \
                     block_on_current_thread (tao/Cocoa NSApplication mandates the \
                     process main thread on macOS; omitting the switch is a runtime crash)"
                ),
            });
        }
        out = replaced;
    }

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
        let cfg = match ctx.db_driver {
            crate::DbDriver::Sqlite => RUNTIME_CONFIG_RS_DB_SQLITE,
            crate::DbDriver::Postgres => RUNTIME_CONFIG_RS_DB_POSTGRES,
        };
        (db_cargo_toml(ctx.db_driver)?, cfg.to_owned())
    } else {
        (CARGO_TOML.to_owned(), RUNTIME_CONFIG_RS.to_owned())
    };
    // Apply server manifest extension on top of whichever base was chosen above.
    // Phase-1b: Live also needs axum + tower-http (the live runtime uses axum
    // internally).  Apply server_cargo_toml for both `uses_server`, `uses_live`,
    // and `uses_webview` (Webview's real backend imports from the live module,
    // which uses axum; the function is idempotent when multiple flags are set).
    let cargo_toml = if ctx.uses_server || ctx.uses_live || ctx.uses_webview {
        server_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // Phase-1b: when the program uses Live, add "live" to the default features.
    // The base manifest already declares `live = []` as a non-default feature;
    // we just need to promote it to the `default` list so the compiled binary
    // includes the `live` module.
    // Phase-1d: Webview's real backend imports `sky_runtime::live::dispatch`
    // (for `build_index`) and `sky_runtime::html::render_html` — both gated
    // behind the `live` feature. Force-promote `live` for Webview as well.
    let cargo_toml = if ctx.uses_live || ctx.uses_webview {
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
    // Phase-1d: when the program uses Webview, add "webview" to the default
    // features and inject the wry + tao deps required by the real native-window
    // backend. The base manifest declares `webview = []` as a non-default feature;
    // this function promotes it, wires it to wry + tao, and adds those deps.
    let cargo_toml = if ctx.uses_webview {
        webview_cargo_toml(&cargo_toml)?
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
        // `tea` must be included whenever user code uses TEA kernels directly,
        // OR whenever `uses_server` is true — because `http_stream.rs` (included
        // via SERVER_APPEND) uses `SkySub` from `tea.rs` via `use super::*;`.
        // Guarded as a union so a program using both emits `pub mod tea;` exactly
        // once (E0428). Transitive-closure invariant: any module depended on by an
        // included module MUST itself be included (same rule as http_header).
        if ctx.uses_tea || ctx.uses_server {
            mod_rs.push_str(RUNTIME_MOD_RS_TEA_APPEND);
        }
        if ctx.uses_server {
            mod_rs.push_str(RUNTIME_MOD_RS_SERVER_APPEND);
        }
        // #111: Std.Auth — append auth module when any Auth kernel is used.
        if ctx.uses_auth {
            mod_rs.push_str(RUNTIME_MOD_RS_AUTH_APPEND);
        }
        // `http_header` is now part of the M0 BASE `mod.rs` (#33 §6.1 made the
        // base `http_client` module depend on it), so it needs no conditional
        // append here — see the retired-append note at the top of this file.
        // #47: Std.Css leaf security kernels — declared for any render-capable
        // program (`uses_ui`, whose html/ui/live runtime modules import
        // `css_safety`) OR a pure-`Std.Css` program (`uses_css`, no render
        // kernel). Pushed BEFORE the UI append because `html.rs` /
        // `ui/render.rs` / `live/style_inject.rs` import `css_safety` at the
        // top level — it must be declared first. The single guard de-duplicates:
        // a program using both emits `pub mod css_safety;` exactly once (E0428).
        // Transitive closure: the `tui` runtime module unconditionally imports
        // `super::ui` (tui/app.rs, tui/layout.rs) and `super::html`
        // (tui/focus.rs), so a String-view Tui program (`uses_tui` without
        // `uses_ui`) still needs the css/ui/html appends — same invariant as
        // the `http_header` leaf above.
        if ctx.uses_ui || ctx.uses_css || ctx.uses_tui {
            mod_rs.push_str(RUNTIME_MOD_RS_CSS_APPEND);
        }
        // M7: Std.Ui / Std.Html render kernels (+ Tui transitive dep).
        if ctx.uses_ui || ctx.uses_tui {
            mod_rs.push_str(RUNTIME_MOD_RS_UI_APPEND);
        }
        // Phase-1b: Std.Live / Sky.Live app-entry kernels.
        if ctx.uses_live || ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_LIVE_APPEND);
        }
        // Phase-1c: Std.Tui / Sky.Tui app-entry kernels.
        if ctx.uses_tui {
            mod_rs.push_str(RUNTIME_MOD_RS_TUI_APPEND);
        }
        // Phase-1d: Std.Webview / Sky.Webview app-entry kernel.
        if ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_WEBVIEW_APPEND);
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
///    `skyc` itself exited 0 — an exit-0-then-cargo-fail SEAL violation.
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
    let sqlx_line = format!(
        "{} = {{ version = \"{}\", features = [\"runtime-tokio-rustls\", {sqlx_features}] }}\n\n",
        crate_specs::SQLX.name,
        crate_specs::SQLX.version,
    );

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
    let step2 = step1.replacen(&tokio_time, &tokio_net_sync, 1);
    if step2 == step1 {
        return Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::server_cargo_toml",
            detail: format!("Cargo.toml anchor {tokio_time:?} not found — golden drifted"),
        });
    }

    // Step 3 — append axum + tower-http before `[profile.dev]`.
    let anchor_pos = step2
        .find(PROFILE_ANCHOR)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::server_cargo_toml",
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
    // The `live` runtime mainline uses `tokio::signal` + `tokio::process`; the base
    // golden emits `net`+`sync` for the HTTP server, so add the two missing features.
    const TOKIO_NET_SYNC_FEATURES: &str = "\"time\", \"net\", \"sync\"]";
    const TOKIO_LIVE_FEATURES: &str = "\"time\", \"net\", \"sync\", \"signal\", \"process\"]";
    // Transitive-closure invariant (#137): the runtime's `live/store.rs` defines
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
    let live_deps = format!(
        "{} = \"{}\"\n{} = \"{}\"\n{} = \"{}\"\n\n",
        crate_specs::ASYNC_TRAIT.name,
        crate_specs::ASYNC_TRAIT.version,
        crate_specs::SERDE_URLENCODED.name,
        crate_specs::SERDE_URLENCODED.version,
        crate_specs::LIBC.name,
        crate_specs::LIBC.version,
    );

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
        let replaced = step1.replacen(&tokio_time_only, &tokio_time_sync, 1);
        if replaced == step1 {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_backend_rust::project::tui_cargo_toml",
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
            where_: "sky_backend_rust::project::tui_cargo_toml",
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
            where_: "sky_backend_rust::project::webview_cargo_toml",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::project::webview_cargo_toml",
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
            where_: "sky_backend_rust::project::webview_cargo_toml",
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
            where_: "sky_backend_rust::project::webview_cargo_toml",
            detail: format!("Cargo.toml anchor {PROFILE_ANCHOR:?} not found — golden drifted"),
        })?;
    let mut result = String::with_capacity(step2.len() + webview_native_deps.len());
    result.push_str(step2.get(..anchor_pos).unwrap_or(""));
    result.push_str(&webview_native_deps);
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
            Self::SqlNull(inner) => sky_runtime::db::SqlParam::Null(Box::new(inner.into_sql_param())),
        }}
    }}
}}
/// Allow `SqlParam::from(sql_value)` so the emitter can use the same
/// `sky_runtime::db::SqlParam::from` projection for ALL element types in
/// the polymorphic `Db.exec`/`query` params list (`List a` where `a` may
/// be `String`, `Int`, `Float`, `Bool`, or `SqlValue`).
impl From<{sv}> for sky_runtime::db::SqlParam {{
    fn from(v: {sv}) -> Self {{
        v.into_sql_param()
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
    use super::{
        CARGO_TOML, RUNTIME_CONFIG_RS_DB_POSTGRES, RUNTIME_CONFIG_RS_DB_SQLITE,
        RUNTIME_MOD_RS_LIVE_APPEND, db_cargo_toml, live_cargo_toml, server_cargo_toml,
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

    // ── #137 seal tests: Db+Live closure ─────────────────────────────────────

    /// `live_cargo_toml` on a DB+Server base manifest must extend the sqlx dep
    /// with the `"postgres"` feature.
    ///
    /// Root cause: `live/store.rs`'s `PostgresStore` references `sqlx::PgPool`
    /// gated on `#[cfg(feature = "db")]`.  The runtime's own `Cargo.toml` has
    /// `["runtime-tokio-rustls", "sqlite", "postgres"]`; the emitted project must
    /// match.  Without this, a Db+Live program passes `skyc` then fails `cargo
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
    /// (found by independent review, 2026-07-10) because the always-emitted
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
    /// Root cause: `db.rs` has `#[cfg(feature = "live")] impl SkyRow for
    /// super::LiveReq` — `super::LiveReq` means `sky_runtime::LiveReq`.  The
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

    /// `RUNTIME_MOD_RS_LIVE_APPEND` must re-export `cmd_publish` and
    /// `cmd_publish_no_echo` so that emitted call sites (`cmd_publish(topic,
    /// payload)`) resolve.  Without this the emitted project fails with E0425
    /// (`cannot find function cmd_publish`) — a seal violation.
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
    }
}
