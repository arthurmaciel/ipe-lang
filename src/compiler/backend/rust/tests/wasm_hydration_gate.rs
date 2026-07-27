//! M7 gate: `HydrationState` field-type gate (spec:
//! `docs/adr/0042-wasm-client-target.md` §M7 field-type gate).
//!
//! When `wasm_hydrate_mode = true` (set by `[wasm] mode = "hydrate"` in
//! `ipe.toml`), the backend inspects the `HydrationState` type declared in
//! module `Main` and rejects any field whose `IrType` is non-serialisable
//! (server-surface handles, async primitives, function types, etc.).
//!
//! Gate properties proven here:
//!   * A `HydrationState` with a `Secret` field → compile error (the
//!     canonical containment predicate from the spec).
//!   * A `HydrationState` with all-primitive fields → compiles clean.
//!   * A program with NO `HydrationState` type passes the gate (not every
//!     hydrate-mode program declares an explicit alias).
//!   * The gate is a no-op when `wasm_hydrate_mode = false` — the same
//!     `Secret`-fielded type compiles cleanly in non-hydrate native mode.
//!   * The emitted spine contains `pub fn hydrate(model_json: &str)` when
//!     `wasm_hydrate_mode = true`.

use ipe_backend_rust::RustBackend;
use ipe_diagnostics::DResult;
use ipe_intern::Interner;
use ipe_ir::{EnumDef, IrType, ModPath, Module, Program, TypeDef, Variant};

// ── helpers ──────────────────────────────────────────────────────────────────

/// Build a minimal wasm-hydrate-mode `Program` with a `Main` module that
/// contains a single `HydrationState` type whose sole variant carries `fields`.
fn hydrate_program(interner: &mut Interner, fields: Vec<IrType>) -> DResult<Program> {
    let main = interner.intern("Main")?;
    let hs_name = interner.intern("HydrationState")?;
    let var_name = interner.intern("HydrationState")?; // single-ctor alias shape
    let hs_def = EnumDef {
        name: hs_name,
        home: ModPath(vec![main]),
        type_params: vec![],
        variants: vec![Variant {
            name: var_name,
            fields,
        }],
    };
    Ok(Program {
        modules: vec![Module {
            name: ModPath(vec![main]),
            types: vec![TypeDef::Enum(hs_def)],
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
            uses_env_public: false,
            uses_ffi: false,
        }],
    })
}

/// Build the wasm-hydrate-mode `RustBackend`.
const fn hydrate_backend(interner: &Interner) -> RustBackend<'_> {
    RustBackend::new(interner)
        .with_target(ipe_ir::Target::WasmClient)
        .with_wasm_hydrate_mode(true)
}

// ── gate tests ────────────────────────────────────────────────────────────────

/// A `HydrationState` whose sole field is `IrType::Secret` must produce a
/// compile error — `Secret` is the spec's canonical "non-serialisable" example.
#[test]
fn hydration_state_secret_field_is_compile_error() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = hydrate_program(&mut interner, vec![IrType::Secret])?;
    let err = hydrate_backend(&interner)
        .emit_spine(&prog)
        .expect_err("Secret-fielded HydrationState must be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("non-serialisable") || msg.contains("HydrationState"),
        "error should mention non-serialisable or HydrationState: {msg}"
    );
    Ok(())
}

/// A `HydrationState` with a `Db` field must also be rejected.
#[test]
fn hydration_state_db_field_is_compile_error() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = hydrate_program(&mut interner, vec![IrType::Db])?;
    let err = hydrate_backend(&interner)
        .emit_spine(&prog)
        .expect_err("Db-fielded HydrationState must be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("non-serialisable") || msg.contains("HydrationState"),
        "error should mention non-serialisable or HydrationState: {msg}"
    );
    Ok(())
}

/// A `HydrationState` with only `Int` and `String` fields compiles cleanly.
#[test]
fn hydration_state_primitive_fields_compile_clean() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = hydrate_program(&mut interner, vec![IrType::Int, IrType::Str])?;
    hydrate_backend(&interner).emit_spine(&prog).map(|_| ())
}

/// A program with NO `HydrationState` type passes the gate — the presence of
/// the type is optional.
#[test]
fn no_hydration_state_type_passes_gate() -> DResult<()> {
    let mut interner = Interner::new();
    let main = interner.intern("Main")?;
    let prog = Program {
        modules: vec![Module {
            name: ModPath(vec![main]),
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
            uses_env_public: false,
            uses_ffi: false,
        }],
    };
    hydrate_backend(&interner).emit_spine(&prog).map(|_| ())
}

/// The gate is a no-op for native mode: the same `Secret`-fielded
/// `HydrationState` compiles cleanly when `wasm_hydrate_mode = false`.
#[test]
fn gate_is_noop_for_native_mode() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = hydrate_program(&mut interner, vec![IrType::Secret])?;
    // Native mode, no hydrate flag — gate must not fire.
    RustBackend::new(&interner).emit_spine(&prog).map(|_| ())
}

/// The emitted spine contains the `hydrate` wasm-bindgen export when
/// `wasm_hydrate_mode = true`.
#[test]
fn hydrate_export_present_in_spine() -> DResult<()> {
    let mut interner = Interner::new();
    // No HydrationState — just check the export is emitted regardless.
    let main = interner.intern("Main")?;
    let prog = Program {
        modules: vec![Module {
            name: ModPath(vec![main]),
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
            uses_env_public: false,
            uses_ffi: false,
        }],
    };
    let spine = hydrate_backend(&interner).emit_spine(&prog)?;
    assert!(
        spine.contains("pub fn hydrate(model_json: &str)"),
        "hydrate export must be present in wasm_hydrate_mode spine:\n{spine}"
    );
    assert!(
        spine.contains("pub fn ipe_start()"),
        "ipe_start export must still be present:\n{spine}"
    );
    Ok(())
}
