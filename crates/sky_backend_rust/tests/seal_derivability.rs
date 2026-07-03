//! #87 seal regression: a user record / enum that stores a first-class function
//! or an opaque wrapper (`Decoder` / `Cmd` / …) must NOT reach the unconditional
//! `#[derive(Clone, Debug, PartialEq)]`.
//!
//! Before the seal, `emit_types` stamped that derive on every generated struct /
//! enum, so a well-typed record like `{ dec : Decoder Int }` made `skyc` exit 0
//! and then `cargo build` fail (E0277 `Clone` / E0369 `==` / E0599
//! `SkyStringify`) — the highest-bar exit-0-then-cargo-fail breach. The gate is
//! now a function of a computed derivability flag ([`sky_ir::ir_type_is_derivable`]
//! together with the backend fixpoint), so the derive can never appear on a
//! non-derivable type by construction.
//!
//! These tests assert the emitted *text*. A non-derivable type carries no
//! `#[derive(` attribute and renders its non-derivable fields as a `<fn>`
//! placeholder in `SkyStringify`; a normal all-primitive record still carries
//! the full derive and the autoref dispatch (byte-shape unchanged).

use std::collections::BTreeMap;

use sky_backend::Backend;
use sky_backend_rust::RustBackend;
use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::{Interner, Symbol};
use sky_ir::{EnumDef, Expr, Func, FuncId, IrType, ModPath, Module, Program, TypeDef, Variant};

fn program(name: Symbol, types: Vec<TypeDef>, funcs: Vec<Func>) -> Program {
    Program {
        modules: vec![Module {
            name: ModPath(vec![name]),
            types,
            funcs,
            entry: None,
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_ui: false,
            uses_live: false,
            uses_tui: false,
            uses_webview: false,
        }],
    }
}

fn emit(interner: &Interner, prog: &Program) -> DResult<String> {
    RustBackend::new(interner)
        .emit(prog)?
        .files
        .get("src/main.rs")
        .cloned()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "seal_derivability test",
            detail: "no src/main.rs".to_owned(),
        })
}

/// The emitter only ever writes the full `#[derive(Clone, Debug, PartialEq)]`
/// (optionally `+ serde`) directly above a declaration, or nothing at all. So a
/// non-derivable type is proven by the *absence* of that full-derive line
/// immediately above its `pub struct` / `pub enum`.
fn assert_no_full_derive(src: &str, kind: &str, name: &str) {
    let with_derive = format!("#[derive(Clone, Debug, PartialEq)]\npub {kind} {name}");
    let with_serde = "#[derive(Clone, Debug, PartialEq, serde";
    assert!(
        !src.contains(&with_derive),
        "non-derivable `{name}` must carry NO Clone/Debug/PartialEq derive:\n{src}"
    );
    assert!(
        !(src.contains(with_serde) && src.contains(&format!("pub {kind} {name}"))),
        "non-derivable `{name}` must not gain a serde derive:\n{src}"
    );
}

/// The Rust struct name synthesised for the program's single record shape (the
/// `Rec…` struct), if present.
fn rec_struct_name(src: &str) -> Option<String> {
    src.lines()
        .find(|l| l.trim_start().starts_with("pub struct Rec"))
        .and_then(|l| l.split_whitespace().nth(2))
        .map(|s| s.trim_end_matches('{').trim().to_owned())
}

/// A record `{ dec : Decoder Int, n : Int }` in a signature: non-derivable
/// (Decoder is `Box<dyn Fn>`-backed) → no derive, `<fn>` placeholder for `dec`,
/// autoref dispatch for `n`.
#[test]
fn decoder_field_record_has_no_derive() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let dec = interner.intern("dec")?;
    let n = interner.intern("n")?;
    let par = interner.intern("r")?;
    let f = interner.intern("getN")?;

    let mut fields = BTreeMap::new();
    fields.insert(dec, IrType::Decoder(Box::new(IrType::Int)));
    fields.insert(n, IrType::Int);
    let rec = IrType::Record(fields);

    let func = Func {
        id: FuncId::from_raw(0),
        name: f,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(par, rec)],
        ret: IrType::Int,
        body: Expr::Access {
            record: Box::new(Expr::Var(par)),
            field: n,
        },
    };
    let src = emit(&interner, &program(main_mod, vec![], vec![func]))?;

    assert!(
        src.contains("dec: Decoder<i64>"),
        "decoder field rendered:\n{src}"
    );
    let name = rec_struct_name(&src).unwrap_or_default();
    assert!(!name.is_empty(), "a Rec struct must be synthesised:\n{src}");
    assert_no_full_derive(&src, "struct", &name);
    assert!(
        src.contains("\"<fn>\""),
        "non-derivable field renders `<fn>` placeholder in SkyStringify:\n{src}"
    );
    assert!(
        src.contains("(&sky_runtime::stringify::Wrap(&self.n)).dispatch()"),
        "derivable sibling field keeps dispatch:\n{src}"
    );
    Ok(())
}

/// A normal all-primitive record keeps the full unconditional derive and the
/// dispatch on every field (byte-shape unchanged — no seal regression).
#[test]
fn normal_record_keeps_full_derive() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let x = interner.intern("x")?;
    let y = interner.intern("y")?;
    let par = interner.intern("p")?;
    let f = interner.intern("getX")?;

    let mut fields = BTreeMap::new();
    fields.insert(x, IrType::Int);
    fields.insert(y, IrType::Int);
    let rec = IrType::Record(fields);

    let func = Func {
        id: FuncId::from_raw(0),
        name: f,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(par, rec)],
        ret: IrType::Int,
        body: Expr::Access {
            record: Box::new(Expr::Var(par)),
            field: x,
        },
    };
    let src = emit(&interner, &program(main_mod, vec![], vec![func]))?;
    let name = rec_struct_name(&src).unwrap_or_default();
    assert!(!name.is_empty(), "a Rec struct must be synthesised:\n{src}");
    assert!(
        src.contains(&format!(
            "#[derive(Clone, Debug, PartialEq)]\npub struct {name}"
        )),
        "normal `{name}` must keep the full derive:\n{src}"
    );
    assert!(
        !src.contains("\"<fn>\""),
        "a normal record must not emit any `<fn>` placeholder:\n{src}"
    );
    Ok(())
}

/// An enum with a variant carrying a function payload → non-derivable → no
/// derive, `<fn>` placeholder + `_` binder for the function field.
#[test]
fn enum_with_function_payload_has_no_derive() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let holder = interner.intern("Holder")?;
    let wrap = interner.intern("Wrap")?;
    let empty = interner.intern("Empty")?;

    let def = EnumDef {
        name: holder,
        type_params: vec![],
        variants: vec![
            Variant {
                name: wrap,
                fields: vec![IrType::Fun(vec![IrType::Int], Box::new(IrType::Int))],
            },
            Variant {
                name: empty,
                fields: vec![],
            },
        ],
    };
    let src = emit(&interner, &program(main_mod, vec![TypeDef::Enum(def)], vec![]))?;

    assert!(src.contains("pub enum MainHolder"), "enum emitted:\n{src}");
    assert_no_full_derive(&src, "enum", "MainHolder");
    assert!(
        src.contains("\"<fn>\""),
        "function payload renders `<fn>` placeholder:\n{src}"
    );
    assert!(
        src.contains("MainHolder::Wrap(_)"),
        "function payload binds `_` in the SkyStringify arm:\n{src}"
    );
    Ok(())
}
