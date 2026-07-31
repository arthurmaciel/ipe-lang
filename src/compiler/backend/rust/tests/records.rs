//! Record synthesis tests for the Rust backend.
//!
//! These exercise the synthesised-struct pipeline that turns closed record
//! shapes into named Rust structs:
//!
//! * one `#[derive(Clone, Debug, PartialEq)]` struct + `IpeStringify` impl per
//!   distinct field-name set, deduplicated across the program,
//! * record literal → struct literal, access → `.field`, update → a
//!   clone-and-reassign block,
//! * nested records,
//! * the upstream-contract guards (a literal whose shape was never declared, and
//!   two record types that share a field set but differ in field types) fail
//!   fast as a `CompilerBug`, never a silent mis-emit.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/ipe/out/ipe` compiles + runs the
//! field-set-equivalent program
//!
//! ```text
//! mk a    = { x = a, y = 2 }
//! bumpX p = { p | x = 5 }
//! getX p  = p.x
//! main    = Io.println (String.fromInt (getX (bumpX (mk 1))))
//! ```
//!
//! to stdout `5\n`, exit 0 (hand-verified in a temp dir). The
//! `end_to_end_*` test (gated on `IPE_E2E=1`) drives the same hand-built IR
//! through the Rust backend, builds the emitted crate, and asserts the identical
//! `5`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::{DResult, Diagnostic};
use ipe_intern::{Interner, Symbol};
use ipe_ir::{
    CallPin, Callee, Expr, Func, FuncId, IrType, KernelFn, ModPath, Module, OnFormKind, Program,
};

/// A single-module program with the given funcs and optional entry.
fn program(name: Symbol, funcs: Vec<Func>, entry: Option<FuncId>) -> Program {
    Program {
        modules: vec![Module {
            name: ModPath(vec![name]),
            types: vec![],
            funcs,
            entry,
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
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
    }
}

fn emit(interner: &Interner, prog: &Program) -> DResult<String> {
    let emitted = RustBackend::new(interner).emit(prog)?;
    emitted
        .files
        .get("src/main.rs")
        .cloned()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "records test",
            detail: "no src/main.rs".to_owned(),
        })
}

/// A `{ x : Int, y : Int }` record type.
fn xy_record(x: Symbol, y: Symbol) -> IrType {
    let mut fields = BTreeMap::new();
    fields.insert(x, IrType::Int);
    fields.insert(y, IrType::Int);
    IrType::Record(fields)
}

/// The canonical trio — `mk` (literal), `bumpX` (update), `getX` (access) — all
/// sharing the `{ x, y }` shape, plus `main` chaining them. Returns the program.
fn record_trio(interner: &mut Interner) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let x = interner.intern("x")?;
    let y = interner.intern("y")?;
    let arg = interner.intern("a")?;
    let par = interner.intern("p")?;
    let mk = interner.intern("mk")?;
    let bump = interner.intern("bumpX")?;
    let getx = interner.intern("getX")?;
    let main = interner.intern("main")?;

    let rec = xy_record(x, y);

    // mk a = { x = a, y = 2 }   (fields sorted by name: x before y)
    let mk_fn = Func {
        id: FuncId::from_raw(0),
        name: mk,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(arg, IrType::Int)],
        ret: rec.clone(),
        body: Expr::Record(vec![(x, Expr::Var(arg)), (y, Expr::Int(2))]),
    };
    // bumpX p = { p | x = 5 }
    let bump_fn = Func {
        id: FuncId::from_raw(1),
        name: bump,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(par, rec.clone())],
        ret: rec.clone(),
        body: Expr::Update {
            record: Box::new(Expr::Var(par)),
            fields: vec![(x, Expr::Int(5))],
        },
    };
    // getX p = p.x
    let getx_fn = Func {
        id: FuncId::from_raw(2),
        name: getx,
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
    // main = Io.println (String.fromInt (getX (bumpX (mk 1))))
    let main_fn = Func {
        id: FuncId::from_raw(3),
        name: main,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![],
        ret: IrType::Task(Box::new(IrType::Unit)),
        body: Expr::Call {
            callee: Callee::Kernel(KernelFn::IoPrintln),
            args: vec![Expr::Call {
                callee: Callee::Kernel(KernelFn::StringFromInt),
                args: vec![Expr::Call {
                    callee: Callee::Func(FuncId::from_raw(2)),
                    args: vec![Expr::Call {
                        callee: Callee::Func(FuncId::from_raw(1)),
                        args: vec![Expr::Call {
                            callee: Callee::Func(FuncId::from_raw(0)),
                            args: vec![Expr::Int(1)],
                            pin: CallPin::None,
                            on_form: OnFormKind::NotForm,
                        }],
                        pin: CallPin::None,
                        on_form: OnFormKind::NotForm,
                    }],
                    pin: CallPin::None,
                    on_form: OnFormKind::NotForm,
                }],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            }],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        },
    };

    Ok(program(
        main_mod,
        vec![mk_fn, bump_fn, getx_fn, main_fn],
        Some(FuncId::from_raw(3)),
    ))
}

#[test]
fn synthesises_struct_literal_access_and_update() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = record_trio(&mut interner)?;
    let out = emit(&interner, &prog)?;

    // One struct definition, derived + field-typed.
    assert!(
        out.contains(
            "#[derive(Clone, Debug, PartialEq)]\npub struct RecXY {\n    x: i64,\n    y: i64,\n}"
        ),
        "struct definition missing or wrong shape:\n{out}"
    );
    // IpeStringify impl mirroring Go `%v` (`{v0 v1}`), in field order.
    // Rustfmt splits the format! call across lines; assert the stable fragments.
    assert!(
        out.contains("impl IpeStringify for RecXY {"),
        "IpeStringify impl missing:\n{out}"
    );
    assert!(
        out.contains("fn ipe_show(&self) -> String {"),
        "IpeStringify ipe_show missing:\n{out}"
    );
    assert!(
        out.contains("\"{{{} {}}}\",")
            && out.contains("(&ipe_runtime::stringify::Wrap(&self.x)).dispatch(),")
            && out.contains("(&ipe_runtime::stringify::Wrap(&self.y)).dispatch()"),
        "IpeStringify format! body wrong:\n{out}"
    );
    // Literal → struct literal.
    assert!(
        out.contains("RecXY { x: a, y: 2 }"),
        "record literal not emitted as struct literal:\n{out}"
    );
    // Update → clone-and-reassign block (no struct name needed).
    // Rustfmt spreads the block across lines; assert the stable key fragments.
    assert!(
        out.contains("let mut __ipe_rec = (p).clone();")
            && out.contains("__ipe_rec.x = 5;")
            && out.contains("__ipe_rec"),
        "record update not emitted as clone-and-reassign:\n{out}"
    );
    // Access → parenthesised `.field`.
    assert!(
        out.contains("(p).x"),
        "record access not emitted as field access:\n{out}"
    );
    // The record type renders as the struct name in signatures.
    assert!(
        out.contains("pub fn main_bump_x(p: RecXY) -> RecXY {"),
        "record-typed signature not rendered with struct name:\n{out}"
    );
    Ok(())
}

#[test]
fn deduplicates_identical_shapes() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = record_trio(&mut interner)?;
    let out = emit(&interner, &prog)?;
    // The `{ x, y }` shape appears in three signatures; exactly one struct.
    let defs = out.matches("pub struct RecXY {").count();
    assert_eq!(defs, 1, "shape must be deduplicated to one struct:\n{out}");
    Ok(())
}

#[test]
fn synthesises_nested_record_structs() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let x = interner.intern("x")?;
    let y = interner.intern("y")?;
    let inner = interner.intern("inner")?;
    let par = interner.intern("p")?;
    let func = interner.intern("f")?;

    // f : { inner : { x : Int, y : Int } } -> { x : Int, y : Int }
    // f p = p.inner   (shape only — the point is collecting BOTH structs)
    let xy = xy_record(x, y);
    let mut outer_fields = BTreeMap::new();
    outer_fields.insert(inner, xy.clone());
    let outer = IrType::Record(outer_fields);

    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: func,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(par, outer)],
        ret: xy.clone(),
        body: Expr::Access {
            record: Box::new(Expr::Var(par)),
            field: inner,
            field_ty: xy,
        },
    };
    let prog = program(main_mod, vec![f_fn], None);
    let out = emit(&interner, &prog)?;

    // Both the outer and inner record shapes synthesise a struct.
    assert!(
        out.contains("pub struct RecXY {"),
        "inner struct missing:\n{out}"
    );
    assert!(
        out.contains("pub struct RecInner {"),
        "outer struct missing:\n{out}"
    );
    // The outer struct's field is typed by the inner struct name.
    assert!(
        out.contains("inner: RecXY,"),
        "nested field not typed by inner struct:\n{out}"
    );
    Ok(())
}

#[test]
fn literal_with_unknown_shape_fails_fast() -> DResult<()> {
    // A record literal whose field set never appears in a signature: the lowerer
    // is contracted to surface every record type it builds, so this is an
    // internal invariant violation — a `CompilerBug`, not a silent mis-emit.
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let x = interner.intern("x")?;
    let func = interner.intern("f")?;

    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: func,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![],
        ret: IrType::Int, // NOT a record type → the literal's shape is uncollected
        body: Expr::Access {
            record: Box::new(Expr::Record(vec![(x, Expr::Int(1))])),
            field: x,
            field_ty: IrType::Int,
        },
    };
    let prog = program(main_mod, vec![f_fn], None);
    let res = emit(&interner, &prog);
    assert!(
        matches!(res, Err(Diagnostic::CompilerBug { .. })),
        "uncollected literal shape must be a CompilerBug, got {res:?}"
    );
    Ok(())
}

#[test]
fn conflicting_field_set_types_fail_fast() -> DResult<()> {
    // Two record types share the field set `{ x }` but differ in field type
    // (`Int` vs `Bool`). Closed records assume one type per field set; this
    // is an upstream-contract violation surfaced as a `CompilerBug`.
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let x = interner.intern("x")?;
    let fsym = interner.intern("f")?;
    let gsym = interner.intern("g")?;
    let par = interner.intern("p")?;
    let qar = interner.intern("q")?;

    let mut int_rec = BTreeMap::new();
    int_rec.insert(x, IrType::Int);
    let mut bool_rec = BTreeMap::new();
    bool_rec.insert(x, IrType::Bool);

    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: fsym,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(par, IrType::Record(int_rec))],
        ret: IrType::Int,
        body: Expr::Int(0),
    };
    let g_fn = Func {
        id: FuncId::from_raw(1),
        name: gsym,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(qar, IrType::Record(bool_rec))],
        ret: IrType::Bool,
        body: Expr::Int(0),
    };
    let prog = program(main_mod, vec![f_fn, g_fn], None);
    let res = emit(&interner, &prog);
    assert!(
        matches!(res, Err(Diagnostic::CompilerBug { .. })),
        "conflicting field-set types must be a CompilerBug, got {res:?}"
    );
    Ok(())
}

/// Full spine: build the canonical record IR, emit the Cargo project, vendor the
/// runtime, `cargo build`, run, and assert the program prints `5` — the value
/// the Go backend produces for the field-set-equivalent program. Gated on
/// `IPE_E2E=1` so the default `cargo test` stays fast and offline.
#[test]
fn end_to_end_builds_and_prints_five() -> DResult<()> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }

    let mut interner = Interner::new();
    let prog = record_trio(&mut interner)?;
    let emitted = RustBackend::new(&interner).emit(&prog)?;

    let out = std::env::temp_dir().join("ipe_backend_records_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).map_err(|e| io_bug(&src, &e))?;

    // Vendor the runtime tree next to the emitted main.rs.
    let runtime = resolve_runtime().ok_or_else(|| Diagnostic::CompilerBug {
        where_: "records e2e",
        detail: "could not locate src/runtime/rust/src/ipe_runtime".to_owned(),
    })?;
    copy_dir(&runtime, &src.join("ipe_runtime"))?;

    let cargo_toml = out.join("Cargo.toml");
    std::fs::write(&cargo_toml, &emitted.cargo_toml).map_err(|e| io_bug(&cargo_toml, &e))?;
    for (rel, contents) in &emitted.files {
        let path = out.join(rel.as_str());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_bug(parent, &e))?;
        }
        std::fs::write(&path, contents).map_err(|e| io_bug(&path, &e))?;
    }

    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", out.join("target"))
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "emitted record project must build: {status:?}"
    );

    let bin = out.join("target").join("debug").join("ipe-app");
    let output = Command::new(&bin).output().map_err(|e| io_bug(&bin, &e))?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "5\n",
        "record program prints 5 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    let _ = std::fs::remove_dir_all(out.join("target"));
    Ok(())
}

/// Wrap a filesystem error as a `CompilerBug` (the E2E test's `DResult` currency).
fn io_bug(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "records e2e io",
        detail: format!("{}: {e}", path.display()),
    }
}

/// Locate the Ipê runtime module tree (`src/runtime/rust/src/ipe_runtime`), via
/// `IPE_RUNTIME_DIR` or an upward search from the current directory.
fn resolve_runtime() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("IPE_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let cwd = std::env::current_dir().ok()?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        for candidate in [
            // In-repo runtime (ipe-lang monorepo).
            dir.join("src").join("runtime").join("rust").join("src"),
            // Legacy: sibling `ipe` checkout.
            dir.join("ipe")
                .join("runtime-rust")
                .join("src")
                .join("ipe_runtime"),
            // Legacy: sibling `runtime-rust` directory.
            dir.join("runtime-rust").join("src").join("ipe_runtime"),
        ] {
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        here = dir.parent();
    }
    None
}

/// Recursively copy the (trusted, bounded-depth) runtime tree into the crate.
fn copy_dir(src: &Path, dst: &Path) -> DResult<()> {
    std::fs::create_dir_all(dst).map_err(|e| io_bug(dst, &e))?;
    for entry in std::fs::read_dir(src).map_err(|e| io_bug(src, &e))? {
        let entry = entry.map_err(|e| io_bug(src, &e))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| io_bug(&from, &e))?;
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| io_bug(&from, &e))?;
        }
    }
    Ok(())
}
