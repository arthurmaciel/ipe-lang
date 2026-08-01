//! Generic-aware record-struct synthesis for the Rust backend.
//!
//! Extends the closed-record pipeline so a record shape whose field types are
//! type variables ([`IrType::Generic`]) synthesises a GENERIC Rust struct:
//!
//! * `{ value : a }` → `pub struct RecValue<T1> { value: T1 }` with a generic
//!   `IpeStringify` impl bounded `T1: IpeStringify + std::fmt::Debug`,
//! * a function `wrap : a -> { value : a }` renders its signature with the
//!   struct instantiated at the function's own generic (`RecValue<T1>`),
//! * a same-field-set concrete record (`{ value : Int }`) deduplicates onto the
//!   ONE generic struct and renders `RecValue<i64>` at the use site,
//! * monomorphic records emit no `<..>` clause.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/ipe/out/ipe` compiles + runs the
//! equivalent program
//!
//! ```text
//! wrap : a -> { value : a }
//! wrap x = { value = x }
//! unwrap : { value : a } -> a
//! unwrap r = r.value
//! main = Io.println (String.fromInt (unwrap (wrap 42)))
//! ```
//!
//! to stdout `42\n`, exit 0 (hand-verified in a temp dir). The `end_to_end_*`
//! test (gated on `IPE_E2E=1`) drives the hand-built IR through the Rust backend,
//! builds the emitted crate, and asserts the identical `42`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::{DResult, Diagnostic};
use ipe_intern::{Interner, Symbol};
use ipe_ir::{
    BoundSet, CallPin, Callee, Expr, Func, FuncId, IrType, KernelFn, ModPath, Module, OnFormKind,
    Program,
};

/// A single-module program.
fn program(name: Symbol, funcs: Vec<Func>, records: Vec<IrType>, entry: Option<FuncId>) -> Program {
    Program {
        modules: vec![Module {
            name: ModPath(vec![name]),
            types: vec![],
            funcs,
            entry,
            records,
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
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
            where_: "generic_records test",
            detail: "no src/main.rs".to_owned(),
        })
}

/// A `{ value : <ty> }` record type.
fn value_record(value: Symbol, ty: IrType) -> IrType {
    let mut fields = BTreeMap::new();
    fields.insert(value, ty);
    IrType::Record(fields)
}

/// The canonical generic-record program: `wrap`/`unwrap` over `{ value : a }`,
/// using DISTINCT source type-variable spellings (`a` in `wrap`, `b` in
/// `unwrap`) to exercise alpha-equivalent template selection.
fn wrap_unwrap_program(interner: &mut Interner) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let value = interner.intern("value")?;
    let a = interner.intern("a")?;
    let b = interner.intern("b")?;
    let x = interner.intern("x")?;
    let r = interner.intern("r")?;
    let wrap = interner.intern("wrap")?;
    let unwrap = interner.intern("unwrap")?;
    let main = interner.intern("main")?;

    // wrap : a -> { value : a } ; wrap x = { value = x }
    let wrap_fn = Func {
        id: FuncId::from_raw(0),
        name: wrap,
        home: ModPath(vec![]),
        type_params: vec![(a, BoundSet::UNBOUNDED)],
        params: vec![(x, IrType::Generic(a))],
        ret: value_record(value, IrType::Generic(a)),
        body: Expr::Record(vec![(value, Expr::Var(x))]),
    };
    // unwrap : { value : b } -> b ; unwrap r = r.value
    let unwrap_fn = Func {
        id: FuncId::from_raw(1),
        name: unwrap,
        home: ModPath(vec![]),
        type_params: vec![(b, BoundSet::UNBOUNDED)],
        params: vec![(r, value_record(value, IrType::Generic(b)))],
        ret: IrType::Generic(b),
        body: Expr::Access {
            record: Box::new(Expr::Var(r)),
            field: value,
            field_ty: IrType::Generic(b),
        },
    };
    // main = Io.println (String.fromInt (unwrap (wrap 42)))
    let main_fn = Func {
        id: FuncId::from_raw(2),
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
                    callee: Callee::Func(FuncId::from_raw(1)),
                    args: vec![Expr::Call {
                        callee: Callee::Func(FuncId::from_raw(0)),
                        args: vec![Expr::Int(42)],
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
        vec![wrap_fn, unwrap_fn, main_fn],
        vec![],
        Some(FuncId::from_raw(2)),
    ))
}

#[test]
fn synthesises_generic_struct_and_signatures() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = wrap_unwrap_program(&mut interner)?;
    let out = emit(&interner, &prog)?;

    // The struct is generic over T1, the `value` field typed by it.
    assert!(
        out.contains(
            "#[derive(Clone, Debug, PartialEq)]\npub struct RecValue<T1> {\n    value: T1,\n}"
        ),
        "generic struct definition missing or wrong shape:\n{out}"
    );
    // Generic IpeStringify impl, bounded so the autoref dispatch resolves; rustfmt wraps format!.
    assert!(
        out.contains(
            "impl<T1: IpeStringify + std::fmt::Debug> IpeStringify for RecValue<T1> {\n    \
             fn ipe_show(&self) -> String {\n        \
             format!(\n            \"{{{}}}\",\n            \
             (&ipe_runtime::stringify::Wrap(&self.value)).dispatch()\n        )"
        ),
        "generic IpeStringify impl missing or wrong:\n{out}"
    );
    // Exactly one struct definition (both generic occurrences dedup to one).
    assert_eq!(
        out.matches("pub struct RecValue").count(),
        1,
        "alpha-equivalent generic shapes must dedup to one struct:\n{out}"
    );
    // `wrap` renders the struct at its own generic.
    assert!(
        out.contains("pub fn main_wrap<T1: Clone>(x: T1) -> RecValue<T1> {"),
        "wrap signature not rendered with generic struct:\n{out}"
    );
    // `unwrap`'s param renders `RecValue<T1>` despite its source var being `b`.
    assert!(
        out.contains("pub fn main_unwrap<T1: Clone>(r: RecValue<T1>) -> T1 {"),
        "unwrap signature not rendered with generic struct:\n{out}"
    );
    // Literal construction resolves to the struct name; Rust infers the arg.
    assert!(
        out.contains("RecValue { value: x }"),
        "generic record literal not emitted as struct literal:\n{out}"
    );
    Ok(())
}

#[test]
fn merges_generic_and_concrete_field_set() -> DResult<()> {
    // A generic `{ value : a }` (in `wrap`) and a concrete `{ value : Int }`
    // (in a monomorphic `mkBox`) share the field set `{ value }`. They must
    // collapse to the ONE generic struct, and the concrete use site renders the
    // struct instantiated at `i64`.
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let value = interner.intern("value")?;
    let a = interner.intern("a")?;
    let x = interner.intern("x")?;
    let n = interner.intern("n")?;
    let wrap = interner.intern("wrap")?;
    let mk_box = interner.intern("mkBox")?;

    // wrap : a -> { value : a }
    let wrap_fn = Func {
        id: FuncId::from_raw(0),
        name: wrap,
        home: ModPath(vec![]),
        type_params: vec![(a, BoundSet::UNBOUNDED)],
        params: vec![(x, IrType::Generic(a))],
        ret: value_record(value, IrType::Generic(a)),
        body: Expr::Record(vec![(value, Expr::Var(x))]),
    };
    // mkBox : Int -> { value : Int } ; mkBox n = { value = n }
    let mk_box_fn = Func {
        id: FuncId::from_raw(1),
        name: mk_box,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(n, IrType::Int)],
        ret: value_record(value, IrType::Int),
        body: Expr::Record(vec![(value, Expr::Var(n))]),
    };
    let prog = program(main_mod, vec![wrap_fn, mk_box_fn], vec![], None);
    let out = emit(&interner, &prog)?;

    // ONE struct, generic.
    assert_eq!(
        out.matches("pub struct RecValue").count(),
        1,
        "generic + concrete same-field-set must dedup to one struct:\n{out}"
    );
    assert!(
        out.contains("pub struct RecValue<T1> {"),
        "merged struct must be the generic one:\n{out}"
    );
    // The concrete monomorphic function instantiates the struct at i64.
    assert!(
        out.contains("pub fn main_mk_box(n: i64) -> RecValue<i64> {"),
        "concrete use site must render the struct instantiated at i64:\n{out}"
    );
    Ok(())
}

#[test]
fn two_type_parameter_record() -> DResult<()> {
    // `{ first : a, second : b }` synthesises a two-parameter struct, params in
    // field-name order (first → T1, second → T2).
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let first = interner.intern("first")?;
    let second = interner.intern("second")?;
    let a = interner.intern("a")?;
    let b = interner.intern("b")?;
    let x = interner.intern("x")?;
    let y = interner.intern("y")?;
    let pair = interner.intern("pair")?;

    let mut rec_fields = BTreeMap::new();
    rec_fields.insert(first, IrType::Generic(a));
    rec_fields.insert(second, IrType::Generic(b));
    let rec = IrType::Record(rec_fields);

    // pair : a -> b -> { first : a, second : b }
    let pair_fn = Func {
        id: FuncId::from_raw(0),
        name: pair,
        home: ModPath(vec![]),
        type_params: vec![(a, BoundSet::UNBOUNDED), (b, BoundSet::UNBOUNDED)],
        params: vec![(x, IrType::Generic(a)), (y, IrType::Generic(b))],
        ret: rec,
        body: Expr::Record(vec![(first, Expr::Var(x)), (second, Expr::Var(y))]),
    };
    let prog = program(main_mod, vec![pair_fn], vec![], None);
    let out = emit(&interner, &prog)?;

    assert!(
        out.contains("pub struct RecFirstSecond<T1, T2> {\n    first: T1,\n    second: T2,\n}"),
        "two-parameter struct missing or wrong shape:\n{out}"
    );
    // rustfmt line-wraps the long impl header before `for`.
    assert!(
        out.contains(
            "impl<T1: IpeStringify + std::fmt::Debug, T2: IpeStringify + std::fmt::Debug> IpeStringify\n    \
             for RecFirstSecond<T1, T2>\n{"
        ),
        "two-parameter IpeStringify impl missing or wrong:\n{out}"
    );
    assert!(
        out.contains(
            "pub fn main_pair<T1: Clone, T2: Clone>(x: T1, y: T2) -> RecFirstSecond<T1, T2> {"
        ),
        "two-parameter signature not rendered:\n{out}"
    );
    Ok(())
}

#[test]
fn monomorphic_record_stays_byte_identical() -> DResult<()> {
    // A field set with NO generic occurrence anywhere emits the monomorphic
    // struct (no `<..>` clause, concrete field type).
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let value = interner.intern("value")?;
    let n = interner.intern("n")?;
    let mk_box = interner.intern("mkBox")?;

    let mk_box_fn = Func {
        id: FuncId::from_raw(0),
        name: mk_box,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![(n, IrType::Int)],
        ret: value_record(value, IrType::Int),
        body: Expr::Record(vec![(value, Expr::Var(n))]),
    };
    let prog = program(main_mod, vec![mk_box_fn], vec![], None);
    let out = emit(&interner, &prog)?;

    assert!(
        out.contains(
            "#[derive(Clone, Debug, PartialEq)]\npub struct RecValue {\n    value: i64,\n}"
        ),
        "monomorphic struct must stay byte-identical (no generic clause):\n{out}"
    );
    assert!(
        out.contains("impl IpeStringify for RecValue {"),
        "monomorphic IpeStringify impl must carry no generic clause:\n{out}"
    );
    assert!(
        out.contains("pub fn main_mk_box(n: i64) -> RecValue {"),
        "monomorphic signature must render the bare struct name:\n{out}"
    );
    Ok(())
}

/// Full spine: build the generic-record IR, emit the project, vendor the runtime,
/// `cargo build`, run, and assert the program prints `42` — the value the Go
/// backend produces. Gated on `IPE_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_forty_two() -> DResult<()> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }

    let mut interner = Interner::new();
    let prog = wrap_unwrap_program(&mut interner)?;
    let emitted = RustBackend::new(&interner).emit(&prog)?;

    let out = std::env::temp_dir().join("ipe_backend_generic_records_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).map_err(|e| io_bug(&src, &e))?;

    let runtime = resolve_runtime().ok_or_else(|| Diagnostic::CompilerBug {
        where_: "generic_records e2e",
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
        "emitted generic-record project must build: {status:?}"
    );

    let bin = out.join("target").join("debug").join("ipe-app");
    let output = Command::new(&bin).output().map_err(|e| io_bug(&bin, &e))?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "42\n",
        "generic-record program prints 42 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    let _ = std::fs::remove_dir_all(out.join("target"));
    Ok(())
}

/// Wrap a filesystem error as a `CompilerBug` (the E2E test's `DResult` currency).
fn io_bug(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "generic_records e2e io",
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
