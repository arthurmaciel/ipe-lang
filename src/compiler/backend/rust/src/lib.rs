#![forbid(unsafe_code)]
//! The Rust backend for the Ipê compiler.
//!
//! Consumes the backend-agnostic typed [`ipe_ir::Program`] and emits a Rust
//! Cargo project. The crate is split into the fixed templates emitted for every
//! program ([`preamble`] / [`epilogue`] / the kernel-wrapper prelude in
//! [`project`]) and the genuinely type-directed emission of the user's types
//! ([`emit_types`]) and functions ([`emit_expr`]). [`naming`] holds the
//! Ipê → Rust identifier rules.
//!
//! The single correctness gate is byte-equality against the golden program
//! (`tests/golden/basics/main.rs`).
//!
//! The [`ipe_ir`] boundary carries [`ipe_intern::Symbol`]s, not strings, so the
//! backend resolves them through the [`ipe_intern::Interner`] it is constructed
//! with. The [`ipe_backend::Backend`] trait stays string-free.

mod crate_specs;
mod doc;
mod emit_console;
mod emit_doc;
mod emit_expr;
mod emit_model_gate;
mod emit_model_schema;
mod emit_tui;
mod emit_types;
mod emit_web;
mod emit_webview;
mod naming;
mod preamble;
mod project;
mod render;
mod rust_file;
pub mod static_build;

use std::collections::{BTreeMap, BTreeSet};

use ipe_backend::{Backend, EmittedProject};
use ipe_diagnostics::{DResult, Diagnostic, NameError, Span};
use ipe_intern::{Interner, Symbol};
use ipe_ir::{FuncId, IrType, ModPath, Program, TypeDef};

pub use emit_doc::{SweepDivergence, native_vs_legacy_sweep};
pub use preamble::{epilogue, preamble};

/// Which SQL database driver the emitted project targets.
///
/// Selected by `ipe.toml`'s `[database] driver` key
/// (`crates/ipe/src/project.rs::DbDriver`, converted at the `ipe` →
/// `ipe_backend_rust` boundary via [`RustBackend::with_db_driver`]) — drives
/// the `ipe_runtime/config.rs` template and `Cargo.toml` sqlx feature
/// [`crate::project::emit_program`] selects. `Sqlite` is the default: a
/// program with no `[database]` section, or one built via the single-file
/// `ipe build` path (no manifest at all), emits byte-identical output to
/// pre-driver-selection backends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DbDriver {
    #[default]
    Sqlite,
    Postgres,
}

/// The consumer-side FFI emission inputs.
///
/// Assembled by the driver from the project's installed FFI artifact cache
/// and threaded in via [`RustBackend::with_ffi`]. Ignored entirely when the
/// program lowers no [`ipe_ir::Callee::Ffi`] call (`uses_ffi` stays false).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FfiEmit {
    /// `"Rust.<Crate>.<TypeName>"` → absolute Rust path
    /// (`"Rust.Semver.Version"` → `"::semver::Version"`) for every opaque
    /// foreign type an installed interface module declares.
    pub foreign_types: BTreeMap<String, String>,
    /// Pinned `[dependencies]` lines for the bound crates (exact versions +
    /// effective feature sets — never `"*"`), pre-merged and de-duplicated by
    /// the driver.
    pub dep_lines: Vec<String>,
    /// The full `src/ffi.rs` content: every installed crate's wrapper module
    /// (`pub mod <slug> { … }` + `pub use <slug>::*;`).
    pub bindings_source: String,
    /// The dotted interface-module name (`"Rust.Firestore"`) of every
    /// installed crate — the compiler-generated forwarder modules the
    /// used-set shake may slice (user modules are never listed here, so a
    /// user fn can never be shaken).
    pub interface_modules: Vec<String>,
}

/// The Rust code-generation backend.
///
/// Holds a reference to the [`Interner`] used to build the program, so it can
/// resolve the [`Symbol`]s carried by the IR without widening the
/// [`Backend::emit`] signature.
pub struct RustBackend<'a> {
    interner: &'a Interner,
    db_driver: DbDriver,
    ffi: Option<FfiEmit>,
    target: ipe_ir::Target,
    wasm_public_env: Vec<String>,
    wasm_hydrate_mode: bool,
}

impl<'a> RustBackend<'a> {
    /// Construct a backend that resolves IR symbols through `interner`.
    /// Defaults to [`DbDriver::Sqlite`] — call [`Self::with_db_driver`] to
    /// target Postgres.
    #[must_use]
    pub const fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            db_driver: DbDriver::Sqlite,
            ffi: None,
            target: ipe_ir::Target::Native,
            wasm_public_env: Vec::new(),
            wasm_hydrate_mode: false,
        }
    }

    /// Select the compilation target the emitted project is built for
    /// (`Native` by default; `WasmClient` under `ipe build --target wasm`).
    #[must_use]
    pub const fn with_target(mut self, target: ipe_ir::Target) -> Self {
        self.target = target;
        self
    }

    /// Select the SQL driver the emitted project targets (from `ipe.toml`'s
    /// `[database] driver`). No-op on programs that don't use any Db kernel.
    #[must_use]
    pub const fn with_db_driver(mut self, driver: DbDriver) -> Self {
        self.db_driver = driver;
        self
    }

    /// Supply the consumer-side FFI emission inputs (from the project's
    /// installed FFI artifact cache). No-op on programs that lower no
    /// foreign-wrapper call.
    #[must_use]
    pub fn with_ffi(mut self, ffi: Option<FfiEmit>) -> Self {
        self.ffi = ffi;
        self
    }

    /// Supply the `[wasm] publicEnv` allowlist (from `ipe.toml`, already
    /// validated against the secret-name denylist at parse time — see
    /// `ipe_cli::project::is_denylisted_public_env_name`). No-op on programs
    /// that call no `Env.public`. Threaded through regardless of
    /// [`Self::target`] so a shared module compiling under both `Native` and
    /// `WasmClient` gets the SAME allowlist on each.
    #[must_use]
    pub fn with_wasm_public_env(mut self, names: Vec<String>) -> Self {
        self.wasm_public_env = names;
        self
    }

    /// Enable the `[wasm] mode = "hydrate"` emission path (M7 SSR + hydration).
    /// When set, the emitted wasm crate exports a `#[wasm_bindgen] pub fn
    /// hydrate(model_json: &str)` in addition to the `#[wasm_bindgen(start)]`
    /// `ipe_start` entry. The `hydrate` export parses the island JSON,
    /// calls `ipe_runtime::wasm::wasm_adopt_app` on success, and falls back to
    /// `ipe_main()` with a console warning on parse failure (fault-tolerant
    /// hydrate — spec Q6 §"Fault-tolerant hydrate — parse, don't unwrap").
    #[must_use]
    pub const fn with_wasm_hydrate_mode(mut self, enabled: bool) -> Self {
        self.wasm_hydrate_mode = enabled;
        self
    }

    /// Render the `Spine` tier's Rust text for `program` — the program-wide
    /// entry file content (see [`project::emit_spine`] for the full
    /// specification). ADDITIVE entry point: NOT on the public
    /// emission path ([`Backend::emit`] still produces a single file). Builds
    /// the [`EmitCtx`] the same way [`Backend::emit`] does, then delegates.
    ///
    /// # Errors
    ///
    /// Propagates any [`Diagnostic`] from [`EmitCtx::build`] or
    /// [`project::emit_spine`].
    pub fn emit_spine(&self, program: &Program) -> DResult<String> {
        let ctx = EmitCtx::build(
            self.interner,
            program,
            self.db_driver,
            self.ffi.clone(),
            self.target,
            self.wasm_public_env.clone(),
            self.wasm_hydrate_mode,
        )?;
        project::emit_spine(&ctx, program)
    }

    /// Render one `IpeModule(home)` file's Rust text for `program` (see
    /// [`project::emit_module_file`]). ADDITIVE entry point: NOT
    /// on the public emission path. Builds the [`EmitCtx`] the same way
    /// [`Backend::emit`] does, then delegates.
    ///
    /// # Errors
    ///
    /// Propagates any [`Diagnostic`] from [`EmitCtx::build`] or
    /// [`project::emit_module_file`].
    pub fn emit_module_file(&self, program: &Program, home: &ModPath) -> DResult<String> {
        let ctx = EmitCtx::build(
            self.interner,
            program,
            self.db_driver,
            self.ffi.clone(),
            self.target,
            self.wasm_public_env.clone(),
            self.wasm_hydrate_mode,
        )?;
        project::emit_module_file(
            &ctx,
            program,
            &rust_file::RustFileId::IpeModule(home.clone()),
        )
    }

    /// Assemble the full split [`EmittedProject`] from already-rendered per-file
    /// texts — `spine_text` (from [`Self::emit_spine`]) plus `module_texts`
    /// (`home` → [`Self::emit_module_file`] output). The
    /// file-count-AGNOSTIC manifest/runtime block is shared verbatim with
    /// [`Backend::emit`]'s single-file path, so the result is byte-identical to
    /// [`Backend::emit`]'s output for the same multi-module program (design doc
    /// §4.4 — the `ipe_db::emit_manifest` assembly seam). NOT on the
    /// single-file path; the caller ([`ipe_db::emit_manifest`]) invokes it ONLY
    /// when 2+ distinct homes are present.
    ///
    /// # Errors
    ///
    /// Propagates any [`Diagnostic`] from [`EmitCtx::build`] or
    /// [`project::assemble_split_manifest`] (`mod_ident` gates, `RelPath`
    /// validation, a missing per-home text, the manifest/runtime construction).
    pub fn assemble_split_manifest(
        &self,
        program: &Program,
        spine_text: &str,
        module_texts: &BTreeMap<ModPath, String>,
    ) -> DResult<EmittedProject> {
        let ctx = EmitCtx::build(
            self.interner,
            program,
            self.db_driver,
            self.ffi.clone(),
            self.target,
            self.wasm_public_env.clone(),
            self.wasm_hydrate_mode,
        )?;
        project::assemble_split_manifest(&ctx, program, spine_text, module_texts)
    }
}

/// The DISTINCT Ipê-module `home`s the backend would emit an OWN Rust file for.
///
/// Every [`rust_file::RustFileId::IpeModule`] bucket [`rust_file::
/// partition_items`] produces, `Spine` excluded (`Spine` is not a per-module
/// home — it is the always-present entry file). This is the `home`-set
/// quantifier `ipe_db::program_rust_file_ids` wraps in a
/// tracked query (spec §4.2): a Ipê-module add/delete changes this set, making
/// "which files exist" a first-class, salsa-observable value.
///
/// Order-agnostic by construction (`BTreeSet`) — callers that need the
/// warm/cold-stable EMISSION order use [`rust_file::partition_items`]'s
/// `type_order`/`func_order` directly ([`project::emit_program`] does); this
/// set answers only the membership question.
#[must_use]
pub fn rust_file_homes(program: &Program, interner: &Interner) -> BTreeSet<ModPath> {
    rust_file::partition_items(program, interner)
        .buckets
        .into_keys()
        .filter_map(|id| match id {
            rust_file::RustFileId::IpeModule(home) => Some(home),
            rust_file::RustFileId::Spine => None,
        })
        .collect()
}

impl Backend for RustBackend<'_> {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn emit(&self, program: &Program) -> DResult<EmittedProject> {
        let ctx = EmitCtx::build(
            self.interner,
            program,
            self.db_driver,
            self.ffi.clone(),
            self.target,
            self.wasm_public_env.clone(),
            self.wasm_hydrate_mode,
        )?;
        project::emit_program(&ctx, program)
    }
}

/// One enum's variants as `(variant name, payload field types)`, in
/// declaration order — the value shape of [`EmitCtx::enum_variants`].
type VariantList = Vec<(Symbol, Vec<IrType>)>;

/// A canonical record field list: `(Ipê field name, field type)` pairs sorted by
/// field name. The order is the struct's declaration / `IpeStringify` read order.
type RecordFields = Vec<(String, IrType)>;

/// Every DISTINCT field-type shape observed for one field-name set, in
/// first-occurrence order — a generic template and/or its concrete
/// instantiations, reconciled by [`canonicalise_shape`].
type ShapeOccurrences = Vec<RecordFields>;

/// A synthesised struct's reconciled form: its canonical field template and its
/// generic parameter symbols (empty for a monomorphic record).
type CanonicalShape = (RecordFields, Vec<Symbol>);

/// A synthesised Rust struct for one distinct CLOSED record shape.
///
/// `fields` is the field set in canonical (field-name ascending) order — the
/// order the struct is declared in and the order its `IpeStringify` body reads.
pub(crate) struct RecordStruct {
    /// The deduplicated, collision-free Rust struct name (e.g. `RecXY`).
    pub name: String,
    /// The fields as `(Ipê field name, field type)`, sorted by field name. The
    /// Rust field identifier is the keyword-mangled field name.
    ///
    /// For a GENERIC record shape, a field's type may be an
    /// [`IrType::Generic`]; the carried [`Symbol`] is the canonical template's
    /// source type-variable, resolved to its Rust generic name (`T1`, `T2`, …)
    /// through a [`crate::emit_types::GenericScope`] over [`Self::type_params`].
    pub fields: Vec<(String, IrType)>,
    /// The struct's generic type parameters: the distinct
    /// [`IrType::Generic`] symbols appearing in [`Self::fields`], in
    /// first-occurrence field order. Empty for a monomorphic record — that path
    /// emits no generic clause.
    ///
    /// The order is load-bearing: a parameter's Rust name (`T1`, `T2`, …) is its
    /// *position* here, exactly as for [`ipe_ir::Func::type_params`], so struct
    /// declaration, field types, and every use-site instantiation agree.
    pub type_params: Vec<Symbol>,
    /// `true` iff every field's type renders to a Rust type supporting the full
    /// `#[derive(Clone, Debug, PartialEq)]` set (see
    /// [`ipe_ir::ir_type_is_derivable`]). Computed once at [`EmitCtx::build`]
    /// against the whole-program enum-derivability fixpoint. The emitter reads
    /// this flag to choose the `CDPeq` derive set, so a record holding a first-class
    /// function / opaque wrapper can never reach the unconditional derive by
    /// construction (upholds the SEAL).
    pub is_derivable: bool,
    /// `true` iff every field's type renders to a Rust type that ALSO derives
    /// `serde::Serialize` + `serde::Deserialize` (see [`ipe_ir::ir_type_is_serde`]).
    /// Computed once at [`EmitCtx::build`] against the whole-program enum-serde
    /// fixpoint. STRICTLY implies [`Self::is_derivable`] (serde-OK leaves are a
    /// subset of derivable leaves). The emitter reads this flag — NOT
    /// `is_derivable` — to gate the serde derive under `uses_web`, so a
    /// `CDPeq`-but-not-serde record (a view-helper holding `Html` / `Element` /
    /// `Color` / a `UiPlain` value) in a Ipe.Web program never gets serde forced
    /// onto it and therefore never exit-0-then-cargo-fails on `E0277` (upholds
    /// the SEAL).
    pub is_serde: bool,
    /// `true` iff every field's DEFAULT emitted carrier is `Clone` (see
    /// [`ipe_ir::carrier_is_clone`]) — a strictly WEAKER property than
    /// [`Self::is_derivable`], because the promoted `Arc<dyn Fn>` fn carrier
    /// ([`ipe_ir::IrType::SharedFun`]) is `Clone` but neither `Debug` nor
    /// `PartialEq`. A record that is `is_clone` but not `is_derivable` gets a
    /// HAND-WRITTEN `impl Clone` (never the `CDPeq` derive), which is what lets
    /// the fn-value-reuse promotion duplicate a reused record-of-functions —
    /// without it, `c.clone()` would be an `ipe`-0-then-cargo-fail `E0599`
    /// (SEAL break). `is_derivable ⇒ is_clone` (a `CDPeq` type is `Clone`), so
    /// the two flags never disagree in the derivable direction.
    pub is_clone: bool,
}

/// Shared emission context: the interner plus the precomputed Ipê → Rust name
/// maps so each emit site is a `O(log n)` lookup rather than recomputing the
/// naming rules. Built once per [`RustBackend::emit`].
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct EmitCtx<'a> {
    interner: &'a Interner,
    /// `true` when the lowerer injected synthetic `SqlValue` / `SqlField`
    /// `EnumDef`s — i.e. the program uses at least one Db kernel. When set,
    /// [`crate::project::emit_program`]:
    ///
    /// * emits a db-enabled `Cargo.toml` (adds `db` to default features +
    ///   adds the `sqlx` dependency);
    /// * emits a db-enabled `ipe_runtime/mod.rs` (adds `pub mod db; pub use
    ///   db::*;`);
    /// * emits a db-enabled `ipe_runtime/config.rs` (full DbPool/DbRow/…
    ///   aliases gated on `#[cfg(feature = "db")]`);
    /// * appends `into_sql_param` / `into_field_param` impl blocks after the
    ///   user type declarations, so the Db call sites can project Ipê ADT
    ///   values to the runtime's `SqlParam`.
    pub(crate) uses_db: bool,
    /// Which SQL driver the emitted `Cargo.toml` / `ipe_runtime/config.rs`
    /// target when [`Self::uses_db`] is set (`ipe.toml`'s `[database] driver`,
    /// threaded in via [`RustBackend::with_db_driver`]). Meaningless / ignored
    /// when `uses_db` is `false`.
    pub(crate) db_driver: DbDriver,
    /// The compilation target (`Native` | `WasmClient`) — selects the manifest
    /// template, the vendored runtime module set, and the entry shape.
    pub(crate) target: ipe_ir::Target,
    /// `true` when the program uses at least one TEA (`Cmd` / `Sub` /
    /// `Time.every`) kernel. When set,
    /// [`crate::project::emit_program`]:
    ///
    /// * appends `pub mod tea; pub use tea::*;` to the emitted
    ///   `ipe_runtime/mod.rs`, making `cmd_none` / `sub_every` / … available;
    /// * adds `pub type IpeCmd<M> = ipe_runtime::tea::IpeCmd<M>;` and
    ///   `pub type IpeSub<M> = ipe_runtime::tea::IpeSub<M>;` to `main.rs`.
    ///
    /// `tea.rs` is ungated (no `live` cargo feature needed); the only
    /// dependency is `tokio`, which is already in the default feature set.
    pub(crate) uses_tea: bool,
    /// `true` when the program uses at least one Ipe.Http.Server kernel.
    /// When set, [`crate::project::emit_program`]:
    ///
    /// * adds `"server"` to the default features of the emitted `Cargo.toml`
    ///   and appends `axum` + `tower-http` dependencies;
    /// * extends the `tokio` dependency with `"net"` and `"sync"` features;
    /// * appends `pub mod server; pub use server::*; pub mod server_stream;
    ///   pub use server_stream::*;` to the emitted `ipe_runtime/mod.rs`.
    pub(crate) uses_server: bool,
    /// `true` when the program uses at least one `Ipe.Ui` / `Ipe.Html` kernel.
    /// When set, [`crate::project::emit_program`] appends
    /// `pub mod ui;` to the emitted `ipe_runtime/mod.rs`.
    pub(crate) uses_ui: bool,
    /// `true` when the program uses at least one `Ipe.Web` / `Ipe.Web`
    /// app-entry kernel.  When set, the emitted project gains the `"live"`
    /// Cargo feature, serde derives on all emitted types, and
    /// `ipe_runtime::live` wired into the runtime module set.
    pub(crate) uses_web: bool,
    /// `true` when the program uses at least one `Ipe.Tui` / `Ipe.Tui`
    /// app-entry kernel.  When set, the emitted project gains the `"tui"`
    /// Cargo feature and the tui module is wired into `ipe_runtime/mod.rs`.
    pub(crate) uses_tui: bool,
    /// `true` when the program uses at least one `Ipe.WebView` app-entry kernel.
    /// When set, the emitted project gains the `"webview"` Cargo feature
    /// (which transitively pulls `"live"`) and the main entry is switched to
    /// `block_on_current_thread` (tao/Cocoa requires the process main thread).
    pub(crate) uses_webview: bool,
    /// `true` when the program uses at least one `Ipe.CssSafety` leaf
    /// security kernel (the `Ipe.Css` backing).  When set (independently of
    /// [`Self::uses_ui`]), [`crate::project::emit_program`] declares
    /// `css_safety` / `css` (`pub use css::*`) in the emitted
    /// `ipe_runtime/mod.rs` so the bare `safe_value` / `safe_prop_name` /
    /// `safe_selector` / `strip_style_close_kernel` names are in scope — a pure
    /// `Ipe.Css` program never sets `uses_ui`, so the UI append alone would leave
    /// those names undeclared (E0425).
    pub(crate) uses_css: bool,
    /// `true` when the program uses at least one `Ipe.Auth` kernel
    /// (`Auth.hashPassword`, `Auth.verifyPassword`, `Auth.signToken`,
    /// `Auth.verifyToken`, `Auth.register`, `Auth.login`, `Auth.setRole`, etc.).
    /// When set, [`crate::project::emit_program`] appends
    /// `pub mod auth; pub use auth::*;` to the emitted `ipe_runtime/mod.rs`.
    pub(crate) uses_auth: bool,
    /// `true` when the program uses at least one outbound `Ipe.WebSocket`
    /// client kernel (`WebSocket.connect` / `send` / `close` / … or an `on*`
    /// subscription).  When set, [`crate::project::assemble_project_files`] adds
    /// the `websocket_client` feature (+ `tokio-tungstenite` dep) to the emitted
    /// `Cargo.toml` and appends `pub mod ws_client; pub use ws_client::*;` to the
    /// emitted `ipe_runtime/mod.rs` — the `ws_client` module is feature-gated and
    /// NOT part of the base module set.
    pub(crate) uses_websocket: bool,
    /// `true` when the program uses the `Ipe.Email` `Email.send` kernel. When
    /// set, [`crate::project::emit_program`] appends `pub mod email; pub use
    /// email::*;` to the emitted `ipe_runtime/mod.rs` and adds the `lettre`
    /// dependency to the emitted `Cargo.toml`.
    pub(crate) uses_email: bool,
    /// `true` when the program lowers at least one [`ipe_ir::Callee::Ffi`]
    /// foreign-wrapper call. When set,
    /// [`crate::project::assemble_project_files`] writes `src/ffi.rs` (from
    /// [`Self::ffi`]), declares `mod ffi;` in `src/main.rs`, and appends the
    /// bound crates' pinned `[dependencies]` lines to the emitted `Cargo.toml`.
    pub(crate) uses_ffi: bool,
    /// The driver-supplied FFI emission inputs. Required (fail-closed) when
    /// [`Self::uses_ffi`] is set; ignored otherwise.
    pub(crate) ffi: Option<FfiEmit>,
    /// `true` when the program uses the `Ipe.Env` `Env.public` kernel. When
    /// set, [`crate::project::assemble_project_files`] emits the per-project
    /// `ipe_runtime/env_public.rs` (built from [`Self::wasm_public_env`]) and
    /// appends `pub mod env_public; pub use env_public::*;` to the emitted
    /// `ipe_runtime/mod.rs`, on EITHER target.
    pub(crate) uses_env_public: bool,
    /// The `[wasm] publicEnv` allowlist (`ipe.toml`, threaded in via
    /// [`RustBackend::with_wasm_public_env`]) — already validated against the
    /// secret-name denylist at `ipe.toml` parse time. Meaningless / ignored
    /// when [`Self::uses_env_public`] is `false`.
    pub(crate) wasm_public_env: Vec<String>,
    /// `true` when `[wasm] mode = "hydrate"` was set in `ipe.toml`. When set,
    /// the emitted wasm epilogue includes a `#[wasm_bindgen] pub fn hydrate(…)`
    /// export in addition to the `#[wasm_bindgen(start)] ipe_start` entry —
    /// the fault-tolerant island parse + `wasm_adopt_app` fallback path (M7).
    pub(crate) wasm_hydrate_mode: bool,
    /// The Rust type name for the emitted `SqlValue` enum (e.g. `MainSqlValue`).
    /// `None` when `uses_db` is `false`.
    pub(crate) sqlvalue_rust_name: Option<String>,
    /// The Rust type name for the emitted `SqlField` enum (e.g. `MainSqlField`).
    /// `None` when `uses_db` is `false`.
    pub(crate) sqlfield_rust_name: Option<String>,
    /// The Rust type the `[wasm] mode = "hydrate"` glue must name as the island
    /// parse target — the `HydrationState` type the user's `fromHydrationState`
    /// projection takes. Resolved through the SAME [`emit_types::render_type`]
    /// the emitted `main_from_hydration_state` signature uses, so the glue and
    /// the signature reference one identical type name and cannot drift (a record
    /// alias `{ count : Int }` resolves to its synthesised `RecCount`, a named
    /// ADT to `MainHydrationState`, etc.). `None` when the program declares no
    /// `fromHydrationState` (a hydrate-mode program with no explicit projection);
    /// the glue then falls back to a clean `ipe_main()` init.
    pub(crate) hydration_state_rust_name: Option<String>,
    /// Type nominal identity `(home, name)` → Rust type name (e.g.
    /// `(["Main"], Msg)` → `MainMsg`, `(["Lib"], Color)` → `LibColor`). Keyed by
    /// `(home, name)` — not `name` alone — so two modules each declaring `type
    /// Color` map to distinct Rust enums instead of colliding.
    enum_names: BTreeMap<(ModPath, Symbol), String>,
    /// `((enum home, enum name), variant symbol)` → that variant's declared
    /// payload field types, in source (positional) order. Empty vector for a
    /// nullary variant. Used at construction / pattern sites to box (and un-box) a
    /// recursive field so the emitted Rust enum stays finite-sized.
    variant_fields: BTreeMap<(ModPath, Symbol, Symbol), Vec<IrType>>,
    /// Type identity `(home, name)` → every variant's `(name, field types)`,
    /// in declaration order. The whole-enum view that
    /// [`Self::is_cyclic_self_field`] walks to decide whether a payload field
    /// sits on a type-size cycle back to its own enum — direct (`Node Tree …`)
    /// or indirect (mutual recursion, or a self-edge routed through a tuple /
    /// record / another generic's type argument) — and that the Model schema
    /// tag ([`emit_model_schema`]) folds variant NAMES from at their declared
    /// positions (the serialized discriminant is assigned by declaration
    /// index, so a variant rename AND a reorder are both wire-format-relevant).
    enum_variants: BTreeMap<(ModPath, Symbol), VariantList>,
    /// Enum type symbol → whether that user enum's rendered Rust type supports
    /// the full `#[derive(Clone, Debug, PartialEq)]` set. Computed by a monotone
    /// whole-program fixpoint at [`EmitCtx::build`]: an enum is non-derivable iff
    /// some variant payload reaches a non-derivable leaf (a function, an opaque
    /// wrapper, or another non-derivable enum). Read by the emitter to gate the
    /// derive set on user enums and on record structs (upholds the SEAL).
    /// Whole-program (all modules) so cross-module `IrType::Enum` references
    /// resolve soundly.
    enum_derivable: BTreeMap<(ModPath, Symbol), bool>,
    /// Enum type symbol → whether that user enum's rendered Rust type derives
    /// `serde::Serialize` **and** `serde::de::DeserializeOwned`. Computed by a
    /// monotone whole-program fixpoint parallel to [`Self::enum_derivable`]: an
    /// enum is non-serde iff some variant payload reaches a non-serde leaf (the
    /// non-derivable set PLUS the `Clone`-only UI value/carrier types, per
    /// [`ipe_ir::ir_type_is_serde`]). Read by the Ipe.Web app-entry Model gate
    /// (upholds the SEAL). Whole-program so cross-module `IrType::Enum`
    /// references resolve soundly.
    enum_serde: BTreeMap<(ModPath, Symbol), bool>,
    /// Function id → Rust function name (e.g. `update` → `main_update`).
    func_names: BTreeMap<FuncId, String>,
    /// Every distinct record shape synthesised for the program, in emission
    /// order (sorted by field-name set).
    record_structs: Vec<RecordStruct>,
    /// Sorted field-name set → index into [`Self::record_structs`]. The field
    /// set is the canonical key: every `IrType::Record` and every record
    /// literal resolves to its struct through it.
    record_by_fieldset: BTreeMap<Vec<String>, usize>,
}

impl<'a> EmitCtx<'a> {
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::similar_names)] // `uses_ui` / `uses_tui` are intentionally similar
    fn build(
        interner: &'a Interner,
        program: &Program,
        db_driver: DbDriver,
        ffi: Option<FfiEmit>,
        target: ipe_ir::Target,
        wasm_public_env: Vec<String>,
        wasm_hydrate_mode: bool,
    ) -> DResult<Self> {
        let mut enum_names: BTreeMap<(ModPath, Symbol), String> = BTreeMap::new();
        let mut variant_fields: BTreeMap<(ModPath, Symbol, Symbol), Vec<IrType>> = BTreeMap::new();
        let mut enum_variants: BTreeMap<(ModPath, Symbol), VariantList> = BTreeMap::new();
        let mut func_names = BTreeMap::new();
        for module in &program.modules {
            let segs = module
                .name
                .0
                .iter()
                .map(|s| resolve_sym(interner, *s))
                .collect::<DResult<Vec<&str>>>()?;
            for ty in &module.types {
                let TypeDef::Enum(def) = ty;
                // The emitted Rust type name is derived from the type's HOME
                // module (its defining module), NOT the merged entry module the
                // linker pooled it into — so `Ipe.Palette.Shade` → `StdPaletteShade`
                // and `Lib.Color` → `LibColor`, while a single-module program
                // (home == entry) stays byte-identical (`Main.Msg` → `MainMsg`).
                // When `def.home` is empty (IR built directly in backend unit
                // tests, not through the full pipeline), fall back to the merged
                // module path — always correct for single-module programs.
                let home_segs: Vec<&str> = if def.home.0.is_empty() {
                    segs.clone()
                } else {
                    def.home
                        .0
                        .iter()
                        .map(|s| resolve_sym(interner, *s))
                        .collect::<DResult<Vec<&str>>>()?
                };
                // Ipe.WebSocket's `WebSocketMessage` / `CloseCode` ADTs are
                // BRIDGED to the runtime enums `WsClientMessage` / `WsCloseCode`
                // (the `sub_subscribe_ws_*` fns take/produce those). Overriding the
                // emitted enum name here makes every reference — the type in a
                // handler's `WebSocketMessage -> msg` param, a `Text s` / `Normal`
                // constructor, a `case … of Text t ->` pattern — resolve to the
                // runtime enum, whose variant names + field types match 1:1. The
                // Ipê enum DECL is suppressed in `emit_enum` (a bridged type has no
                // user-emitted body). Same intent as the reference `Types.hs` map.
                let def_name = resolve_sym(interner, def.name)?;
                let rust_name = websocket_bridge_rust_name(&home_segs, def_name)
                    .map_or_else(|| naming::enum_name(&home_segs, def_name), str::to_owned);
                // A type's nominal identity is `(home, name)`. Two modules each
                // declaring `type Color` share the bare `name` `Symbol` but differ
                // in `home`, so they do not collide — each keys a distinct Rust
                // enum. A genuine duplicate `(home, name)` (the SAME type
                // declared twice) is caught upstream by the link-level gate; this
                // stays as a fail-closed defence-in-depth backstop (IPE-I0202).
                let key = (def.home.clone(), def.name);
                if enum_names.contains_key(&key) {
                    return Err(Diagnostic::Name {
                        span: Span::DUMMY,
                        msg: NameError::DuplicateType {
                            name: rust_name.into_boxed_str(),
                            first: Span::DUMMY,
                        },
                    });
                }
                // Guard the emitted-name space too: `naming::enum_name`'s camel-case
                // fold is not injective over the (home, name) split (`["Std",
                // "Palette"]/Color` and `["Std"]/PaletteColor` both fold to
                // `StdPaletteColor`), so two DISTINCT identities could otherwise
                // emit the same Rust enum and trip `rustc` E0428. Fail closed with
                // the same duplicate-type diagnostic rather than emit a broken crate.
                if enum_names.values().any(|n| n == &rust_name) {
                    return Err(Diagnostic::Name {
                        span: Span::DUMMY,
                        msg: NameError::DuplicateType {
                            name: rust_name.into_boxed_str(),
                            first: Span::DUMMY,
                        },
                    });
                }
                enum_names.insert(key.clone(), rust_name.clone());
                let mut all_fields = Vec::with_capacity(def.variants.len());
                for variant in &def.variants {
                    variant_fields.insert(
                        (def.home.clone(), def.name, variant.name),
                        variant.fields.clone(),
                    );
                    all_fields.push((variant.name, variant.fields.clone()));
                }
                enum_variants.insert(key, all_fields);
            }
            for func in &module.funcs {
                // Use the function's own home module path for naming so that two
                // functions with the same bare name from different source modules
                // emit distinct Rust names (e.g. `lib_helper` vs `main_helper`).
                // When `func.home` is empty (IR built directly in backend unit
                // tests, not through the full pipeline), fall back to the
                // containing merged module's path — the pre-Defect-2 behaviour —
                // which is always correct for single-module programs.
                let effective_home: &[_] = if func.home.0.is_empty() {
                    &module.name.0
                } else {
                    &func.home.0
                };
                let func_segs = effective_home
                    .iter()
                    .map(|s| resolve_sym(interner, *s))
                    .collect::<DResult<Vec<&str>>>()?;
                let rust_name = naming::module_value(&func_segs, resolve_sym(interner, func.name)?);
                // AUD-08: mirror the enum guard above (`enum_names.values().any`,
                // line ~306). `naming::module_value`'s snake_case fold is not
                // injective over the (home, name) split — `["Std", "Ui"]/borderRounded`
                // and `["Std", "Ui", "Border"]/rounded` both fold to
                // `std_ui_border_rounded` — so two DISTINCT functions could
                // otherwise emit the same Rust fn and trip `rustc` E0428. Fail
                // closed with the same duplicate-value diagnostic rather than
                // emit a broken crate.
                if func_names.values().any(|n| n == &rust_name) {
                    return Err(Diagnostic::Name {
                        span: Span::DUMMY,
                        msg: NameError::DuplicateValue {
                            name: rust_name.into_boxed_str(),
                            first: Span::DUMMY,
                        },
                    });
                }
                func_names.insert(func.id, rust_name);
            }
        }

        // Prepass: collect every distinct CLOSED record shape the program uses
        // (recursing into nested records / tuples), so each gets one synthesised
        // struct, declared before any use. Two sources feed it: function
        // signatures (params / return), and the lowerer-surfaced `module.records`
        // — the shapes of record literals that live inside function bodies, where
        // the type appears in no signature. A record literal resolves to its
        // struct through this table by its field-name set; a literal whose set is
        // absent is an internal invariant violation (surfaced as a `CompilerBug`,
        // never a silent mis-emit).
        //
        // Each field-name set maps to the LIST of distinct field-type shapes seen
        // for it. A set may carry both a generic template (`{ value : a }`, from a
        // parametric signature) and concrete instantiations (`{ value : Int }`):
        // [`canonicalise_shape`] reconciles them into a single struct.
        let mut shapes: BTreeMap<Vec<String>, ShapeOccurrences> = BTreeMap::new();
        for module in &program.modules {
            for func in &module.funcs {
                for (_, ty) in &func.params {
                    collect_record_shapes(interner, ty, &mut shapes)?;
                }
                collect_record_shapes(interner, &func.ret, &mut shapes)?;
            }
            for ty in &module.records {
                collect_record_shapes(interner, ty, &mut shapes)?;
            }
            // An enum variant's payload field type may itself be (or carry) a
            // record shape (`type Boxed a = Box { value : a }`). The variant
            // field types are not in any signature, so collect them here too —
            // otherwise emitting the enum would resolve the record type to a
            // struct that was never synthesised (a `CompilerBug` miss).
            for ty in &module.types {
                let TypeDef::Enum(def) = ty;
                for variant in &def.variants {
                    for field_ty in &variant.fields {
                        collect_record_shapes(interner, field_ty, &mut shapes)?;
                    }
                }
            }
        }
        // seal: whole-program enum-derivability fixpoint. Every user enum
        // starts optimistic (derivable), then is monotonically demoted to
        // non-derivable if any variant payload reaches a non-derivable leaf (a
        // first-class function, an opaque effect/handle wrapper, or a — by the
        // current estimate — non-derivable enum). Non-derivability only
        // propagates (the lattice descends true → false), so the loop reaches a
        // fixpoint in at most `enum count` passes. `ir_type_is_derivable`
        // consults `lookup` for referenced enums; a name absent from the map
        // (never a user enum — builtins are distinct `IrType` variants) defaults
        // to derivable, which can only be as permissive as the pre-seal
        // unconditional derive.
        let mut enum_derivable: BTreeMap<(ModPath, Symbol), bool> =
            enum_variants.keys().map(|k| (k.clone(), true)).collect();
        loop {
            let mut to_demote: Vec<(ModPath, Symbol)> = Vec::new();
            {
                let lookup = |home: &ModPath, name: Symbol| {
                    enum_derivable
                        .get(&(home.clone(), name))
                        .copied()
                        .unwrap_or(true)
                };
                for (key, variants) in &enum_variants {
                    if !enum_derivable.get(key).copied().unwrap_or(true) {
                        continue;
                    }
                    let ok = variants.iter().all(|(_, fields)| {
                        fields
                            .iter()
                            .all(|f| ipe_ir::ir_type_is_derivable(f, &lookup))
                    });
                    if !ok {
                        to_demote.push(key.clone());
                    }
                }
            }
            if to_demote.is_empty() {
                break;
            }
            for s in to_demote {
                enum_derivable.insert(s, false);
            }
        }

        // seal: whole-program enum-serde fixpoint, computed identically to
        // `enum_derivable` above but through `ir_type_is_serde` (whose serde-OK
        // leaf set is a strict subset — the UI value/carrier types are `Clone`
        // but not `serde`). Every user enum starts optimistic (serde) and is
        // monotonically demoted if any variant payload reaches a non-serde leaf
        // or a (currently-estimated) non-serde enum. Non-serde only propagates
        // (true → false), so the loop reaches a fixpoint in at most `enum count`
        // passes. Read by the Ipe.Web Model-admissibility gate.
        let mut enum_serde: BTreeMap<(ModPath, Symbol), bool> =
            enum_variants.keys().map(|k| (k.clone(), true)).collect();
        loop {
            let mut to_demote: Vec<(ModPath, Symbol)> = Vec::new();
            {
                let lookup = |home: &ModPath, name: Symbol| {
                    enum_serde
                        .get(&(home.clone(), name))
                        .copied()
                        .unwrap_or(true)
                };
                for (key, variants) in &enum_variants {
                    if !enum_serde.get(key).copied().unwrap_or(true) {
                        continue;
                    }
                    let ok = variants.iter().all(|(_, fields)| {
                        fields.iter().all(|f| ipe_ir::ir_type_is_serde(f, &lookup))
                    });
                    if !ok {
                        to_demote.push(key.clone());
                    }
                }
            }
            if to_demote.is_empty() {
                break;
            }
            for s in to_demote {
                enum_serde.insert(s, false);
            }
        }

        let mut record_structs = Vec::with_capacity(shapes.len());
        let mut record_by_fieldset = BTreeMap::new();
        let mut used_names: BTreeSet<String> = BTreeSet::new();
        for (key, occurrences) in shapes {
            let (fields, type_params) = canonicalise_shape(&key, &occurrences)?;
            let name = unique_struct_name(naming::record_struct_name(&key), &mut used_names);
            // seal: a record struct is derivable iff every field type is,
            // consulting the enum fixpoint for referenced user enums.
            let is_derivable = {
                let lookup = |home: &ModPath, name: Symbol| {
                    enum_derivable
                        .get(&(home.clone(), name))
                        .copied()
                        .unwrap_or(true)
                };
                fields
                    .iter()
                    .all(|(_, ty)| ipe_ir::ir_type_is_derivable(ty, &lookup))
            };
            // seal: a record struct is serde-OK iff every field type is,
            // consulting the parallel enum-serde fixpoint. Strictly implies
            // `is_derivable` (serde-OK leaves ⊂ derivable leaves), so a record
            // never gets serde without CDPeq. Gates the serde derive under
            // `uses_web` so a CDPeq-but-not-serde record (Html/Element/Color/
            // UiPlain field) in a Web program is not forced to serde.
            let is_serde = {
                let lookup = |home: &ModPath, name: Symbol| {
                    enum_serde
                        .get(&(home.clone(), name))
                        .copied()
                        .unwrap_or(true)
                };
                fields
                    .iter()
                    .all(|(_, ty)| ipe_ir::ir_type_is_serde(ty, &lookup))
            };
            // A record whose every field carrier is `Clone` (including the
            // promoted `Arc<dyn Fn>` `SharedFun` slot) gets a hand-written
            // `impl Clone` when it is not fully `CDPeq`-derivable — the property
            // the fn-value-reuse promotion relies on to duplicate a reused
            // record-of-functions.
            let is_clone = fields.iter().all(|(_, ty)| ipe_ir::carrier_is_clone(ty));
            record_by_fieldset.insert(key, record_structs.len());
            record_structs.push(RecordStruct {
                name,
                fields,
                type_params,
                is_derivable,
                is_serde,
                is_clone,
            });
        }

        // detect whether the lowerer injected SqlValue / SqlField.
        // The lowerer injects them iff any Db kernel is used. Detecting here
        // (rather than re-scanning Func bodies) avoids a duplicate walk and
        // keeps the "what's in the type list?" answer canonical.
        let uses_db = program.modules.iter().any(|m| {
            m.types.iter().any(|td| {
                let TypeDef::Enum(def) = td;
                interner.resolve(def.name) == Some("SqlValue")
            })
        });
        let sqlvalue_rust_name = if uses_db {
            // The SqlValue enum was injected into the same module as the rest of the
            // program (the lowerer always uses one module), so its Rust name is in
            // `enum_names` under the "SqlValue" symbol.  Look it up by scanning the
            // names map for the entry whose interner resolution is "SqlValue".
            program.modules.iter().find_map(|m| {
                m.types.iter().find_map(|td| {
                    let TypeDef::Enum(def) = td;
                    if interner.resolve(def.name) == Some("SqlValue") {
                        enum_names.get(&(def.home.clone(), def.name)).cloned()
                    } else {
                        None
                    }
                })
            })
        } else {
            None
        };
        let sqlfield_rust_name = if uses_db {
            program.modules.iter().find_map(|m| {
                m.types.iter().find_map(|td| {
                    let TypeDef::Enum(def) = td;
                    if interner.resolve(def.name) == Some("SqlField") {
                        enum_names.get(&(def.home.clone(), def.name)).cloned()
                    } else {
                        None
                    }
                })
            })
        } else {
            None
        };

        // detect whether any TEA kernel is used, from the flag the lowerer
        // set on the module.
        let uses_tea = program.modules.iter().any(|m| m.uses_tea);

        // detect whether any Ipe.Http.Server kernel is used.
        let uses_server = program.modules.iter().any(|m| m.uses_server);

        // detect Ipe.Ui / Ipe.Html / Ipe.Web / Ipe.Tui / Ipe.WebView usage.
        let (uses_ui, uses_web, uses_tui, uses_webview) = (
            program.modules.iter().any(|m| m.uses_ui),
            program.modules.iter().any(|m| m.uses_web),
            program.modules.iter().any(|m| m.uses_tui),
            program.modules.iter().any(|m| m.uses_webview),
        );

        // detect Ipe.Css (Ipe.CssSafety) leaf-kernel usage.
        let uses_css = program.modules.iter().any(|m| m.uses_css);

        // detect Ipe.Auth kernel usage.
        let uses_auth = program.modules.iter().any(|m| m.uses_auth);
        // detect Ipe.Email kernel usage.
        let uses_email = program.modules.iter().any(|m| m.uses_email);
        // detect Ipe.Env `Env.public` kernel usage.
        let uses_env_public = program.modules.iter().any(|m| m.uses_env_public);

        // detect outbound Ipe.WebSocket client usage.
        let uses_websocket = program.modules.iter().any(|m| m.uses_websocket);

        // detect foreign-crate FFI wrapper usage.
        let uses_ffi = program.modules.iter().any(|m| m.uses_ffi);

        let mut ctx = Self {
            interner,
            uses_db,
            db_driver,
            target,
            uses_tea,
            uses_server,
            uses_ui,
            uses_web,
            uses_tui,
            uses_webview,
            uses_css,
            uses_auth,
            uses_websocket,
            uses_email,
            uses_ffi,
            ffi,
            uses_env_public,
            wasm_public_env,
            wasm_hydrate_mode,
            sqlvalue_rust_name,
            sqlfield_rust_name,
            hydration_state_rust_name: None,
            enum_names,
            variant_fields,
            enum_variants,
            enum_derivable,
            enum_serde,
            func_names,
            record_structs,
            record_by_fieldset,
        };
        // Resolve the `HydrationState` type name through the same renderer the
        // emitted `main_from_hydration_state` signature uses, so the wasm-hydrate
        // glue references EXACTLY that type (single source of truth — no drift
        // between glue and signature). Needs the fully-built `ctx` because
        // `render_type` reads its enum/record tables. Only meaningful under
        // `wasm_hydrate_mode`; skip the walk otherwise.
        if wasm_hydrate_mode {
            ctx.hydration_state_rust_name = ctx.resolve_hydration_state_rust_name(program)?;
        }
        Ok(ctx)
    }

    /// The Rust type name of the `HydrationState` island-parse target, resolved
    /// from the user's `fromHydrationState` projection's parameter type through
    /// [`emit_types::render_type`] — the identical renderer that produces the
    /// emitted `main_from_hydration_state` signature. Returns `None` when the
    /// program declares no `fromHydrationState` function (the glue then never
    /// names a hydration type and falls straight through to a clean init).
    fn resolve_hydration_state_rust_name(&self, program: &Program) -> DResult<Option<String>> {
        for module in &program.modules {
            for func in &module.funcs {
                if self.interner.resolve(func.name) != Some("fromHydrationState") {
                    continue;
                }
                let Some((_, param_ty)) = func.params.first() else {
                    // A `fromHydrationState` with no parameter cannot name an
                    // island type — treat as absent (fall through to clean init).
                    return Ok(None);
                };
                // The `HydrationState` island type is monomorphic (the M7
                // field-type gate admits only serialisable, non-generic leaves),
                // so an empty generic scope is exact.
                let rendered =
                    emit_types::render_type(self, param_ty, emit_types::GenericScope::new(&[]))?;
                return Ok(Some(rendered));
            }
        }
        Ok(None)
    }

    /// Is `home` a driver-generated FFI interface module (`Rust.*`)? The
    /// `Rust.*` namespace is origin-reserved at canonicalisation, so the home
    /// prefix IS the provenance.
    pub(crate) fn is_foreign_interface_home(&self, home: &ModPath) -> bool {
        home.0
            .first()
            .and_then(|s| self.interner.resolve(*s))
            .is_some_and(|s| s == "Rust")
    }

    /// The absolute Rust path for a foreign opaque type declared by an FFI
    /// interface module (`(Rust.Semver, Version)` → `::semver::Version`).
    ///
    /// # Errors
    ///
    /// [`Diagnostic::CompilerBug`] when no FFI emission inputs were supplied
    /// or the type is absent from the map — the driver derives the map and
    /// the interface modules from the SAME artifacts, so a miss is an
    /// internal invariant violation, surfaced rather than emitted as a
    /// dangling Rust type.
    pub(crate) fn foreign_type_path(&self, home: &ModPath, name: Symbol) -> DResult<String> {
        let mut key = String::new();
        for seg in &home.0 {
            key.push_str(self.resolve_ident(*seg)?);
            key.push('.');
        }
        key.push_str(self.resolve_ident(name)?);
        self.ffi
            .as_ref()
            .and_then(|f| f.foreign_types.get(&key))
            .cloned()
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "backend.foreign_type_path",
                detail: format!(
                    "foreign opaque type `{key}` has no Rust path in the FFI emission \
                     inputs — the driver must supply every installed crate's opaque map"
                ),
            })
    }

    /// Does user enum `sym`'s rendered Rust type support the full
    /// `#[derive(Clone, Debug, PartialEq)]` set?
    ///
    /// Resolved from the whole-program derivability fixpoint computed at
    /// [`Self::build`]. A symbol that is not a user enum (never reachable as an
    /// [`IrType::Enum`] target — builtins are distinct `IrType` variants)
    /// defaults to `true`, matching the pre-seal unconditional derive.
    pub(crate) fn enum_is_derivable(&self, home: &ModPath, sym: Symbol) -> bool {
        self.enum_derivable
            .get(&(home.clone(), sym))
            .copied()
            .unwrap_or(true)
    }

    /// Does user enum `sym`'s rendered Rust type derive `serde::Serialize` and
    /// `serde::de::DeserializeOwned`?
    ///
    /// Resolved from the whole-program serde fixpoint computed at
    /// [`Self::build`]. A symbol that is not a user enum defaults to `true`
    /// (builtins are distinct `IrType` variants and never reach this lookup as a
    /// bare enum name). Used by the Ipe.Web Model-admissibility gate.
    pub(crate) fn enum_is_serde(&self, home: &ModPath, sym: Symbol) -> bool {
        self.enum_serde
            .get(&(home.clone(), sym))
            .copied()
            .unwrap_or(true)
    }

    /// Every variant of user enum `sym` as `(variant name, payload field
    /// types)`, in declaration order. Empty when `sym` is not a known user
    /// enum. Read by the Model-admissibility gate (payload shapes only) and
    /// by the Model schema tag (names AND shapes — the serialized
    /// discriminant is declaration-index-keyed, so the name at each position
    /// is wire-format-relevant).
    pub(crate) fn enum_variant_payloads(
        &self,
        home: &ModPath,
        sym: Symbol,
    ) -> &[(Symbol, Vec<IrType>)] {
        self.enum_variants
            .get(&(home.clone(), sym))
            .map_or(&[], Vec::as_slice)
    }

    /// The declared payload field types of constructor `variant` of enum `ty`,
    /// in positional order.
    ///
    /// A miss means a constructor expression / pattern names a variant the
    /// program never declared — an upstream-contract violation (the type checker
    /// pins every constructor to its union), surfaced as a [`Diagnostic::CompilerBug`]
    /// rather than a silent mis-emit.
    pub(crate) fn variant_fields(
        &self,
        home: &ModPath,
        ty: Symbol,
        variant: Symbol,
    ) -> DResult<&[IrType]> {
        self.variant_fields
            .get(&(home.clone(), ty, variant))
            .map(Vec::as_slice)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::EmitCtx::variant_fields",
                detail: format!(
                    "no declared field types for variant {} of enum {}",
                    variant.as_raw(),
                    ty.as_raw()
                ),
            })
    }

    /// Is `field` a payload field of enum `enum_sym` that sits on a type-size
    /// cycle back to that enum — so the Rust enum is infinite-sized (E0072)
    /// unless the field is boxed?
    ///
    /// This generalises the old direct-self-edge test (`field` *is* the enum's
    /// own type, `type Tree = … | Node Tree …`) to every cycle the field can
    /// close: mutual recursion between two enums, and a self-edge routed through
    /// a tuple (`Node (Tree, Int)`), a record (`Node { left : Tree }`), or
    /// another generic's type argument (`Node (Maybe Tree)`). The backend wraps
    /// such a field in `Box<…>` at the declaration and balances that with
    /// `Box::new` at construction and a deref at pattern binding — boxing at
    /// least one edge of every cycle, which is what keeps the emitted crate
    /// finite-sized and matches the Go reference's recursive-payload boxing.
    ///
    /// Every *constructible* recursive Ipê type routes through an enum (the enum
    /// supplies the nullary base case), so boxing the cyclic enum-payload edge
    /// breaks every reachable cycle; a hypothetical pure record/tuple alias
    /// cycle (no enum on it) is rejected upstream before it can reach the
    /// backend.
    pub(crate) fn is_cyclic_self_field(
        &self,
        field: &IrType,
        enum_home: &ModPath,
        enum_sym: Symbol,
    ) -> bool {
        let mut visited = BTreeSet::new();
        type_reaches_enum(
            field,
            (enum_home, enum_sym),
            &self.enum_variants,
            &mut visited,
        )
    }

    /// Every synthesised record struct, in emission order.
    pub(crate) fn record_structs(&self) -> &[RecordStruct] {
        &self.record_structs
    }

    /// Is `name` the Rust name of some user enum this program declares?
    ///
    /// A thin accessor over [`Self::enum_names`] so callers outside this
    /// module never need to know that map's internal key shape.
    pub(crate) fn contains_type_name(&self, name: &str) -> bool {
        self.enum_names.values().any(|n| n == name)
    }

    /// Fail closed if any synthesised [`RecordStruct`]'s ALREADY-CHOSEN name
    /// (i.e. after [`unique_struct_name`]'s existing intra-category
    /// collision bumping, which stays the right tool for two record shapes
    /// that coincidentally camel-case to the same base) collides with an
    /// enum name, a function name, or a caller-supplied `mod_ident`.
    ///
    /// **Why this check exists (design doc §2.2).** `RecordStruct` and
    /// `EnumDef` both render as Rust
    /// `struct`/`enum` items and share Rust's TYPE namespace (`mod` items
    /// share it too — the same namespace `mod_ident`'s collision gate
    /// polices). Today's single-file backend never cross-checks a record
    /// struct's name against [`Self::enum_names`] — a collision there
    /// currently surfaces as a loud `cargo`-time `E0428`, never a silent
    /// mis-emit. Under a future flat glob-reexport barrel splitting `EnumDef`s
    /// and `RecordStruct`s across different files, Rust's name-resolution
    /// precedence (local definition wins over a glob `use`) would turn that
    /// SAME collision into a SILENT SHADOW instead of a loud error. This gate
    /// closes that gap now, before any file-splitting code exists, so "the
    /// flat namespace is sound" is true by construction rather than by an
    /// untested assumption.
    ///
    /// `func_names` is included for defense-in-depth even though the
    /// value/type namespace split means a record-struct/func collision is
    /// not strictly load-bearing today — cheap, and it stops relying on an
    /// implicit "func-name casing convention never collides" invariant a
    /// future `naming.rs` change could silently violate.
    ///
    /// Fails closed with `Diagnostic::Name::DuplicateValue` — mirroring
    /// [`crate::rust_file::assert_mod_idents_unique`]'s own choice for
    /// `mod_ident` collisions (mirror, not auto-rename, for a namespace-wide
    /// collision), rather than [`unique_struct_name`]'s intra-category
    /// auto-suffix behaviour.
    pub(crate) fn assert_record_structs_disjoint_from_type_namespace(
        &self,
        mod_idents: &BTreeSet<String>,
    ) -> DResult<()> {
        for rec in &self.record_structs {
            let collides = self.contains_type_name(&rec.name)
                || self.func_names.values().any(|n| n == &rec.name)
                || mod_idents.contains(&rec.name);
            if collides {
                return Err(Diagnostic::Name {
                    span: Span::DUMMY,
                    msg: NameError::DuplicateValue {
                        name: rec.name.clone().into_boxed_str(),
                        first: Span::DUMMY,
                    },
                });
            }
        }
        Ok(())
    }

    /// Render a record TYPE at a USE SITE to its Rust spelling, keyed by its
    /// field-name set: the bare struct name for a monomorphic shape (`RecXY`),
    /// or the struct instantiated at concrete type
    /// arguments for a generic shape (`RecValue<i64>`).
    ///
    /// `generics` is the enclosing function's generic scope: a use-site field
    /// type may itself be an [`IrType::Generic`] (a parametric signature passing
    /// the record through, `wrap : a -> { value : a }`), in which case the
    /// argument renders as that function's Rust generic (`RecValue<T1>`).
    ///
    /// The prepass collected every `IrType::Record` reachable from a signature,
    /// so a miss here is an internal invariant violation (IPE-I0204).
    fn render_record_use(
        &self,
        fields: &BTreeMap<Symbol, IrType>,
        generics: emit_types::GenericScope,
    ) -> DResult<String> {
        let mut key = Vec::with_capacity(fields.len());
        for sym in fields.keys() {
            key.push(self.resolve_ident(*sym)?.to_owned());
        }
        key.sort();
        let rec = self.record_struct_by_key(&key)?;
        if rec.type_params.is_empty() {
            // Monomorphic shape: the bare struct name.
            return Ok(rec.name.clone());
        }
        // Generic shape: match the use-site field types against the struct's
        // template to recover one concrete type per generic parameter, then
        // render each (through the ambient scope) as a turbofish-free arg list.
        let mut by_name: BTreeMap<&str, &IrType> = BTreeMap::new();
        for (sym, ty) in fields {
            by_name.insert(self.resolve_ident(*sym)?, ty);
        }
        let mut subst: BTreeMap<Symbol, IrType> = BTreeMap::new();
        for (field_name, template_ty) in &rec.fields {
            let use_ty =
                by_name
                    .get(field_name.as_str())
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::EmitCtx::render_record_use",
                        detail: format!(
                            "use-site record is missing field `{field_name}` present in the \
                         synthesised struct template"
                        ),
                    })?;
            match_template(template_ty, use_ty, &mut subst)?;
        }
        let mut args = Vec::with_capacity(rec.type_params.len());
        for param in &rec.type_params {
            let arg_ty = subst.get(param).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::EmitCtx::render_record_use",
                detail: format!(
                    "generic record parameter symbol {} was not pinned by any use-site \
                     field; the use site does not instantiate the struct template",
                    param.as_raw()
                ),
            })?;
            args.push(emit_types::render_type(self, arg_ty, generics)?);
        }
        Ok(format!("{}<{}>", rec.name, args.join(", ")))
    }

    /// The Rust struct name for a record LITERAL, keyed by its field names.
    ///
    /// A miss means the literal's shape never appeared in a signature — a
    /// lowerer-contract violation (IPE-I0204), surfaced rather than mis-emitted.
    fn record_name_for_literal(&self, field_names: &[String]) -> DResult<&str> {
        let mut key = field_names.to_vec();
        key.sort();
        self.record_name_by_key(&key)
    }

    /// Is a synthesised struct registered for this (unsorted) field-name set?
    ///
    /// Used by [`emit_expr::emit_record`]'s `HttpRequest` name-shape shortcut
    /// to defer to the lowerer's authoritative, TYPE-AWARE decision before
    /// falling back to its own field-NAME-only heuristic: `ipe_lower`'s
    /// `ir_type_from_ty` (consulted transitively via
    /// `Lowerer::collect_records_in_ty`, which populates `module.records`,
    /// which THIS registry is built from — see `collect_record_shapes`
    /// above) folds a genuinely `HttpRequest`-shaped value to the opaque
    /// `IrType::HttpRequest`, which never reaches `collect_record_shapes` as
    /// an `IrType::Record` — so a REAL `HttpRequest` literal never gets an
    /// entry here. Conversely, a record that merely shares the 7 canonical
    /// field NAMES with unrelated field TYPES (e.g. all-`Int`) is correctly
    /// classified as a plain record by the (now type-aware) lowerer, so it
    /// DOES get a registered struct here — checking this registry FIRST lets
    /// `emit_record` use that correctly-typed struct instead of mislabelling
    /// the literal `HttpRequest` by name alone.
    fn has_record_struct_for(&self, field_names: &[String]) -> bool {
        let mut key = field_names.to_vec();
        key.sort();
        self.record_by_fieldset.contains_key(&key)
    }

    /// Resolve a (sorted) field-name set to its synthesised struct name.
    fn record_name_by_key(&self, key: &[String]) -> DResult<&str> {
        Ok(self.record_struct_by_key(key)?.name.as_str())
    }

    /// Resolve a (sorted) field-name set to its synthesised [`RecordStruct`].
    fn record_struct_by_key(&self, key: &[String]) -> DResult<&RecordStruct> {
        self.record_by_fieldset
            .get(key)
            .and_then(|i| self.record_structs.get(*i))
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::EmitCtx::record_name",
                detail: format!(
                    "no synthesised struct for record shape {{{}}}; the lowerer must \
                     surface every record type it constructs in a signature",
                    key.join(", ")
                ),
            })
    }

    /// Resolve a symbol that will be emitted as a Rust identifier, rejecting an
    /// absent *or* empty resolution. The lowerer is contracted never to hand the
    /// backend a dangling or empty-intended value/variant/param symbol, so a
    /// failure here is an internal invariant violation (IPE-I0201) — surfaced as
    /// a [`Diagnostic::CompilerBug`] rather than silently emitting an empty (and
    /// uncompilable) Rust identifier.
    fn resolve_ident(&self, sym: Symbol) -> DResult<&str> {
        match self.interner.resolve(sym) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(Diagnostic::CompilerBug {
                where_: "backend.dangling_symbol",
                detail: format!(
                    "value/variant symbol {} resolved to an empty or absent identifier",
                    sym.as_raw()
                ),
            }),
        }
    }

    /// Resolve a symbol to the Rust identifier to emit for it: checked for
    /// emptiness ([`Self::resolve_ident`]) and then mangled if it collides with
    /// a Rust keyword ([`naming::mangle_reserved`]). Used for every emitted
    /// value/variant/param name.
    pub(crate) fn emit_ident(&self, sym: Symbol) -> DResult<String> {
        Ok(naming::mangle_reserved(self.resolve_ident(sym)?.to_owned()))
    }

    /// is this the `Ipe.Cache.Cache` opaque handle type — home
    /// `["Std", "Cache"]`, name `Cache`, and NOT a user-declared enum of the
    /// same name (absent from `enum_names`)? Backed by the runtime
    /// `IpeCacheHandle`; the render/ctor/pattern paths route there.
    pub(crate) fn is_cache_handle_type(&self, home: &ModPath, ty: Symbol) -> bool {
        self.interner.resolve(ty) == Some("Cache")
            && !self.enum_names.contains_key(&(home.clone(), ty))
            && matches!(
                home.0.as_slice(),
                [a, b] if self.interner.resolve(*a) == Some("Ipe")
                    && self.interner.resolve(*b) == Some("Cache")
            )
    }

    pub(crate) fn enum_name(&self, home: &ModPath, ty: Symbol) -> DResult<&str> {
        // `StreamId` is a builtin opaque Http.Stream type backed by the runtime
        // struct `IpeStreamId`.  It has no synthetic `EnumDef` injection (unlike
        // `SqlValue`), so it is not in `enum_names`; we route it here instead.
        // `ChunkEvent` is handled analogously via a dedicated arm in `render_type`.
        if home.0.is_empty() && matches!(self.interner.resolve(ty), Some("StreamId")) {
            // SAFETY: this literal has 'static lifetime; the returned &str
            // is valid for the duration of the emit pass.
            return Ok("IpeStreamId");
        }
        // `Ipe.Cache.Cache` → the non-generic runtime enum `IpeCacheHandle`
        // (its `EnumDef` is suppressed in `ipe_lower`, so it is absent from
        // `enum_names`). The type-position render drops the phantom `k`/`v` args
        // via a dedicated `render_type` arm before reaching here.
        if self.is_cache_handle_type(home, ty) {
            return Ok("IpeCacheHandle");
        }
        self.enum_names
            .get(&(home.clone(), ty))
            .map(String::as_str)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::EmitCtx::enum_name",
                detail: format!("no Rust name for enum type symbol {}", ty.as_raw()),
            })
    }

    /// `true` when `(home, ty)` is a `Ipe.WebSocket` ADT bridged to a
    /// runtime enum (`WebSocketMessage` / `CloseCode`) — its Ipê decl is
    /// suppressed because the runtime already defines the type. See
    /// [`websocket_bridge_rust_name`].
    pub(crate) fn is_websocket_bridged_enum(&self, home: &ModPath, ty: Symbol) -> DResult<bool> {
        let name = resolve_sym(self.interner, ty)?;
        let home_segs = home
            .0
            .iter()
            .map(|s| resolve_sym(self.interner, *s))
            .collect::<DResult<Vec<&str>>>()?;
        Ok(websocket_bridge_rust_name(&home_segs, name).is_some())
    }

    /// The runtime enum path for a built-in constructor's type, or `None` for a
    /// user-declared enum. `Maybe` / `Result` are not user `type` declarations —
    /// their constructors (`Just` / `Nothing` / `Ok` / `Err`) are Prelude
    /// built-ins backed by the runtime's `IpeMaybe` / `IpeResult`, whose variant
    /// names match Ipê's verbatim. A `Some` result steers constructor and pattern
    /// emission to the runtime type (no user-enum field-boxing lookup applies, as
    /// neither is self-recursive).
    pub(crate) fn builtin_runtime_enum(&self, home: &ModPath, ty: Symbol) -> Option<&'static str> {
        // A declared user enum always wins: real Ipê cannot name a `type` `Maybe`
        // or `Result` (canonicalisation rejects shadowing a built-in), so a
        // program-level enum carrying that symbol is a distinct, user-owned type
        // and must route to its own emitted enum, not the runtime shortcut.
        if self.enum_names.contains_key(&(home.clone(), ty)) {
            return None;
        }
        match self.interner.resolve(ty) {
            Some("Maybe") => Some("IpeMaybe"),
            Some("Result") => Some("IpeResult"),
            // `Order` is backed by `IpeOrder` — the `#[repr(u8)]` enum
            // from the runtime crate, in scope via `pub use ipe_runtime::*`.
            // Constructor emission: `IpeOrder::LT` / `IpeOrder::EQ` / `IpeOrder::GT`.
            Some("Order") => Some("IpeOrder"),
            // `ChunkEvent` — the builtin `Ipe.Http.Stream` chunk event enum
            // backed by `ipe_runtime::http_stream::ChunkEvent<IpeError>`.
            // Constructor names match Ipê's verbatim: `Chunk` / `Done` / `Errored`.
            // NOTE: This returns the bare name "ChunkEvent" (without generic args)
            // so that pattern paths emit `ChunkEvent::Chunk(...)`, not
            // `ChunkEvent<IpeError>::Chunk(...)` (invalid Rust syntax).
            // Type-position rendering adds the `<IpeError>` via a special arm in
            // `emit_types::render_type` BEFORE the general `ctx.enum_name` path.
            Some("ChunkEvent") => Some("ChunkEvent"),
            // `Error` is backed by `IpeError` — a single
            // tuple-variant enum whose constructor shares the type's name
            // (`enum_variants[(Prelude, error)] = [error]`, set in
            // `ipe_lower`), so this emits `IpeError::Error(kind, info)` via
            // the SAME path `Maybe`/`Result` use above.
            Some("Error") => Some("IpeError"),
            // `ErrorKind` is backed by `IpeErrorKind` (#repr(u8), mirrors
            // `Order`/`IpeOrder`). Constructor emission: `IpeErrorKind::Io` /
            // `::Network` / etc.
            Some("ErrorKind") => Some("IpeErrorKind"),
            // `ErrorDetails` is backed by `IpeErrorDetails`. Constructor names
            // match Ipê source verbatim:
            // `FfiPanic` / `TypeMismatch` / `HttpStatus` / `JsonDecode` /
            // `Custom`.
            Some("ErrorDetails") => Some("IpeErrorDetails"),
            // `Ipe.Cache.Cache` is backed by the non-generic runtime enum
            // `IpeCacheHandle { Cache(i64) }`. Its `EnumDef` is suppressed in
            // `ipe_lower` (no `enum_names` entry, so the guard above lets this
            // fire), so the `Cache` ctor + `case … of Cache raw` pattern route
            // to `IpeCacheHandle::Cache`. A user's own `type Cache …` DOES get an
            // `EnumDef` (registered in `enum_names`) and short-circuits above.
            Some("Cache") => Some("IpeCacheHandle"),
            // `Ipe.Email.EmailProvider` is backed by the runtime enum
            // `ipe_runtime::email::EmailProvider` (variant names `Resend`/`Ses`/
            // `SendGrid`/`Smtp` match the Ipê ctors verbatim). Its `EnumDef` is
            // suppressed in `ipe_lower` (no `enum_names` entry, so the guard
            // above lets this fire), so `Resend "k"` / `Ses cfg` construct the
            // runtime variants and `case p of Resend k -> …` matches them. In
            // scope via `pub use ipe_runtime::email::*`. A user's own
            // `type EmailProvider` DOES get an `EnumDef` and short-circuits above.
            Some("EmailProvider") => Some("EmailProvider"),
            _ => None,
        }
    }

    fn func_name(&self, id: FuncId) -> DResult<&str> {
        self.func_names
            .get(&id)
            .map(String::as_str)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::EmitCtx::func_name",
                detail: format!("no Rust name for function id {}", id.as_raw()),
            })
    }
}

/// Walk a type, recording every distinct CLOSED record shape it contains
/// (recursing through tuples and nested records). A shape is keyed by its sorted
/// field-name set; the value accumulates each DISTINCT `(field name, type)` list
/// observed for that set, in first-occurrence order.
///
/// One set can legitimately carry several entries — a generic template
/// (`{ value : a }`) plus concrete instantiations (`{ value : Int }`). The later
/// [`canonicalise_shape`] pass reconciles them into one struct. Storing every
/// distinct occurrence (rather than rejecting the second) is what makes the
/// generic-plus-concrete merge representable.
fn collect_record_shapes(
    interner: &Interner,
    ty: &IrType,
    shapes: &mut BTreeMap<Vec<String>, ShapeOccurrences>,
) -> DResult<()> {
    match ty {
        IrType::Tuple(elems) => {
            for elem in elems {
                collect_record_shapes(interner, elem, shapes)?;
            }
        }
        IrType::Record(map) => {
            for field_ty in map.values() {
                collect_record_shapes(interner, field_ty, shapes)?;
            }
            let mut fields: Vec<(String, IrType)> = Vec::with_capacity(map.len());
            for (sym, field_ty) in map {
                fields.push((resolve_sym(interner, *sym)?.to_owned(), field_ty.clone()));
            }
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            let key: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            let entry = shapes.entry(key).or_default();
            if !entry.contains(&fields) {
                entry.push(fields);
            }
        }
        // A function type (`Fun` / `Arc`-carried `SharedFun` / curried
        // `FnOnceChain`) contributes no struct of its own, but its param/return
        // types may carry record shapes (e.g. a callback over a record).
        IrType::Fun(params, ret)
        | IrType::SharedFun(params, ret)
        | IrType::FnOnceChain(params, ret) => {
            for param in params {
                collect_record_shapes(interner, param, shapes)?;
            }
            collect_record_shapes(interner, ret, shapes)?;
        }
        IrType::Enum { args, .. } => {
            // An enum carries no struct of its own, but its type arguments may
            // (e.g. `Maybe { x : Int }`).
            for arg in args {
                collect_record_shapes(interner, arg, shapes)?;
            }
        }
        // `Maybe a` / `List a` / `Set a` carry no struct of their own, but their
        // element type may (`Maybe { x : Int }`).
        IrType::Maybe(elem) | IrType::List(elem) | IrType::Set(elem) => {
            collect_record_shapes(interner, elem, shapes)?;
        }
        // `Result e a` / `Dict k v` — descend into both element types.
        IrType::Result(a, b) | IrType::Dict(a, b) => {
            collect_record_shapes(interner, a, shapes)?;
            collect_record_shapes(interner, b, shapes)?;
        }
        // `Decoder<T>`, `IpeTask<E,A>`, `IpeCmd<M>`, `IpeSub<M>` are opaque
        // aliases; descend into the inner type for any nested record shape.
        IrType::Decoder(inner)
        | IrType::Task(inner)
        | IrType::Cmd(inner)
        | IrType::Sub(inner) => {
            collect_record_shapes(interner, inner, shapes)?;
        }
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        // The opaque Db connection pool handle carries no record shape.
        | IrType::Db
        // Opaque server types carry no record shapes of their own.
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` is an opaque handle — no record shape.
        | IrType::StreamWriter
        // `HttpRequest` is an opaque handle — no record shape.
        | IrType::HttpRequest
        // `WsHandle` / `WsServerCfg` are opaque handles — no record shape.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        // A generic type variable carries no concrete record shape of its own.
        | IrType::Generic(_)
        // nullary plain types (`Length`, `Color`, …) and the opaque live
        // request handle carry no record shapes of their own.
        | IrType::UiPlain(_)
        | IrType::WebReq
        // `Order` (LT/EQ/GT) is a primitive leaf — no record shape.
        // `Decimal` is a Copy newtype — no record shape.
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        // Nominal error-payload leaves (SEAL fix) — monomorphic
        // runtime structs, same classification as `Error`/`ErrorDetails`.
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        // `SqlFragment` is an opaque query-building value — no
        // record shape.
        // `Secret` is an opaque sealed string wrapper — no
        // record shape.
        | IrType::SqlFragment
        | IrType::Secret
        | IrType::Path
        // Cache config / stats + Csv document are folded to nominal runtime
        // structs — no structural record shape to synthesise.
        | IrType::CacheCfg
        | IrType::WebSocketClientCfg
        | IrType::CacheStats
        | IrType::CsvDoc
        // Ipe.Email records + provider ADT are folded to nominal runtime
        // structs — no structural record shape to synthesise.
        | IrType::EmailMessage
        | IrType::EmailAttachment
        | IrType::EmailSesConfig
        | IrType::EmailSmtpConfig
        | IrType::EmailProvider => {}
        // `WebRoute page` is page-parametric — descend in case the page type
        // carries a nested record shape.
        IrType::WebRoute(page) => {
            collect_record_shapes(interner, page, shapes)?;
        }
        // `Ui { ctor, msg }` is a msg-parametric wrapper — descend into
        // `msg` in case it carries a nested record (e.g. `Element { x : Int }`).
        IrType::Ui { msg, .. } => {
            collect_record_shapes(interner, msg, shapes)?;
        }
    }
    Ok(())
}

/// Does `ty` reach the enum type `target` by following type-size edges —
/// tuple elements, record fields, an enum's type arguments, and (memoised by
/// enum name) an enum's own variant payload fields?
///
/// A `Box<…>` and a first-class function value (`Box<dyn Fn …>`) are already a
/// pointer-sized indirection, so traversal does NOT descend through
/// [`IrType::Fun`]; those edges can never make a type infinite-sized.
///
/// `visited` memoises the per-enum *definition* walk (a name-keyed, type-arg-
/// independent set of fields) so a recursive enum is explored once. The
/// use-site type arguments are checked on every visit (NOT memoised), because
/// `Maybe Int` and `Maybe Tree` share the enum name `Maybe` but carry different
/// arguments — memoising under the name would drop the `Tree` argument on the
/// second visit.
fn type_reaches_enum(
    ty: &IrType,
    target: (&ModPath, Symbol),
    enums: &BTreeMap<(ModPath, Symbol), VariantList>,
    visited: &mut BTreeSet<(ModPath, Symbol)>,
) -> bool {
    match ty {
        IrType::Enum { home, name, args } => {
            if (home, *name) == target {
                return true;
            }
            if args
                .iter()
                .any(|a| type_reaches_enum(a, target, enums, visited))
            {
                return true;
            }
            // Descend into this enum's own variant payload fields once.
            let self_key = (home.clone(), *name);
            if visited.insert(self_key.clone())
                && let Some(variants) = enums.get(&self_key)
            {
                return variants
                    .iter()
                    .flat_map(|(_, fields)| fields)
                    .any(|f| type_reaches_enum(f, target, enums, visited));
            }
            false
        }
        IrType::Tuple(elems) => elems
            .iter()
            .any(|e| type_reaches_enum(e, target, enums, visited)),
        IrType::Record(map) => map
            .values()
            .any(|v| type_reaches_enum(v, target, enums, visited)),
        // `Maybe a` / `Result e a` are the runtime's own (already finite) types;
        // a size cycle can still pass THROUGH their element types, so descend.
        IrType::Maybe(elem) | IrType::List(elem) => type_reaches_enum(elem, target, enums, visited),
        IrType::Result(err, ok) => {
            type_reaches_enum(err, target, enums, visited)
                || type_reaches_enum(ok, target, enums, visited)
        }
        // `Dict k v` / `Set a` are heap-allocated (pointer-sized); they cannot
        // participate in an infinite-size cycle. Recurse into element types for
        // completeness (a Dict/Set whose element type reaches the target enum is
        // still finite because the backing HashMap/BTreeSet is a heap pointer).
        IrType::Dict(k, v) => {
            type_reaches_enum(k, target, enums, visited)
                || type_reaches_enum(v, target, enums, visited)
        }
        IrType::Set(a) => type_reaches_enum(a, target, enums, visited),
        // `Decoder<T>` / `Task<A>` / `IpeCmd<M>` / `IpeSub<M>` are all heap-
        // allocated; descend into their inner types for completeness.
        IrType::Decoder(inner) | IrType::Task(inner) | IrType::Cmd(inner) | IrType::Sub(inner) => {
            type_reaches_enum(inner, target, enums, visited)
        }
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        // `Db` is a pointer-sized opaque handle (connection pool Arc); it cannot
        // participate in an infinite-size cycle.
        | IrType::Db
        // Opaque server types are pointer-sized — they cannot be part of an
        // infinite-size cycle.
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` is a pointer-sized opaque handle — no size cycle.
        | IrType::StreamWriter
        // `HttpRequest` is a pointer-sized opaque handle — no size cycle.
        | IrType::HttpRequest
        // `WsHandle` / `WsServerCfg` are opaque handles — no size cycle.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        | IrType::Fun(_, _)
        // The promoted `Arc<dyn Fn>` carrier is pointer-sized — no size cycle.
        | IrType::SharedFun(_, _)
        // A curried `FnOnce` chain is the same boxed-trait-object shape as
        // `Fun` — pointer-sized, no size-cycle risk.
        | IrType::FnOnceChain(_, _)
        | IrType::Generic(_)
        // nullary plain types and the opaque live request handle are
        // pointer-sized — they cannot form an infinite-size cycle.
        | IrType::UiPlain(_)
        | IrType::WebReq
        // `Order` is a primitive value — no cycle risk.
        // `Decimal` is a Copy newtype — no cycle risk.
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        // Nominal error-payload leaves (SEAL fix) — monomorphic
        // runtime structs, same classification as `Error`/`ErrorDetails`.
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        // `SqlFragment` is a heap-backed struct (String +
        // Vec<SqlParam>) — no size-cycle risk.
        // `Secret` is a heap-backed newtype (String) — no
        // size-cycle risk.
        | IrType::SqlFragment
        | IrType::Secret
        | IrType::Path
        // Cache config / stats + Csv document are monomorphic runtime structs
        // — no reachable enum edge to `target`.
        | IrType::CacheCfg
        | IrType::WebSocketClientCfg
        | IrType::CacheStats
        | IrType::CsvDoc
        // Ipe.Email records + provider ADT are monomorphic runtime types —
        // no reachable enum edge to `target`.
        | IrType::EmailMessage
        | IrType::EmailAttachment
        | IrType::EmailSesConfig
        | IrType::EmailSmtpConfig
        | IrType::EmailProvider => false,
        // `Route<Page>` stores its `not_found`/built pages by value — a page
        // type reaching `target` through a route is a genuine size edge.
        IrType::WebRoute(page) => type_reaches_enum(page, target, enums, visited),
        // `Ui { ctor, msg }` — descend into `msg`.
        IrType::Ui { msg, .. } => type_reaches_enum(msg, target, enums, visited),
    }
}

/// Does this type contain an [`IrType::Generic`] anywhere (a field that is — or
/// structurally carries — a type variable)?
fn contains_generic(ty: &IrType) -> bool {
    match ty {
        IrType::Generic(_) => true,
        IrType::Tuple(elems) => elems.iter().any(contains_generic),
        IrType::Record(map) => map.values().any(contains_generic),
        IrType::Fun(params, ret)
        | IrType::SharedFun(params, ret)
        | IrType::FnOnceChain(params, ret) => {
            params.iter().any(contains_generic) || contains_generic(ret)
        }
        IrType::Enum { args, .. } => args.iter().any(contains_generic),
        IrType::Maybe(elem) | IrType::List(elem) => contains_generic(elem),
        IrType::Result(err, ok) => contains_generic(err) || contains_generic(ok),
        IrType::Dict(k, v) => contains_generic(k) || contains_generic(v),
        IrType::Set(a) => contains_generic(a),
        IrType::Decoder(inner) | IrType::Task(inner) | IrType::Cmd(inner) | IrType::Sub(inner) => {
            contains_generic(inner)
        }
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        // `Db` is an opaque monomorphic handle — no generic parameters.
        | IrType::Db
        // Opaque server types are monomorphic — no generic parameters.
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` is monomorphic — no generic parameters.
        | IrType::StreamWriter
        // `HttpRequest` is monomorphic — no generic parameters.
        | IrType::HttpRequest
        // `WsHandle` / `WsServerCfg` are monomorphic — no generic parameters.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        // nullary plain types and the opaque live request handle are
        // monomorphic.
        | IrType::UiPlain(_)
        | IrType::WebReq
        // `Order` is monomorphic — no generic parameters.
        // `Decimal` is monomorphic — no generic parameters.
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        // Nominal error-payload leaves (SEAL fix) — monomorphic
        // runtime structs, same classification as `Error`/`ErrorDetails`.
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        // `SqlFragment` is monomorphic — no generic parameters.
        // `Secret` is monomorphic — no generic parameters.
        | IrType::SqlFragment
        | IrType::Secret
        | IrType::Path
        // Cache config / stats + Csv document are monomorphic — no generic
        // parameters.
        | IrType::CacheCfg
        | IrType::WebSocketClientCfg
        | IrType::CacheStats
        | IrType::CsvDoc
        // Ipe.Email records + provider ADT are monomorphic — no generic
        // parameters.
        | IrType::EmailMessage
        | IrType::EmailAttachment
        | IrType::EmailSesConfig
        | IrType::EmailSmtpConfig
        | IrType::EmailProvider => false,
        // `WebRoute page` is parametric on `page`; check if it carries a
        // generic.
        IrType::WebRoute(page) => contains_generic(page),
        // `Ui { ctor, msg }` is parametric on `msg`; check if `msg` carries
        // a generic.
        IrType::Ui { msg, .. } => contains_generic(msg),
    }
}

/// Collect the distinct [`IrType::Generic`] symbols in `ty`, appending each (in
/// first-occurrence order) to `out` if not already present.
fn collect_generics(ty: &IrType, out: &mut Vec<Symbol>) {
    match ty {
        IrType::Generic(s) => {
            if !out.contains(s) {
                out.push(*s);
            }
        }
        IrType::Tuple(elems) => {
            for e in elems {
                collect_generics(e, out);
            }
        }
        IrType::Record(map) => {
            for v in map.values() {
                collect_generics(v, out);
            }
        }
        IrType::Fun(params, ret)
        | IrType::SharedFun(params, ret)
        | IrType::FnOnceChain(params, ret) => {
            for p in params {
                collect_generics(p, out);
            }
            collect_generics(ret, out);
        }
        IrType::Enum { args, .. } => {
            for a in args {
                collect_generics(a, out);
            }
        }
        IrType::Maybe(elem) | IrType::List(elem) => collect_generics(elem, out),
        IrType::Result(err, ok) => {
            collect_generics(err, out);
            collect_generics(ok, out);
        }
        IrType::Dict(k, v) => {
            collect_generics(k, out);
            collect_generics(v, out);
        }
        IrType::Set(a) => collect_generics(a, out),
        IrType::Decoder(inner) | IrType::Task(inner) | IrType::Cmd(inner) | IrType::Sub(inner) => {
            collect_generics(inner, out);
        }
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        // `Db` is monomorphic — no generic parameters to collect.
        | IrType::Db
        // Opaque server types are monomorphic.
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` is monomorphic — no generics to collect.
        | IrType::StreamWriter
        // `HttpRequest` is monomorphic — no generics to collect.
        | IrType::HttpRequest
        // `WsHandle` / `WsServerCfg` are monomorphic — no generics to collect.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        // nullary plain types and the opaque live request handle
        // contribute no generics.
        | IrType::UiPlain(_)
        | IrType::WebReq
        // `Order` is monomorphic — no generics to collect.
        // `Decimal` is monomorphic — no generics to collect.
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        // Nominal error-payload leaves (SEAL fix) — monomorphic
        // runtime structs, same classification as `Error`/`ErrorDetails`.
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        // `SqlFragment` is monomorphic — no generics to collect.
        // `Secret` is monomorphic — no generics to collect.
        | IrType::SqlFragment
        | IrType::Secret
        | IrType::Path
        // Cache config / stats + Csv document are monomorphic — no generics to
        // collect.
        | IrType::CacheCfg
        | IrType::WebSocketClientCfg
        | IrType::CacheStats
        | IrType::CsvDoc
        // Ipe.Email records + provider ADT are monomorphic — no generics.
        | IrType::EmailMessage
        | IrType::EmailAttachment
        | IrType::EmailSesConfig
        | IrType::EmailSmtpConfig
        | IrType::EmailProvider => {}
        // `WebRoute page` may carry generic parameters through `page`.
        IrType::WebRoute(page) => collect_generics(page, out),
        // `Ui { ctor, msg }` may carry generic parameters through `msg`.
        IrType::Ui { msg, .. } => collect_generics(msg, out),
    }
}

/// A position-canonical rendering of a field-shape: every [`IrType::Generic`]
/// symbol is replaced by its first-occurrence index, so two alpha-equivalent
/// templates (`{ value : a }` and `{ value : b }`) render the same string and a
/// non-equivalent one (`{ x : a, y : a }` vs `{ x : a, y : b }`) does not. Used
/// only for equality, never emitted.
fn skeleton_key(fields: &[(String, IrType)]) -> String {
    let mut idx: BTreeMap<Symbol, usize> = BTreeMap::new();
    let mut out = String::new();
    for (name, ty) in fields {
        out.push_str(name);
        out.push(':');
        skeleton_ty(ty, &mut idx, &mut out);
        out.push(';');
    }
    out
}

fn skeleton_ty(ty: &IrType, idx: &mut BTreeMap<Symbol, usize>, out: &mut String) {
    match ty {
        IrType::Generic(s) => {
            let next = idx.len();
            let n = *idx.entry(*s).or_insert(next);
            out.push('G');
            out.push_str(&n.to_string());
        }
        IrType::Tuple(elems) => {
            out.push('(');
            for e in elems {
                skeleton_ty(e, idx, out);
                out.push(',');
            }
            out.push(')');
        }
        IrType::Record(map) => {
            out.push('{');
            for (k, v) in map {
                out.push_str(&k.as_raw().to_string());
                out.push(':');
                skeleton_ty(v, idx, out);
                out.push(',');
            }
            out.push('}');
        }
        IrType::Fun(params, ret) => {
            out.push_str("fn(");
            for p in params {
                skeleton_ty(p, idx, out);
                out.push(',');
            }
            out.push_str(")->");
            skeleton_ty(ret, idx, out);
        }
        IrType::Enum { home, name, args } => {
            // Key by the enum's nominal identity (home + symbol) plus its
            // (possibly generic) type args, so `Maybe a` and `Maybe Int`
            // skeletonise distinctly while `Maybe a` and `Maybe b`
            // (alpha-equivalent) coincide. Home is included so two same-short-named
            // types from different modules do not conflate record shapes.
            out.push('E');
            for seg in &home.0 {
                out.push_str(&seg.as_raw().to_string());
                out.push('.');
            }
            out.push_str(&name.as_raw().to_string());
            out.push('<');
            for a in args {
                skeleton_ty(a, idx, out);
                out.push(',');
            }
            out.push('>');
        }
        IrType::Dict(k, v) => {
            out.push_str("Dict<");
            skeleton_ty(k, idx, out);
            out.push(',');
            skeleton_ty(v, idx, out);
            out.push('>');
        }
        IrType::Set(a) => {
            out.push_str("Set<");
            skeleton_ty(a, idx, out);
            out.push('>');
        }
        // `IpeCmd<M>` / `IpeSub<M>` carry a message type parameter; recurse so
        // `IpeCmd<G0>` and `IpeCmd<G1>` (alpha-equivalent) get the same key.
        IrType::Cmd(inner) => {
            out.push_str("Cmd<");
            skeleton_ty(inner, idx, out);
            out.push('>');
        }
        IrType::Sub(inner) => {
            out.push_str("Sub<");
            skeleton_ty(inner, idx, out);
            out.push('>');
        }
        // Scalar / leaf types (Int / Bool / …): their `Debug` form is a stable,
        // generic-free discriminator — exactly what a skeleton needs.
        other => {
            use core::fmt::Write as _;
            // Writing to a `String` is infallible; the `Result` is discarded.
            let _ = write!(out, "{other:?}");
        }
    }
}

/// Match a struct-template type against a USE-SITE type, recording in `subst`
/// the concrete (or generic-in-the-enclosing-function) type each template
/// [`IrType::Generic`] binds to. A template `Generic` binds any use-site type
/// (consistently — a symbol seen twice must bind the same type); every other
/// node must structurally agree.
///
/// A mismatch means a use site that does not instantiate the struct template —
/// an upstream-contract violation surfaced as a [`Diagnostic::CompilerBug`]
/// (IPE-I0205), never a silent mis-emit.
#[allow(clippy::too_many_lines)]
fn match_template(
    template: &IrType,
    concrete: &IrType,
    subst: &mut BTreeMap<Symbol, IrType>,
) -> DResult<()> {
    let mismatch = || Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::match_template",
        detail: format!(
            "use-site record type does not instantiate the synthesised struct template \
             (template {template:?} vs use site {concrete:?})"
        ),
    };
    match template {
        IrType::Generic(s) => match subst.get(s) {
            Some(prev) if prev != concrete => Err(Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::match_template",
                detail: format!(
                    "generic parameter symbol {} is bound to two distinct types at one use \
                     site ({prev:?} and {concrete:?})",
                    s.as_raw()
                ),
            }),
            Some(_) => Ok(()),
            None => {
                subst.insert(*s, concrete.clone());
                Ok(())
            }
        },
        IrType::Tuple(ts) => match concrete {
            IrType::Tuple(cs) if cs.len() == ts.len() => {
                for (t, c) in ts.iter().zip(cs.iter()) {
                    match_template(t, c, subst)?;
                }
                Ok(())
            }
            _ => Err(mismatch()),
        },
        IrType::Record(tm) => match concrete {
            IrType::Record(cm) if tm.len() == cm.len() => {
                for ((tk, tv), (ck, cv)) in tm.iter().zip(cm.iter()) {
                    if tk != ck {
                        return Err(mismatch());
                    }
                    match_template(tv, cv, subst)?;
                }
                Ok(())
            }
            _ => Err(mismatch()),
        },
        IrType::Fun(tp, tr) => match concrete {
            IrType::Fun(cp, cr) if tp.len() == cp.len() => {
                for (t, c) in tp.iter().zip(cp.iter()) {
                    match_template(t, c, subst)?;
                }
                match_template(tr, cr, subst)
            }
            _ => Err(mismatch()),
        },
        // Same structural-shape matching as `Fun` — the promoted `Arc<dyn Fn>`
        // carrier reconciles only against another `SharedFun` (a `Box`-carried
        // `Fun` is a distinct Rust type).
        IrType::SharedFun(tp, tr) => match concrete {
            IrType::SharedFun(cp, cr) if tp.len() == cp.len() => {
                for (t, c) in tp.iter().zip(cp.iter()) {
                    match_template(t, c, subst)?;
                }
                match_template(tr, cr, subst)
            }
            _ => Err(mismatch()),
        },
        // Same structural-shape matching as `Fun` (same arity-checked
        // parameter list plus return-type recursion) — a curried `FnOnce`
        // chain template reconciles only against another `FnOnceChain`.
        IrType::FnOnceChain(tp, tr) => match concrete {
            IrType::FnOnceChain(cp, cr) if tp.len() == cp.len() => {
                for (t, c) in tp.iter().zip(cp.iter()) {
                    match_template(t, c, subst)?;
                }
                match_template(tr, cr, subst)
            }
            _ => Err(mismatch()),
        },
        IrType::Enum {
            home: th,
            name: tn,
            args: ta,
        } => match concrete {
            // Nominal identity is (home, name): a template enum reconciles with a
            // concrete enum only when BOTH match, so two same-short-named types
            // from different modules never cross-reconcile.
            IrType::Enum {
                home: ch,
                name: cn,
                args: ca,
            } if th == ch && tn == cn && ta.len() == ca.len() => {
                for (t, c) in ta.iter().zip(ca.iter()) {
                    match_template(t, c, subst)?;
                }
                Ok(())
            }
            _ => Err(mismatch()),
        },
        IrType::Maybe(te) => match concrete {
            IrType::Maybe(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::List(te) => match concrete {
            IrType::List(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::Result(terr, tok) => match concrete {
            IrType::Result(cerr, cok) => {
                match_template(terr, cerr, subst)?;
                match_template(tok, cok, subst)
            }
            _ => Err(mismatch()),
        },
        IrType::Dict(tk, tv) => match concrete {
            IrType::Dict(ck, cv) => {
                match_template(tk, ck, subst)?;
                match_template(tv, cv, subst)
            }
            _ => Err(mismatch()),
        },
        IrType::Set(te) => match concrete {
            IrType::Set(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::Decoder(te) => match concrete {
            IrType::Decoder(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::Task(te) => match concrete {
            IrType::Task(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::Cmd(te) => match concrete {
            IrType::Cmd(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::Sub(te) => match concrete {
            IrType::Sub(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        // A concrete leaf must equal the use-site leaf exactly.
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        // `Db` is a monomorphic opaque handle; its template and concrete forms
        // must be identical (both `IrType::Db`).
        | IrType::Db
        // Opaque server types are monomorphic leaf types.
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` is a monomorphic opaque handle.
        | IrType::StreamWriter
        // `HttpRequest` is a monomorphic opaque handle.
        | IrType::HttpRequest
        // `WsHandle` / `WsServerCfg` are monomorphic opaque handles.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        // nullary plain types (`Length`, `Color`, …) and the opaque live
        // request handle are monomorphic — must equal exactly.
        | IrType::UiPlain(_)
        | IrType::WebReq
        // `Order` is a monomorphic leaf — must equal exactly.
        // `Decimal` is a monomorphic leaf — must equal exactly.
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        // Nominal error-payload leaves (SEAL fix) — monomorphic
        // runtime structs, same classification as `Error`/`ErrorDetails`.
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        // `SqlFragment` is a monomorphic opaque leaf.
        // `Secret` is a monomorphic opaque leaf.
        | IrType::SqlFragment
        | IrType::Secret
        | IrType::Path
        // Cache config / stats + Csv document are monomorphic runtime-struct
        // leaves.
        | IrType::CacheCfg
        | IrType::WebSocketClientCfg
        | IrType::CacheStats
        | IrType::CsvDoc
        // Ipe.Email records + provider ADT are monomorphic runtime-type leaves.
        | IrType::EmailMessage
        | IrType::EmailAttachment
        | IrType::EmailSesConfig
        | IrType::EmailSmtpConfig
        | IrType::EmailProvider => {
            if template == concrete {
                Ok(())
            } else {
                Err(mismatch())
            }
        }
        // `WebRoute page` is parametric on `page` — recurse into the page
        // argument.
        IrType::WebRoute(tp) => match concrete {
            IrType::WebRoute(cp) => match_template(tp, cp, subst),
            _ => Err(mismatch()),
        },
        // `Ui { ctor, msg }` is parametric on `msg`; match the ctor tag
        // then recurse into the msg argument.
        IrType::Ui { ctor: tc, msg: tm } => match concrete {
            IrType::Ui { ctor: cc, msg: cm } if tc == cc => match_template(tm, cm, subst),
            _ => Err(mismatch()),
        },
    }
}

/// Reconcile every distinct field-type shape observed for one field-name set
/// into a single synthesised struct: its canonical `(field name, type)` template
/// and its generic parameter list.
///
/// * No occurrence carries a type variable → a MONOMORPHIC struct (empty
///   parameter list). All occurrences must be identical; a second, differing
///   concrete shape is a "two types for one field set" upstream-contract
///   violation, rejected as IPE-I0204.
/// * At least one occurrence is generic → a GENERIC struct. Every generic
///   occurrence must be alpha-equivalent (same [`skeleton_key`]); the first is
///   the canonical template, whose generic symbols name the parameters in
///   first-occurrence field order. Every concrete occurrence must be a valid
///   instantiation of that template (checked via [`match_template`]).
fn canonicalise_shape(key: &[String], occurrences: &[RecordFields]) -> DResult<CanonicalShape> {
    let first = occurrences.first().ok_or_else(|| Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::canonicalise_shape",
        detail: format!(
            "record field set {{{}}} has no collected shape",
            key.join(", ")
        ),
    })?;

    let is_generic = |fields: &[(String, IrType)]| fields.iter().any(|(_, t)| contains_generic(t));

    // Pick the canonical generic template (the first generic occurrence), if any.
    let template = occurrences.iter().find(|f| is_generic(f));

    let Some(template) = template else {
        // All-concrete: exactly one shape per field set.
        for other in occurrences.iter().skip(1) {
            if other != first {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::canonicalise_shape",
                    detail: format!(
                        "record field set {{{}}} maps to two distinct field-type shapes; \
                         closed records assume one type per field set",
                        key.join(", ")
                    ),
                });
            }
        }
        return Ok((first.clone(), Vec::new()));
    };

    let template_skeleton = skeleton_key(template);
    let mut type_params: Vec<Symbol> = Vec::new();
    for (_, ty) in template {
        collect_generics(ty, &mut type_params);
    }

    for occ in occurrences {
        if is_generic(occ) {
            // Every generic occurrence must be alpha-equivalent to the template.
            if skeleton_key(occ) != template_skeleton {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::canonicalise_shape",
                    detail: format!(
                        "record field set {{{}}} maps to two non-alpha-equivalent generic \
                         shapes",
                        key.join(", ")
                    ),
                });
            }
        } else {
            // Every concrete occurrence must instantiate the template.
            let mut subst: BTreeMap<Symbol, IrType> = BTreeMap::new();
            for ((_, tv), (_, cv)) in template.iter().zip(occ.iter()) {
                match_template(tv, cv, &mut subst)?;
            }
        }
    }

    Ok((template.clone(), type_params))
}

/// Return `base` if unused, else the first `base_<n>` (n ≥ 2) that is free,
/// recording the chosen name in `used`. Deterministic given a deterministic call
/// order; guarantees a collision-free struct name even when two distinct field
/// sets camel-case to the same base.
fn unique_struct_name(base: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}_{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

/// Resolve a symbol that the IR guarantees came from `interner`. A `None` here
/// means the IR carried a symbol from a different interner — an internal
/// invariant violation, surfaced as a [`Diagnostic::CompilerBug`] rather than a
/// silent empty name.
fn resolve_sym(interner: &Interner, sym: Symbol) -> DResult<&str> {
    interner
        .resolve(sym)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::resolve_sym",
            detail: format!("symbol {} not present in interner", sym.as_raw()),
        })
}

/// The runtime enum name a `Ipe.WebSocket` ADT is BRIDGED to, or `None`
/// for any other type.
///
/// `WebSocketMessage` and `CloseCode` are declared in the stdlib with
/// constructors, but the `sub_subscribe_ws_{message,close}` runtime fns
/// take/produce `ipe_runtime::ws_client::{WsClientMessage, WsCloseCode}` — whose
/// variant names (`Text`/`Binary`, `Normal`/`GoingAway`/`UnsupportedData`/
/// `InternalError`/`Custom`) and field types match the Ipê ADTs 1:1. Emitting
/// the enum name AS the runtime type makes every constructor / pattern / typed
/// param resolve to the runtime enum (so the user's `toMsg` closure has the
/// exact `Fn(WsClientMessage) -> M` shape the runtime fn requires), and the Ipê
/// enum's own decl is suppressed in [`crate::emit_types::emit_enum`]. Keyed on
/// the type's HOME module so a user's unrelated `type CloseCode` never folds.
pub(crate) fn websocket_bridge_rust_name(home_segs: &[&str], name: &str) -> Option<&'static str> {
    if home_segs != ["Ipe", "WebSocket"] {
        return None;
    }
    match name {
        "WebSocketMessage" => Some("WsClientMessage"),
        "CloseCode" => Some("WsCloseCode"),
        _ => None,
    }
}

#[cfg(test)]
mod record_struct_namespace_tests {
    use ipe_ir::{EnumDef, Module, Variant};

    use super::*;

    /// A record shape whose synthesised name collides with a user enum's Rust
    /// name must fail closed, not silently coexist (today's single-file backend
    /// never cross-checks `RecordStruct` names against `enum_names`/`func_names`
    /// — a gap that only manifests once file-splitting lets a local declaration
    /// silently shadow a glob-reexported one).
    ///
    /// `naming::enum_name(&["Rec"], "XY")` folds to `"RecXY"` — module home
    /// `["Rec"]`, type name `"XY"`. Separately, a record literal with fields
    /// `{x, y}` (`naming::record_struct_name(&["x", "y"])`, asserted
    /// byte-for-byte "`RecXY`" by `naming::record_struct_names_from_field_sets`)
    /// synthesises a struct with the SAME Rust name — a real, constructible
    /// collision, not a hedged placeholder.
    #[test]
    fn record_struct_colliding_with_enum_name_fails_closed() -> DResult<()> {
        let mut interner = Interner::new();
        let rec_mod = interner.intern("Rec")?;
        let xy_ty = interner.intern("XY")?;
        let a_ctor = interner.intern("A")?;
        let b_ctor = interner.intern("B")?;
        let x_field = interner.intern("x")?;
        let y_field = interner.intern("y")?;

        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![rec_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: xy_ty,
                    home: ModPath(vec![rec_mod]),
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: a_ctor,
                            fields: vec![],
                        },
                        Variant {
                            name: b_ctor,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![],
                entry: None,
                records: vec![IrType::Record(BTreeMap::from([
                    (x_field, IrType::Int),
                    (y_field, IrType::Int),
                ]))],
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
                uses_env_public: false,
                uses_debug: false,
                uses_ffi: false,
            }],
        };

        // `EmitCtx::build` itself must still succeed today — the
        // record-vs-enum collision is a PRE-EXISTING gap `build` does not
        // check; this task adds the check as a SEPARATE, explicit gate.
        let ctx = EmitCtx::build(
            &interner,
            &program,
            DbDriver::Sqlite,
            None,
            ipe_ir::Target::Native,
            Vec::new(),
            false,
        )?;
        assert_eq!(ctx.record_structs().len(), 1);
        assert_eq!(
            ctx.record_structs().first().map(|r| r.name.as_str()),
            Some("RecXY")
        );
        assert!(ctx.contains_type_name("RecXY"));

        let result = ctx.assert_record_structs_disjoint_from_type_namespace(&BTreeSet::new());
        assert!(
            matches!(
                result,
                Err(Diagnostic::Name {
                    msg: NameError::DuplicateValue { .. },
                    ..
                })
            ),
            "expected a fail-closed DuplicateValue collision, got {result:?}"
        );
        Ok(())
    }

    /// The common case (no record/enum name collision) must stay unaffected
    /// — the new gate is purely additive.
    #[test]
    fn disjoint_record_structs_do_not_fail_closed() -> DResult<()> {
        let mut interner = Interner::new();
        let main_mod = interner.intern("Main")?;
        let msg_ty = interner.intern("Msg")?;
        let increment = interner.intern("Increment")?;
        let a_field = interner.intern("a")?;
        let b_field = interner.intern("b")?;

        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: msg_ty,
                    home: ModPath(vec![main_mod]),
                    type_params: vec![],
                    variants: vec![Variant {
                        name: increment,
                        fields: vec![],
                    }],
                })],
                funcs: vec![],
                entry: None,
                records: vec![IrType::Record(BTreeMap::from([
                    (a_field, IrType::Int),
                    (b_field, IrType::Int),
                ]))],
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
                uses_env_public: false,
                uses_debug: false,
                uses_ffi: false,
            }],
        };

        let ctx = EmitCtx::build(
            &interner,
            &program,
            DbDriver::Sqlite,
            None,
            ipe_ir::Target::Native,
            Vec::new(),
            false,
        )?;
        ctx.assert_record_structs_disjoint_from_type_namespace(&BTreeSet::new())
    }
}
