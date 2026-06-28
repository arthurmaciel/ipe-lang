//! Literal-, alias- and wildcard-pattern rendering tests for the M3b-3 Rust
//! backend (task M3B3-CORE).
//!
//! These exercise the [`Pat`] additions the backend learned to render as flat
//! Rust match arms:
//!
//! * the LITERAL leaves [`Pat::Int`] / [`Pat::Bool`] / [`Pat::Char`] /
//!   [`Pat::Str`] — rendered as the Rust literals `0` / `true` / `'a'` / `"hi"`,
//! * the ALIAS / `as` binder [`Pat::Alias`] — rendered as the Rust binding-with-
//!   subpattern form `name @ <inner>`.
//!
//! The literal cases appear here as constructor-payload SUB-patterns so they
//! flow through `render_pat` (the case-arm head stays a constructor under M3a's
//! exhaustiveness contract; same-top-constructor literal discrimination — `Just 0`
//! vs `Just n` — is M3b-4 / SKY-L0116 and lands with the decision-tree compiler).
//! They are render-only assertions: a single literal sub-pattern is not an
//! exhaustive cover of its scrutinee axis in Rust, so these programs are not
//! built — the literal's flat-arm SPELLING is what CORE pins. Exhaustiveness +
//! E2E for literals arrive with the M3b-3 lowering / types tasks.
//!
//! The alias case IS exhaustive (a binder matches anything), so its emitted
//! crate builds and runs: `end_to_end_alias_binds_whole_value` (gated on
//! `SKY_E2E=1`) drives the hand-built IR through the Rust backend and asserts
//! `7`, matching the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` on the shape-equivalent
//!
//! ```text
//! type Wrap = MkWrap Int
//! f w = case w of MkWrap (x as y) -> y
//! main = println (String.fromInt (f (MkWrap 7)))      -- prints 7
//! ```
//!
//! (hand-verified in a temp dir → stdout `7\n`, exit 0).

use std::path::{Path, PathBuf};
use std::process::Command;

use sky_backend::Backend;
use sky_backend_rust::RustBackend;
use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::{Interner, Symbol};
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
            where_: "literal_patterns test",
            detail: "no src/main.rs".to_owned(),
        })
}

/// Build a one-function program whose single case arm matches `payload` as the
/// sole sub-pattern of an `A`-variant constructor (`type Tag = A Int | B`), so
/// the sub-pattern flows through `render_pat`. The `B` arm completes the M3a
/// constructor cover. Returns `(program, the A variant symbol)`.
fn tag_program(interner: &mut Interner, payload: Pat) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let tag = interner.intern("Tag")?;
    let a = interner.intern("A")?;
    let b = interner.intern("B")?;
    let f = interner.intern("f")?;
    let w = interner.intern("w")?;
    let zero = interner.intern("zero")?;

    let def = EnumDef {
        name: tag,
        type_params: vec![],
        variants: vec![
            Variant {
                name: a,
                fields: vec![IrType::Int],
            },
            Variant {
                name: b,
                fields: vec![],
            },
        ],
    };

    let arms = vec![
        Arm {
            pat: Pat::Ctor {
                ty: tag,
                variant: a,
                args: vec![payload],
            },
            body: Expr::Int(1),
        },
        Arm {
            pat: Pat::Ctor {
                ty: tag,
                variant: b,
                args: vec![],
            },
            body: Expr::Int(0),
        },
    ];
    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: f,
        type_params: vec![],
        params: vec![(
            w,
            IrType::Enum {
                name: tag,
                args: vec![],
            },
        )],
        ret: IrType::Int,
        body: Expr::Match(sky_ir::Match::new(Expr::Var(w), arms, &[a, b])?),
    };

    // A trivial entry so the program is well-formed; `zero` keeps it minimal.
    let main_fn = Func {
        id: FuncId::from_raw(1),
        name: zero,
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
                        ty: tag,
                        variant: a,
                        args: vec![Expr::Int(0)],
                    }],
                }],
            }],
        },
    };

    Ok(Program {
        modules: vec![Module {
            name: ModPath(vec![main_mod]),
            types: vec![TypeDef::Enum(def)],
            funcs: vec![f_fn, main_fn],
            entry: Some(FuncId::from_raw(1)),
            records: vec![],
        }],
    })
}

#[test]
fn int_literal_subpattern_renders() -> DResult<()> {
    let mut i = Interner::new();
    let prog = tag_program(&mut i, Pat::Int(0))?;
    let src = emit(&i, &prog)?;
    assert!(
        src.contains("MainTag::A(0) =>"),
        "int literal sub-pattern must render as `0`; got:\n{src}"
    );
    Ok(())
}

#[test]
fn bool_literal_subpattern_renders() -> DResult<()> {
    let mut i = Interner::new();
    let prog = tag_program(&mut i, Pat::Bool(true))?;
    let src = emit(&i, &prog)?;
    assert!(
        src.contains("MainTag::A(true) =>"),
        "bool literal sub-pattern must render as `true`; got:\n{src}"
    );
    Ok(())
}

#[test]
fn char_literal_subpattern_renders() -> DResult<()> {
    let mut i = Interner::new();
    let prog = tag_program(&mut i, Pat::Char("a".to_owned()))?;
    let src = emit(&i, &prog)?;
    assert!(
        src.contains("MainTag::A('a') =>"),
        "char literal sub-pattern must render as `'a'`; got:\n{src}"
    );
    Ok(())
}

#[test]
fn char_literal_quote_escapes() -> DResult<()> {
    let mut i = Interner::new();
    let prog = tag_program(&mut i, Pat::Char("'".to_owned()))?;
    let src = emit(&i, &prog)?;
    assert!(
        src.contains(r"MainTag::A('\'') =>"),
        "single-quote char must escape to a valid Rust char literal; got:\n{src}"
    );
    Ok(())
}

#[test]
fn str_literal_subpattern_renders_and_escapes() -> DResult<()> {
    let mut i = Interner::new();
    let prog = tag_program(&mut i, Pat::Str("hi\"\n".to_owned()))?;
    let src = emit(&i, &prog)?;
    assert!(
        src.contains(r#"MainTag::A("hi\"\n") =>"#),
        "string literal sub-pattern must render an escaped Rust string literal; got:\n{src}"
    );
    Ok(())
}

/// The `Wrap` alias program: `f w = case w of MkWrap (x as y) -> y`. The `x as y`
/// alias renders `y @ x` and the whole match stays exhaustive (a binder matches
/// anything), so the emitted crate builds and runs.
fn alias_program(interner: &mut Interner) -> DResult<(Program, Symbol, Symbol)> {
    let main_mod = interner.intern("Main")?;
    let wrap = interner.intern("Wrap")?;
    let mk_wrap = interner.intern("MkWrap")?;
    let f = interner.intern("f")?;
    let main = interner.intern("main")?;
    let w = interner.intern("w")?;
    let x = interner.intern("x")?;
    let y = interner.intern("y")?;

    let def = EnumDef {
        name: wrap,
        type_params: vec![],
        variants: vec![Variant {
            name: mk_wrap,
            fields: vec![IrType::Int],
        }],
    };

    // f w = case w of MkWrap (x as y) -> y
    let arms = vec![Arm {
        pat: Pat::Ctor {
            ty: wrap,
            variant: mk_wrap,
            args: vec![Pat::Alias(Box::new(Pat::Var(x)), y)],
        },
        body: Expr::Var(y),
    }];
    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: f,
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

    // main = println (String.fromInt (f (MkWrap 7)))
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
                        args: vec![Expr::Int(7)],
                    }],
                }],
            }],
        },
    };

    Ok((
        Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(def)],
                funcs: vec![f_fn, main_fn],
                entry: Some(FuncId::from_raw(1)),
                records: vec![],
            }],
        },
        x,
        y,
    ))
}

#[test]
fn alias_subpattern_renders_binding_with_subpattern() -> DResult<()> {
    let mut i = Interner::new();
    let (prog, _x, _y) = alias_program(&mut i)?;
    let src = emit(&i, &prog)?;
    assert!(
        src.contains("MainWrap::MkWrap(y @ x) =>"),
        "alias pattern must render as Rust `name @ <inner>`; got:\n{src}"
    );
    Ok(())
}

/// Full spine: build the alias `Wrap` IR, emit, vendor the runtime, `cargo
/// build`, run, and assert `7` — the Go-backend value for `f (MkWrap 7)` where
/// the `as` binder rebinds the whole matched payload. Gated on `SKY_E2E=1`.
#[test]
fn end_to_end_alias_binds_whole_value() -> DResult<()> {
    let mut interner = Interner::new();
    let (prog, _x, _y) = alias_program(&mut interner)?;
    build_and_assert(&interner, &prog, "sky_backend_alias_wrap_e2e", "7\n")
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
        where_: "literal_patterns e2e",
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
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "emitted alias-pattern project must build: {status:?}"
    );

    let bin = out.join("target").join("debug").join("sky-app");
    let output = Command::new(&bin).output().map_err(|e| io_bug(&bin, &e))?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected,
        "alias-pattern program output must match the Go oracle"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    Ok(())
}

fn io_bug(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "literal_patterns e2e io",
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
