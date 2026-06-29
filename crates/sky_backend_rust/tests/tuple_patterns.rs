//! Tuple-pattern and unit-value tests for the M3b-1 Rust backend (task
//! M3B1-CORE).
//!
//! These exercise the two IR additions the backend learned to emit:
//!
//! * a TUPLE PATTERN [`Pat::Tuple`] as a constructor-payload sub-pattern — a
//!   variant carrying a tuple field (`type Wrap = MkWrap (Int, Int)`), matched
//!   with `MkWrap (a, b) -> a`, emits the Rust arm `Main::MkWrap((a, b)) => a`,
//!   and the construction `MkWrap (3, 4)` emits `Main::MkWrap((3, 4))`,
//! * the UNIT VALUE [`Expr::Unit`] — Sky's `()` literal — emits the Rust unit
//!   expression `()`, and [`IrType::Unit`] renders as `()`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the
//! shape-equivalent program
//!
//! ```text
//! type alias IntPair = (Int, Int)
//! type Wrap = MkWrap IntPair
//! fstOf w = case w of MkWrap (a, b) -> a
//! main = println (String.fromInt (fstOf (MkWrap (3, 4))))      -- prints 3
//! ```
//!
//! to stdout `3\n`, exit 0 (hand-verified in a temp dir). The `end_to_end_*`
//! test (gated on `SKY_E2E=1`) drives the same hand-built IR through the Rust
//! backend, builds the emitted crate, runs it, and asserts the identical `3` —
//! the soundness-floor regression for a value laundered through a tuple-carrying
//! payload destructured by a tuple pattern.

use std::path::{Path, PathBuf};
use std::process::Command;

use sky_backend::Backend;
use sky_backend_rust::RustBackend;
use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Interner;
use sky_ir::{
    Arm, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, ModPath, Module, Pat, Program,
    TypeDef, Variant,
};

fn emit(interner: &Interner, prog: &Program) -> DResult<String> {
    let emitted = RustBackend::new(interner).emit(prog)?;
    emitted
        .files
        .get("src/main.rs")
        .cloned()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "tuple_patterns test",
            detail: "no src/main.rs".to_owned(),
        })
}

/// The `Wrap` program: a single-variant enum whose payload is a `(Int, Int)`
/// tuple, a `fstOf` that destructures it with a tuple pattern, and a `main` that
/// prints `fstOf (MkWrap (3, 4))`.
fn wrap_program(i: &mut Interner) -> DResult<Program> {
    let main_mod = i.intern("Main")?;
    let wrap = i.intern("Wrap")?;
    let mk_wrap = i.intern("MkWrap")?;
    let fst_of = i.intern("fstOf")?;
    let main = i.intern("main")?;
    let w = i.intern("w")?;
    let a = i.intern("a")?;
    let b = i.intern("b")?;

    let def = EnumDef {
        name: wrap,
        type_params: vec![],
        variants: vec![Variant {
            name: mk_wrap,
            fields: vec![IrType::Tuple(vec![IrType::Int, IrType::Int])],
        }],
    };

    // fstOf w = case w of MkWrap (a, b) -> a
    let arms = vec![Arm {
        pat: Pat::Ctor {
            ty: wrap,
            variant: mk_wrap,
            args: vec![Pat::Tuple(vec![Pat::Var(a), Pat::Var(b)])],
        },
        body: Expr::Var(a),
    }];
    let fst_of_fn = Func {
        id: FuncId::from_raw(0),
        name: fst_of,
        type_params: vec![],
        params: vec![(
            w,
            IrType::Enum {
                name: wrap,
                args: vec![],
            },
        )],
        ret: IrType::Int,
        body: Expr::Match(sky_ir::Match::new(Expr::Var(w), arms, &[mk_wrap])?),
    };

    // main = Log.println (String.fromInt (fstOf (MkWrap (3, 4))))
    let main_fn = Func {
        id: FuncId::from_raw(1),
        name: main,
        type_params: vec![],
        params: vec![],
        ret: IrType::TaskUnit,
        body: Expr::Call {
            callee: Callee::Kernel(KernelFn::LogPrintln),
            args: vec![Expr::Call {
                callee: Callee::Kernel(KernelFn::StringFromInt),
                args: vec![Expr::Call {
                    callee: Callee::Func(FuncId::from_raw(0)),
                    args: vec![Expr::Ctor {
                        ty: wrap,
                        variant: mk_wrap,
                        args: vec![Expr::Tuple(vec![Expr::Int(3), Expr::Int(4)])],
                    }],
                }],
            }],
        },
    };

    Ok(Program {
        modules: vec![Module {
            name: ModPath(vec![main_mod]),
            types: vec![TypeDef::Enum(def)],
            funcs: vec![fst_of_fn, main_fn],
            entry: Some(FuncId::from_raw(1)),
            records: vec![],
        }],
    })
}

#[test]
fn tuple_subpattern_in_ctor_arm_renders() -> DResult<()> {
    let mut i = Interner::new();
    let prog = wrap_program(&mut i)?;
    let src = emit(&i, &prog)?;

    // The constructor pattern's tuple sub-pattern renders as `(a, b)`.
    assert!(
        src.contains("MainWrap::MkWrap((a, b)) => a"),
        "tuple sub-pattern must render as a Rust tuple pattern; got:\n{src}"
    );
    // The matching tuple construction renders as `((3, 4))` inside the ctor.
    assert!(
        src.contains("MainWrap::MkWrap((3, 4))"),
        "tuple-field construction must render a Rust tuple; got:\n{src}"
    );
    Ok(())
}

#[test]
fn unit_value_and_type_render() -> DResult<()> {
    let mut i = Interner::new();
    let main_mod = i.intern("Main")?;
    let nop = i.intern("nop")?;

    // nop() -> () = ()
    let prog = Program {
        modules: vec![Module {
            name: ModPath(vec![main_mod]),
            types: vec![],
            funcs: vec![Func {
                id: FuncId::from_raw(0),
                name: nop,
                type_params: vec![],
                params: vec![],
                ret: IrType::Unit,
                body: Expr::Unit,
            }],
            entry: None,
            records: vec![],
        }],
    };
    let src = emit(&i, &prog)?;

    assert!(
        src.contains("pub fn main_nop() -> () {\n    ()\n}"),
        "unit value and unit return type must both render as `()`; got:\n{src}"
    );
    Ok(())
}

/// Full spine: build the `Wrap` IR, emit, vendor the runtime, `cargo build`,
/// run, and assert `3` — the Go-backend value for `fstOf (MkWrap (3, 4))`. The
/// soundness-floor regression for a value laundered through a tuple-carrying
/// payload destructured by a tuple pattern. Gated on `SKY_E2E=1`.
#[test]
fn end_to_end_ctor_tuple_field_prints_three() -> DResult<()> {
    let mut interner = Interner::new();
    let prog = wrap_program(&mut interner)?;
    build_and_assert(&interner, &prog, "sky_backend_tuple_wrap_e2e", "3\n")
}

/// Emit `prog`, vendor the runtime into a temp dir named `slot`, `cargo build`,
/// run the binary, and assert its stdout equals `expected`. Gated on `SKY_E2E=1`
/// so the default `cargo test` stays fast and offline.
fn build_and_assert(
    interner: &Interner,
    prog: &Program,
    slot: &str,
    expected: &str,
) -> DResult<()> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let emitted = RustBackend::new(interner).emit(prog)?;

    let out = std::env::temp_dir().join(slot);
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).map_err(|e| io_bug(&src, &e))?;

    let runtime = resolve_runtime().ok_or_else(|| Diagnostic::CompilerBug {
        where_: "tuple_patterns e2e",
        detail: "could not locate runtime-rust/src/sky_runtime".to_owned(),
    })?;
    copy_dir(&runtime, &src.join("sky_runtime"))?;

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
        "emitted tuple-pattern project must build: {status:?}"
    );

    let bin = out.join("target").join("debug").join("sky-app");
    let output = Command::new(&bin).output().map_err(|e| io_bug(&bin, &e))?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected,
        "tuple-pattern program output must match the Go oracle"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    Ok(())
}

fn io_bug(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "tuple_patterns e2e io",
        detail: format!("{}: {e}", path.display()),
    }
}

fn resolve_runtime() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("SKY_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let cwd = std::env::current_dir().ok()?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        for candidate in [
            dir.join("sky")
                .join("runtime-rust")
                .join("src")
                .join("sky_runtime"),
            dir.join("runtime-rust").join("src").join("sky_runtime"),
        ] {
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        here = dir.parent();
    }
    None
}

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
