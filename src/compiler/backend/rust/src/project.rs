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

use ipe_backend::{EmittedProject, RelPath};
use ipe_diagnostics::{DResult, Diagnostic};
use ipe_ir::{ModPath, Program};

use crate::EmitCtx;
use crate::crate_specs;
use crate::emit_expr::emit_func;
use crate::emit_types::{emit_enum, emit_record_struct};
use crate::preamble::{epilogue, preamble};
use crate::rust_file;
use crate::rust_file::{Partitioned, RustFileId, partition_items};

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
const RUNTIME_CONFIG_RS: &str = include_str!("../../../../../tests/golden/basics/ipe_runtime/config.rs");

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

// ── Sky.Http.Server ──────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses
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
// `http_header.rs` (a dependency-free leaf exposing `canonical_header`) is part
// of the base `mod.rs` (`tests/golden/basics/ipe_runtime/mod.rs`), because the
// outbound `http_client.rs` response path always calls it. It must NOT be
// conditionally appended here — a conditional `pub mod http_header;` would
// duplicate the base declaration (E0428) for server/live programs.

// ── Std.Auth ──────────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Std.Auth
/// kernels (`Auth.hashPassword` / `verifyPassword` / `signToken` /
/// `verifyToken` / `register` / `login` / `setRole` etc.).
///
/// `auth.rs` requires `bcrypt` (password hashing) and `jsonwebtoken` (JWT
/// signing/verification); both are unconditional deps in the generated
/// project's `Cargo.toml` (included in the `crypto` and `json` default
/// features), so no manifest surgery is needed — only a `mod.rs` declaration.
const RUNTIME_MOD_RS_AUTH_APPEND: &str = "pub mod auth;\npub use auth::*;\n";

// ── Sky.Core.WebSocket — outbound WebSocket client ──────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses outbound
/// `Sky.Core.WebSocket` client kernels.
///
/// `ws_client.rs` is gated by the `websocket_client` Cargo feature in the
/// runtime source; this addition wires it into the module namespace so the
/// generated `main.rs` can call `web_socket_connect` / `web_socket_send` / … and
/// the `sub_subscribe_ws_*` subscription fns via `pub use ipe_runtime::*`.
///
/// `ssrf.rs` (`ws_client`'s SSRF validators) is already part of the base
/// `mod.rs` (the always-present `http_client` module also needs it), so no
/// `ssrf` append is required here. `tea.rs` (whose `SkySub<M>` the
/// `sub_subscribe_ws_*` fns return) is force-appended alongside this in
/// [`assemble_project_files`], mirroring the `uses_server` rule.
const RUNTIME_MOD_RS_WEBSOCKET_APPEND: &str = "pub mod ws_client;\npub use ws_client::*;\n";
// ── Std.Email ───────────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses the `Std.Email`
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

// ── Std.Ui / Std.Html ───────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Std.Ui /
/// Std.Html render kernels.
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
/// css out lets a pure-`Std.Css` program (no render kernel ⇒ no `uses_ui`) still
/// get the css declarations via `uses_css` alone.
const RUNTIME_MOD_RS_UI_APPEND: &str = "pub mod html;\npub use html::*;\npub mod ui;\n";

/// Lines appended to `ipe_runtime/mod.rs` when the program uses the `Std.Css`
/// leaf security kernels (`Sky.Core.CssSafety.safeValue` / `safePropName` /
/// `safeSelector` / `stripStyleClose`) — OR any `Std.Ui` / `Std.Html`
/// render kernel (whose runtime modules import `css_safety` at the top level).
///
/// `css_safety.rs` is a dependency-free, audited leaf; `css.rs` (the four
/// `Std.Css` leaf kernels — `safe_value` / `safe_prop_name` / `safe_selector` /
/// `strip_style_close_kernel`) depends only on `css_safety`, and is glob-re-
/// exported (`pub use css::*;`) so the emitted `pub use ipe_runtime::*;`
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

// ── Std.Tui / Sky.Tui ───────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Std.Tui /
/// Sky.Tui app-entry kernels.
///
/// Both `tui/app.rs` and `tui/layout.rs` (and their dependencies `cell.rs`,
/// `diff.rs`, `focus.rs`, `key.rs`) are gated by the `tui` Cargo feature in the
/// runtime source.  This addition wires `tui::tui_app` and `tui::tui_app_ui`
/// into the module namespace so the generated `main.rs` can call them via
/// `ipe_runtime::tui::tui_app_ui`.
///
/// The `ui` module must also be loaded (tui/layout.rs imports `super::ui::Element`)
/// — but `uses_ui` is set whenever `uses_tui` is set (a Tui app always references
/// Std.Ui Element/attribute kernels), so `RUNTIME_MOD_RS_UI_APPEND` is already
/// appended by the time this addition fires.
const RUNTIME_MOD_RS_TUI_APPEND: &str = "#[cfg(feature = \"tui\")]\npub mod tui;\n\
     #[cfg(feature = \"tui\")]\npub use tui::{tui_app, tui_app_ui};\n";

// ── Std.Webview / Sky.Webview ───────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Std.Webview /
/// Sky.Webview app-entry kernels.
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

// ── Std.Live / Sky.Live ─────────────────────────────────────────────────────

/// Lines appended to `ipe_runtime/mod.rs` when the program uses Std.Live /
/// Sky.Live app-entry kernels.
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
/// runtime's `db.rs` module contains a `#[cfg(feature = "live")] impl SkyRow for
/// super::LiveReq` block — `super::LiveReq` means `ipe_runtime::LiveReq`. In the
/// real runtime source `mod.rs` uses `pub use live::*;` which surfaces `LiveReq`
/// (via `live/mod.rs`'s own `pub use req::*;`), but the emitted project uses a
/// selective export list.  Without `LiveReq` here, any program that uses BOTH Db
/// and Live kernels fails with E0412 (`LiveReq in super` not found) at
/// `db.rs:impl SkyRow for super::LiveReq`.
///
/// The `route` sub-module is referenced by path (`ipe_runtime::live::route::Route`)
/// not via `pub use live::*;` (to avoid surfacing the internal `store` / `req`
/// internals in the top-level namespace).
const RUNTIME_MOD_RS_LIVE_APPEND: &str = "#[cfg(feature = \"live\")]\npub mod live;\n\
     #[cfg(feature = \"live\")]\npub use live::{live_app, live_app_routed, live_render_static, sub_subscribe_topic, cmd_publish, cmd_publish_no_echo, pubsub_publish, pubsub_publish_no_echo, LiveReq};\n";

/// The `SkyCmd<M>` and `SkySub<M>` project-level type aliases emitted when the
/// program uses TEA kernels. Placed immediately after `runtime_bindings()` (the
/// block that also contains `SkyTask<A>` and `Decoder<T>`).
const TEA_TYPE_ALIASES: &str = "pub type SkyCmd<M> = ipe_runtime::tea::SkyCmd<M>;\n\
     pub type SkySub<M> = ipe_runtime::tea::SkySub<M>;\n";

// ── Std.Auth — concrete wrappers emitted when uses_auth is true ────────

/// Concrete wrappers appended to `main.rs` when the program uses Std.Auth
/// kernels.  Each wrapper specialises the generic `E` type parameter to
/// `SkyError` so call sites in user function bodies compile without requiring
/// a turbofish annotation.
///
/// `auth_sign_token` / `auth_verify_token` take a Sky-typed
/// `ipe_runtime::secret::Secret` (not `String`) at this boundary — "secrets
/// are typed, never `fmt`-stringified" (`PRINCIPLES.md`). The wrapper reveals
/// it via `ipe_runtime::secret::secret_reveal` immediately before delegating
/// to the runtime's `String`-typed `ipe_runtime::auth::{auth_sign_token,
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
    ipe_runtime::auth::auth_hash_password(pw)\n\
}\n\
pub fn auth_hash_password_cost(pw: String, cost: i64) -> SkyResult<SkyError, String> {\n    \
    ipe_runtime::auth::auth_hash_password_cost(pw, cost)\n\
}\n\
pub fn auth_verify_password(pw: String, hash: String) -> SkyResult<SkyError, bool> {\n    \
    ipe_runtime::auth::auth_verify_password(pw, hash)\n\
}\n\
pub fn auth_password_strength(pw: String) -> SkyResult<SkyError, String> {\n    \
    ipe_runtime::auth::auth_password_strength(pw)\n\
}\n\
pub fn auth_sign_token(\n    \
    secret: ipe_runtime::secret::Secret, claims: HashMap<String, String>, expiry_seconds: i64,\n\
) -> SkyResult<SkyError, String> {\n    \
    ipe_runtime::auth::auth_sign_token(ipe_runtime::secret::secret_reveal(secret), claims, expiry_seconds)\n\
}\n\
pub fn auth_verify_token(secret: ipe_runtime::secret::Secret, token: String) -> SkyResult<SkyError, HashMap<String, String>> {\n    \
    ipe_runtime::auth::auth_verify_token(ipe_runtime::secret::secret_reveal(secret), token)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_register(conn: Db, email: String, password: String) -> SkyTask<i64> {\n    \
    ipe_runtime::auth::auth_register(conn, email, password)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_login(conn: Db, email: String, password: String) -> SkyTask<i64> {\n    \
    ipe_runtime::auth::auth_login(conn, email, password)\n\
}\n\
#[cfg(feature = \"db\")]\n\
pub fn auth_set_role(conn: Db, user_id: i64, role: String) -> SkyTask<()> {\n    \
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
/// Postgres (`sky.toml`'s `[database] driver = "postgres"`). Same symbol
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
/// These bindings (`SkyError`, the `log_*` / `system_*` / `time_*` / … wrappers)
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
    const START: &str = "pub use ipe_runtime::error::SkyError;";
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
    // number of DISTINCT `RustFileId::SkyModule` buckets — NEVER counting the
    // always-possible `Spine` bucket (§3.3: "counts `SkyModule` buckets only,
    // never `Spine`") — is the trigger for the real per-module split:
    //   • 0 or 1 distinct SkyModule bucket → the Spine-collapse invariant
    //     fires and we emit today's byte-identical single `src/main.rs`.
    //   • 2+ → the real split materialises (`emit_spine` + one
    //     `emit_module_file` per bucket + the `main.rs` barrel lines).
    let partition = partition_items(program, ctx.interner);

    // The DISTINCT `SkyModule` homes, in first-encounter (linker/topological)
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

    // (design doc §2.2): fail closed if a
    // synthesised record struct's name collides with a user enum's name, a
    // function name, or a `mod_ident`. In the single-file collapse case no
    // `mod` declarations are written, so the honest set is empty; in the real
    // split every `SkyModule` bucket contributes its `mod_ident` (§2.1.1's new
    // namespace, whose intra-set uniqueness `assert_mod_idents_unique` already
    // guarantees — this check is the DISJOINTNESS obligation against the
    // record-struct namespace).
    let mod_idents: BTreeSet<String> = if module_homes.len() >= 2 {
        module_homes
            .iter()
            .filter_map(|id| match id {
                RustFileId::SkyModule(home) => {
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
    // one `src/sky_mods/<ident>.rs` per module). The manifest + runtime-module
    // files below are file-count-agnostic and shared by both branches.
    let mut rust_sources: Vec<(RelPath, String)> = Vec::new();

    if module_homes.len() >= 2 {
        // ── The real per-Sky-module split (§2.1/§3.3) ────────────────────────
        // `main.rs` = the Spine tier (preamble, SqlValue/SqlField enums,
        // record structs, DB-projection impls, kernel-wrapper prelude, epilogue,
        // `fn main()`) + the flat glob barrel that re-exports every module's
        // items at the crate root.
        let mut main_rs = emit_spine(ctx, program)?;
        // Barrel lines (§2.1), one pair per distinct SkyModule home, in the
        // deterministic first-encounter order computed above:
        //   #[path = "sky_mods/sky_mod_<home>.rs"]
        //   mod sky_mod_<home>;
        //   pub(crate) use sky_mod_<home>::*;
        // The `#[path]` attribute is load-bearing: `main.rs` is the crate root,
        // so a BARE `mod sky_mod_<home>;` would resolve to a crate-root sibling
        // `src/sky_mod_<home>.rs`, NOT the `src/sky_mods/<ident>.rs` file this
        // design places (§2.1). `#[path]` is resolved relative to the declaring
        // file's directory (`src/`), so it points the module at the real file
        // under `sky_mods/` — closing an E0583 "file not found for module"
        // exit-0-then-cargo-fail (THE SEAL) that a bare `mod` decl would ship.
        // Because every user name is already globally unique (§1.3) and this
        // re-exports every module at the crate root, each per-module file's
        // `use crate::*;` sees every Spine item and every other module's item.
        main_rs.push('\n');
        for id in &module_homes {
            let RustFileId::SkyModule(home) = id else {
                continue;
            };
            let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
            // Built via `push_str` fragments rather than one `push_str(&format!)`
            // to satisfy `clippy::format_push_string` (denied via pedantic) —
            // no intermediate allocation, same bytes.
            main_rs.push_str("#[path = \"sky_mods/");
            main_rs.push_str(&ident);
            main_rs.push_str(".rs\"]\nmod ");
            main_rs.push_str(&ident);
            main_rs.push_str(";\npub(crate) use ");
            main_rs.push_str(&ident);
            main_rs.push_str("::*;\n");
        }
        rust_sources.push((RelPath::new("src/main.rs")?, main_rs));

        // One `src/sky_mods/<ident>.rs` per module, carrying ONLY that home's
        // `pub(crate)` items behind a `use crate::*;` glob header.
        for id in &module_homes {
            let RustFileId::SkyModule(home) = id else {
                continue;
            };
            let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
            let file = emit_module_file(ctx, program, id)?;
            rust_sources.push((RelPath::new(format!("src/sky_mods/{ident}.rs"))?, file));
        }
    } else {
        // ── The Spine-collapse invariant (§3.3) ──────────────────────────────
        // Exactly ONE distinct `SkyModule` bucket (or none): inline that one
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
        // enums.  The Db call sites need to project Sky ADT values to the
        // runtime's concrete `SqlParam` / `Option<SqlParam>`.
        if ctx.uses_db {
            out.push_str(&emit_db_projection_impls(ctx)?);
        }

        out.push('\n');

        // Fixed kernel-wrapper prelude (SkyError, SkyTask<A>, Decoder<T>, …).
        out.push_str(runtime_bindings()?);

        // TEA kernels → the SkyCmd<M> / SkySub<M> type aliases.
        if ctx.uses_tea {
            out.push_str(TEA_TYPE_ALIASES);
        }
        // Std.Auth kernels → concrete E = SkyError wrappers.
        if ctx.uses_auth {
            out.push_str(AUTH_WRAPPERS);
        }
        out.push('\n');

        // User functions, walked via `func_order` (its OWN first-encounter
        // order over `program.modules[..].funcs`). `partition_items` never
        // routes a `Func` into `Spine`, so funcs land purely in `SkyModule`
        // buckets.
        for file_id in func_order {
            let (_, funcs) = bucket_or_bug(buckets, file_id)?;
            for &func in funcs {
                out.push_str(&emit_func(ctx, func)?);
            }
        }
        out.push('\n');

        out.push_str(&epilogue()?);

        // ── G3: Webview main-thread entry switch ──────────────────────────────
        // Sky.Webview's `tao` event loop requires the process's TRUE main
        // thread on every OS. The standard entry uses `block_on`; Webview MUST
        // use `block_on_current_thread`. (In the real split, `emit_spine`
        // performs this same switch on its own — the anchor lives in the
        // epilogue, which is Spine-only.)
        if ctx.uses_webview {
            const BLOCK_ON_ANCHOR: &str = "block_on(sky_main())";
            const BLOCK_ON_THREAD_REPLACEMENT: &str = "block_on_current_thread(sky_main())";
            let replaced = out.replacen(BLOCK_ON_ANCHOR, BLOCK_ON_THREAD_REPLACEMENT, 1);
            if replaced == out {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::project::emit_program::G3_block_on",
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

        rust_sources.push((RelPath::new("src/main.rs")?, out));
    }

    assemble_project_files(ctx, rust_sources)
}

/// Assemble the final [`EmittedProject`] from the already-rendered Rust source
/// files (`src/main.rs` plus, in the real split, each `src/sky_mods/<ident>.rs`)
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
fn assemble_project_files(
    ctx: &EmitCtx,
    rust_sources: Vec<(RelPath, String)>,
) -> DResult<EmittedProject> {
    // ── Manifest + runtime module files ──────────────────────────────────────
    // The driver (skyc) first copies the full runtime source tree into
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
    // Sky.Core.WebSocket client: promote the `websocket_client` feature +
    // add tokio-tungstenite + tokio `"sync"`. Applied last; idempotent on the
    // tokio `"sync"` step so it composes with any prior server/live/tui surgery.
    let cargo_toml = if ctx.uses_websocket {
        websocket_cargo_toml(&cargo_toml)?
    } else {
        cargo_toml
    };
    // Std.Email: `email.rs` needs the `lettre` crate for the SMTP transport
    // (every other crate it uses — `base64` / `hmac` / `sha2` / `serde_json` /
    // `reqwest` / `url` — is already an unconditional base-manifest dep). Add
    // `lettre` only when the program uses `Email.send`.
    let cargo_toml = if ctx.uses_email {
        email_cargo_toml(&cargo_toml)?
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
        // `tea` must be included whenever user code uses TEA kernels directly,
        // OR whenever `uses_server` is true — because `http_stream.rs` (included
        // via SERVER_APPEND) uses `SkySub` from `tea.rs` via `use super::*;`.
        // Guarded as a union so a program using both emits `pub mod tea;` exactly
        // once (E0428). Transitive-closure invariant: any module depended on by an
        // included module MUST itself be included (same rule as http_header).
        // `uses_websocket` also forces `tea`: `ws_client.rs`'s `sub_subscribe_ws_*`
        // fns return `SkySub<M>` (from `tea.rs`) via `use super::*`, so a
        // connect/send-only program (no explicit Sub kernel ⇒ no `uses_tea`) still
        // needs `tea` declared — same transitive-closure rule as `uses_server`.
        if ctx.uses_tea || ctx.uses_server || ctx.uses_websocket {
            mod_rs.push_str(RUNTIME_MOD_RS_TEA_APPEND);
        }
        if ctx.uses_server {
            mod_rs.push_str(RUNTIME_MOD_RS_SERVER_APPEND);
        }
        // Sky.Core.WebSocket client — declare `ws_client` (its `ssrf` dep is
        // already in the base, its `tea` dep forced above).
        if ctx.uses_websocket {
            mod_rs.push_str(RUNTIME_MOD_RS_WEBSOCKET_APPEND);
        }
        // Std.Auth — append auth module when any Auth kernel is used.
        if ctx.uses_auth {
            mod_rs.push_str(RUNTIME_MOD_RS_AUTH_APPEND);
        }
        // Std.Email — append email module when `Email.send` is used.
        if ctx.uses_email {
            mod_rs.push_str(RUNTIME_MOD_RS_EMAIL_APPEND);
        }
        // `http_header` is part of the base `mod.rs` (the base `http_client`
        // module depends on it), so it needs no conditional append here — see
        // the note at the top of this file.
        // Std.Css leaf security kernels — declared for any render-capable
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
        // `live/mod.rs` unconditionally does
        // `pub use crate::ipe_runtime::html::*` and `html.rs` imports
        // `css_safety`, so a `uses_live`-only program (e.g. PubSub-only, no
        // Std.Ui kernels) still needs `css_safety` and `html` declared.
        if ctx.uses_ui || ctx.uses_css || ctx.uses_tui || ctx.uses_live || ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_CSS_APPEND);
        }
        // Std.Ui / Std.Html render kernels (+ Tui + Live transitive dep).
        // `live/mod.rs` unconditionally re-exports `crate::ipe_runtime::html::*`;
        // `live/style_inject.rs` imports `super::html` — so `html` must be
        // declared whenever live is enabled, even without explicit Std.Ui use.
        if ctx.uses_ui || ctx.uses_tui || ctx.uses_live || ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_UI_APPEND);
        }
        // Std.Live / Sky.Live app-entry kernels.
        if ctx.uses_live || ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_LIVE_APPEND);
        }
        // Std.Tui / Sky.Tui app-entry kernels.
        if ctx.uses_tui {
            mod_rs.push_str(RUNTIME_MOD_RS_TUI_APPEND);
        }
        // Std.Webview / Sky.Webview app-entry kernel.
        if ctx.uses_webview {
            mod_rs.push_str(RUNTIME_MOD_RS_WEBVIEW_APPEND);
        }
        mod_rs
    };

    let mut files = BTreeMap::new();
    // The emitted Rust source files: `src/main.rs` always, plus one
    // `src/sky_mods/<ident>.rs` per module in the real-split case. In the
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
    Ok(EmittedProject { files, cargo_toml })
}

/// Render the `Spine` tier's text for `program` — everything that is
/// program-wide rather than Sky-module-owned (design doc §2.1/§2.3):
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
    let mut out = String::with_capacity(GOLDEN.len() + 4096);
    out.push_str(&preamble()?);

    let Partitioned { buckets, .. } = partition_items(program, ctx.interner);

    // The Spine bucket's `SqlValue`/`SqlField` enums, in insertion order —
    // rendered where the user types would sit in the single-file layout, i.e.
    // immediately before the record structs (§2.2's ordering rule). No
    // `SkyModule` bucket enums are emitted here — those are `emit_module_file`.
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

    out.push('\n');

    out.push_str(runtime_bindings()?);
    if ctx.uses_tea {
        out.push_str(TEA_TYPE_ALIASES);
    }
    if ctx.uses_auth {
        out.push_str(AUTH_WRAPPERS);
    }
    out.push('\n');

    // The spine carries NO user functions — they are `emit_module_file`'s.
    // The blank line the single-file layout emits between the functions and
    // the epilogue is preserved so the spine's fixed-section spacing matches.
    out.push('\n');

    out.push_str(&epilogue()?);

    // ── G3: Webview main-thread entry switch ──────────────────────────────
    // The `block_on(sky_main())` anchor lives in the epilogue, which is
    // Spine-only — so under the split this scan sees a strictly smaller
    // haystack than the whole concatenated `main.rs` (design doc §2.3).
    if ctx.uses_webview {
        const BLOCK_ON_ANCHOR: &str = "block_on(sky_main())";
        const BLOCK_ON_THREAD_REPLACEMENT: &str = "block_on_current_thread(sky_main())";
        let replaced = out.replacen(BLOCK_ON_ANCHOR, BLOCK_ON_THREAD_REPLACEMENT, 1);
        if replaced == out {
            return Err(Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::emit_spine::G3_block_on",
                detail: format!(
                    "G3 webview entry-switch: anchor {BLOCK_ON_ANCHOR:?} not found in \
                     emitted spine — epilogue golden has drifted; Sky.Webview REQUIRES \
                     block_on_current_thread"
                ),
            });
        }
        out = replaced;
    }

    Ok(out)
}

/// Render the `SkyModule(home)` file's text for one Sky module's OWN
/// declarations (design doc §2.1): ONLY that `home`'s `EnumDef`s + `Func`s,
/// each `pub(crate)`-visible (not the bare `pub` the single-file layout uses,
/// since these now live inside a `mod` block), opening with the flat-barrel
/// `use crate::*;` glob so every `Spine`/other-module item is in scope.
///
/// A `home` with no items in `program` (never the real driver path — every
/// `SkyModule` file materialises FROM a non-empty bucket) yields just the
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
/// `spine_text` is [`emit_spine`]'s output; `module_texts` maps each Sky-module
/// `home` to its [`emit_module_file`] output. This function performs ONLY the
/// file-count-dependent assembly the single-file [`emit_program`] path also
/// does in its `>= 2` branch — computing the deterministic first-encounter
/// module order, the fail-closed `mod_ident` uniqueness gate, the record-struct
/// disjointness gate, the `main.rs` barrel lines, and the `src/sky_mods/*.rs`
/// file list — then delegates the file-count-AGNOSTIC manifest/runtime block to
/// the shared [`assemble_project_files`]. It never re-renders any user item;
/// the texts are taken verbatim, so the salsa `emit_manifest` query's output is
/// byte-identical to `emit_program`'s split output for the same program.
///
/// PRECONDITION (`>= 2` distinct `SkyModule` homes): this is the real-split
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

    // The distinct `SkyModule` homes in first-encounter (linker/topological)
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

    // Fail closed if a synthesised record struct's name collides with a
    // `mod_ident` (every SkyModule home contributes its ident in the split).
    let mod_idents: BTreeSet<String> = module_homes
        .iter()
        .filter_map(|id| match id {
            RustFileId::SkyModule(home) => Some(rust_file::resolve_mod_ident(home, ctx.interner)),
            RustFileId::Spine => None,
        })
        .collect::<DResult<BTreeSet<String>>>()?;
    ctx.assert_record_structs_disjoint_from_type_namespace(&mod_idents)?;

    let mut rust_sources: Vec<(RelPath, String)> = Vec::new();

    // `main.rs` = the given spine text + the flat glob barrel, one pair per
    // distinct SkyModule home in first-encounter order (byte-identical to
    // `emit_program`'s split branch).
    let mut main_rs = spine_text.to_owned();
    main_rs.push('\n');
    for id in &module_homes {
        let RustFileId::SkyModule(home) = id else {
            continue;
        };
        let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
        main_rs.push_str("#[path = \"sky_mods/");
        main_rs.push_str(&ident);
        main_rs.push_str(".rs\"]\nmod ");
        main_rs.push_str(&ident);
        main_rs.push_str(";\npub(crate) use ");
        main_rs.push_str(&ident);
        main_rs.push_str("::*;\n");
    }
    rust_sources.push((RelPath::new("src/main.rs")?, main_rs));

    // One `src/sky_mods/<ident>.rs` per module, its text taken verbatim from
    // the demanded per-file query output.
    for id in &module_homes {
        let RustFileId::SkyModule(home) = id else {
            continue;
        };
        let ident = rust_file::resolve_mod_ident(home, ctx.interner)?;
        let text = module_texts
            .get(home)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::project::assemble_split_manifest",
                detail: format!(
                    "no rendered text supplied for SkyModule home ident {ident:?} — \
                 emit_manifest must demand emit_rust_file for every home in \
                 program_rust_file_ids"
                ),
            })?;
        rust_sources.push((
            RelPath::new(format!("src/sky_mods/{ident}.rs"))?,
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
/// an enum's trailing `impl … SkyStringify` block carries no `pub`, and a
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
/// `email.rs` (the vendored `Std.Email` runtime module) needs `lettre` for the
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

/// Emit the `into_sql_param` impl for `SqlValue` and `into_field_param` impl
/// for `SqlField`.
///
/// These are fixed-shape impls — the variant names are always the same Sky
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
    /// Root cause: `db.rs` has `#[cfg(feature = "live")] impl SkyRow for
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
}
