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
//!   * The emitted `hydrate` glue references the SAME Rust type its
//!     `main_from_hydration_state` signature names — the structural `RecCount`
//!     record-alias struct, never the nonexistent `MainHydrationState`
//!     convention name (issue #224). The compile-level SEAL (the emitted crate
//!     actually `cargo check`s for wasm) lives in `ipe-cli`'s
//!     `wasm_target_gate` integration test.
//!   * A hydrate-mode program with no `fromHydrationState` projection emits no
//!     `hydrate` glue (no island type to name).

use ipe_backend_rust::RustBackend;
use ipe_diagnostics::DResult;
use ipe_intern::Interner;
use ipe_ir::{EnumDef, Expr, Func, FuncId, IrType, ModPath, Module, Program, TypeDef, Variant};
use std::collections::BTreeMap;

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
            uses_debug: false,
            uses_ffi: false,
            uses_async_runtime: false,
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
            uses_debug: false,
            uses_ffi: false,
            uses_async_runtime: false,
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

/// Build a wasm-hydrate `Program` whose `Main` module declares a
/// `fromHydrationState : { count : Int } -> { count : Int }` projection — the
/// shape of the `examples/wasm/hydration` example. The `HydrationState` type is
/// a RECORD ALIAS, so the backend synthesises it as a structural `RecCount`
/// struct (NOT `MainHydrationState`) — the exact regression of issue #224.
fn hydrate_program_with_record_projection(interner: &mut Interner) -> DResult<Program> {
    let main = interner.intern("Main")?;
    let count = interner.intern("count")?;
    let hs = interner.intern("hs")?;
    let from_hs = interner.intern("fromHydrationState")?;
    let rec = || IrType::Record(BTreeMap::from([(count, IrType::Int)]));
    let from_hs_fn = Func {
        id: FuncId::from_raw(1),
        name: from_hs,
        home: ModPath(vec![main]),
        type_params: vec![],
        row_params: vec![],
        params: vec![(hs, rec())],
        ret: rec(),
        // Body is irrelevant to the glue↔type-name contract this test proves;
        // the signature's parameter type is what the glue must agree with.
        body: Expr::Var(hs),
    };
    Ok(Program {
        modules: vec![Module {
            name: ModPath(vec![main]),
            types: vec![],
            funcs: vec![from_hs_fn],
            entry: None,
            records: vec![],
            uses_tea: false,
            uses_server: false,
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
            uses_debug: false,
            uses_ffi: false,
            uses_async_runtime: false,
        }],
    })
}

/// The emitted `hydrate` glue references the SAME Rust type the emitted
/// `main_from_hydration_state` signature names — the structural `RecCount`
/// struct the record-alias emitter produces — never the nonexistent
/// `MainHydrationState` convention name (issue #224). This is the string-level
/// witness of the fix; the compile-level SEAL is
/// `wasm_target_gate::hydrate_glue_type_name_matches_emitted_struct_and_compiles_for_wasm`.
#[test]
fn hydrate_glue_references_the_emitted_record_struct_name() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = hydrate_program_with_record_projection(&mut interner)?;
    let spine = hydrate_backend(&interner).emit_spine(&prog)?;

    assert!(
        spine.contains("pub fn hydrate(model_json: &str)"),
        "hydrate export must be present in wasm_hydrate_mode spine:\n{spine}"
    );
    assert!(
        spine.contains("pub fn ipe_start()"),
        "ipe_start export must still be present:\n{spine}"
    );
    // The record alias `{ count : Int }` is emitted structurally as `RecCount`.
    // The struct is synthesised and the glue names that SAME struct (resolved
    // through the shared `render_type`, not a hardcoded convention name).
    assert!(
        spine.contains("pub struct RecCount"),
        "the `{{ count : Int }}` record alias must be synthesised as `RecCount`:\n{spine}"
    );
    assert!(
        spine.contains("serde_json::from_str::<crate::RecCount>"),
        "the hydrate glue must parse the island JSON as the SAME struct the \
         projection takes (`RecCount`), not the nonexistent `MainHydrationState`:\n{spine}"
    );
    assert!(
        !spine.contains("MainHydrationState"),
        "the glue must not reference the convention type the emitter never \
         produces (issue #224 regression):\n{spine}"
    );
    Ok(())
}

/// A hydrate-mode program with NO `fromHydrationState` projection has no island
/// type to name, so the backend emits only the `ipe_start` entry — never a
/// dangling `hydrate` glue that would reference an absent type. (Parse, don't
/// validate: the glue exists only when its type source exists.)
#[test]
fn no_projection_emits_no_hydrate_glue() -> DResult<()> {
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
            uses_debug: false,
            uses_ffi: false,
            uses_async_runtime: false,
        }],
    };
    let spine = hydrate_backend(&interner).emit_spine(&prog)?;
    assert!(
        spine.contains("pub fn ipe_start()"),
        "ipe_start export must still be present:\n{spine}"
    );
    assert!(
        !spine.contains("pub fn hydrate(model_json: &str)"),
        "no `fromHydrationState` projection ⇒ no hydrate glue (it would reference \
         an absent island type):\n{spine}"
    );
    Ok(())
}
