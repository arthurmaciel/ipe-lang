//! seal regression: a user record / enum that stores a first-class function
//! or an opaque wrapper (`Decoder` / `Cmd` / …) must NOT reach the unconditional
//! `#[derive(Clone, Debug, PartialEq)]`.
//!
//! Before the seal, `emit_types` stamped that derive on every generated struct /
//! enum, so a well-typed record like `{ dec : Decoder Int }` made `ipe` exit 0
//! and then `cargo build` fail (E0277 `Clone` / E0369 `==` / E0599
//! `IpeStringify`) — the highest-bar exit-0-then-cargo-fail breach. The gate is
//! now a function of a computed derivability flag ([`ipe_ir::ir_type_is_derivable`]
//! together with the backend fixpoint), so the derive can never appear on a
//! non-derivable type by construction.
//!
//! These tests assert the emitted *text*. A non-derivable type carries no
//! `#[derive(` attribute and renders its non-derivable fields as a `<fn>`
//! placeholder in `IpeStringify`; a normal all-primitive record still carries
//! the full derive and the autoref dispatch (byte-shape unchanged).

use std::collections::BTreeMap;

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::{DResult, Diagnostic};
use ipe_intern::{Interner, Symbol};
use ipe_ir::{
    EnumDef, Expr, Func, FuncId, IrType, ModPath, Module, Program, TypeDef, UiCtor, Variant,
};

fn program(name: Symbol, types: Vec<TypeDef>, funcs: Vec<Func>) -> Program {
    program_with_web(name, types, funcs, false)
}

/// Like [`program`] but lets the test set `uses_web`, so the emitter takes the
/// Ipe.Web serde-derive path (SEAL gate exercise).
fn program_with_web(
    name: Symbol,
    types: Vec<TypeDef>,
    funcs: Vec<Func>,
    uses_web: bool,
) -> Program {
    Program {
        modules: vec![Module {
            name: ModPath(vec![name]),
            types,
            funcs,
            entry: None,
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_ui: uses_web,
            uses_web,
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
    }
}

/// Classify the derive line that immediately precedes the monomorphic
/// `pub struct <name> {` declaration. Returns `(has_cdpeq_only, has_serde)`:
/// a serde-forced struct matches the second form (which is a superset of the
/// first as text), so `has_serde` is checked against the exact serde line.
fn derive_flavours(src: &str, name: &str) -> (bool, bool) {
    let decl = format!("\npub struct {name} {{");
    let cdpeq_only = format!("#[derive(Clone, Debug, PartialEq)]{decl}");
    let with_serde =
        format!("#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]{decl}");
    (src.contains(&cdpeq_only), src.contains(&with_serde))
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
            field_ty: IrType::Int,
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
        "non-derivable field renders `<fn>` placeholder in IpeStringify:\n{src}"
    );
    assert!(
        src.contains("(&ipe_runtime::stringify::Wrap(&self.n)).dispatch()"),
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
            field_ty: IrType::Int,
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
        home: ModPath(vec![]),
    };
    let src = emit(
        &interner,
        &program(main_mod, vec![TypeDef::Enum(def)], vec![]),
    )?;

    assert!(src.contains("pub enum MainHolder"), "enum emitted:\n{src}");
    assert_no_full_derive(&src, "enum", "MainHolder");
    assert!(
        src.contains("\"<fn>\""),
        "function payload renders `<fn>` placeholder:\n{src}"
    );
    assert!(
        src.contains("MainHolder::Wrap(_)"),
        "function payload binds `_` in the IpeStringify arm:\n{src}"
    );
    Ok(())
}

/// seal: in a `Ipe.Web` program, a NON-Model view-helper record that holds
/// an `Html` field is `CDPeq`-supporting (`Html<M>` derives `Clone, Debug,
/// PartialEq`) but NOT serde-supporting (`Html<M>` is not `Serialize`). Gating
/// the serde derive on the `CDPeq` flag (`is_derivable`) would force
/// `#[derive(..., serde::Serialize, serde::Deserialize)]` onto such a record →
/// `ipe` exit 0 then `cargo` E0277. The gate instead reads the per-record
/// serde flag (`is_serde`): the helper keeps its `CDPeq` derive WITHOUT serde,
/// while a sibling all-primitive record still gets the serde derive.
///
/// This is the emit-text half of the seal (fast, no cargo). The cargo-buildable
/// half is proven end-to-end by `ipe`'s `web_e2e::web_html_helper_record_build_only`.
#[test]
fn web_html_helper_record_gets_cdpeq_without_serde() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    // Field symbols. The synthesised struct name is field-name-sorted PascalCase,
    // so `{ body, title }` → `RecBodyTitle` and `{ count }` → `RecCount`.
    let body = interner.intern("body")?;
    let title = interner.intern("title")?;
    let count = interner.intern("count")?;
    let p = interner.intern("p")?;
    let helper_fn = interner.intern("renderSection")?;
    let model_fn = interner.intern("useModel")?;

    // A view-helper record `{ body : Html Int, title : String }` — Html carrier
    // makes it `CDPeq`-but-not-serde. `Html<Int>` (msg = Int) is enough; the serde
    // predicate rejects every `IrType::Ui` carrier regardless of the msg type.
    let mut helper_fields = BTreeMap::new();
    helper_fields.insert(
        body,
        IrType::Ui {
            ctor: UiCtor::Html,
            msg: Box::new(IrType::Int),
        },
    );
    helper_fields.insert(title, IrType::Str);
    let helper_rec = IrType::Record(helper_fields);

    // A plain-data Model record `{ count : Int }` — fully serde.
    let mut model_fields = BTreeMap::new();
    model_fields.insert(count, IrType::Int);
    let model_rec = IrType::Record(model_fields);

    // Two functions, each taking one record shape, so both shapes are synthesised.
    let helper_func = Func {
        id: FuncId::from_raw(0),
        name: helper_fn,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(p, helper_rec)],
        ret: IrType::Str,
        body: Expr::Access {
            record: Box::new(Expr::Var(p)),
            field: title,
            field_ty: IrType::Str,
        },
    };
    let model_func = Func {
        id: FuncId::from_raw(1),
        name: model_fn,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(p, model_rec)],
        ret: IrType::Int,
        body: Expr::Access {
            record: Box::new(Expr::Var(p)),
            field: count,
            field_ty: IrType::Int,
        },
    };

    let prog = program_with_web(main_mod, vec![], vec![helper_func, model_func], true);
    let src = emit(&interner, &prog)?;

    // The helper renders its Html field type — sanity that the shape survived.
    assert!(
        src.contains("body: ipe_runtime::html::Html<i64>"),
        "helper record's Html field rendered:\n{src}"
    );

    // core: the Html-holding helper carries `CDPeq` WITHOUT serde.
    let (helper_cdpeq, helper_serde) = derive_flavours(&src, "RecBodyTitle");
    assert!(
        helper_cdpeq,
        "view-helper record must keep `#[derive(Clone, Debug, PartialEq)]`:\n{src}"
    );
    assert!(
        !helper_serde,
        "view-helper record holding `Html` must NOT be forced to serde (E0277):\n{src}"
    );

    // Regression: a sibling all-primitive record in the same Web program still
    // gets the serde derive (the fix only demotes non-serde records).
    let (_model_cdpeq, model_serde) = derive_flavours(&src, "RecCount");
    assert!(
        model_serde,
        "plain-data record in a Web program must still get serde:\n{src}"
    );
    Ok(())
}
