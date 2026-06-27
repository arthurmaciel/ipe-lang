//! End-to-end byte-equality gate for the M0 Rust backend.
//!
//! Builds the canonical M0 IR `Program` by hand (the same program the full
//! pipeline lowers `tests/golden/m0/Main.sky` into) and asserts that
//! [`RustBackend::emit`] reproduces the golden `main.rs` and `Cargo.toml`
//! byte-for-byte. The golden is the correctness contract for M0.

use sky_backend::Backend;
use sky_backend_rust::RustBackend;
use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Interner;
use sky_ir::{
    Arm, BinOp, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath, Module, Pat,
    Program, TypeDef,
};

const GOLDEN_MAIN: &str = include_str!("../../../tests/golden/m0/main.rs");
const GOLDEN_CARGO: &str = include_str!("../../../tests/golden/m0/Cargo.toml");

/// Build the M0 program:
/// ```sky
/// type Msg = Increment | Decrement
/// update msg count =
///     case msg of
///         Increment -> count + 1
///         Decrement -> count - 1
/// main = println (String.fromInt (update Increment 0))
/// ```
fn build_m0(interner: &mut Interner) -> DResult<Program> {
    let main_mod = interner.intern("Main");
    let msg_ty = interner.intern("Msg");
    let increment = interner.intern("Increment");
    let decrement = interner.intern("Decrement");
    let update = interner.intern("update");
    let main = interner.intern("main");
    let msg = interner.intern("msg");
    let count = interner.intern("count");

    let update_id = FuncId::from_raw(0);
    let main_id = FuncId::from_raw(1);

    let arms = vec![
        Arm {
            pat: Pat::Ctor {
                ty: msg_ty,
                variant: increment,
            },
            body: Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(count)),
                rhs: Box::new(Expr::Int(1)),
            },
        },
        Arm {
            pat: Pat::Ctor {
                ty: msg_ty,
                variant: decrement,
            },
            body: Expr::BinOp {
                op: BinOp::Sub,
                lhs: Box::new(Expr::Var(count)),
                rhs: Box::new(Expr::Int(1)),
            },
        },
    ];
    let update_match = Match::new(Expr::Var(msg), arms, &[increment, decrement])?;

    let update_fn = Func {
        id: update_id,
        name: update,
        params: vec![(msg, IrType::Enum(msg_ty)), (count, IrType::Int)],
        ret: IrType::Int,
        body: Expr::Match(update_match),
    };

    let main_fn = Func {
        id: main_id,
        name: main,
        params: vec![],
        ret: IrType::TaskUnit,
        body: Expr::Call {
            callee: Callee::Kernel(KernelFn::LogPrintln),
            args: vec![Expr::Call {
                callee: Callee::Kernel(KernelFn::StringFromInt),
                args: vec![Expr::Call {
                    callee: Callee::Func(update_id),
                    args: vec![
                        Expr::Ctor {
                            ty: msg_ty,
                            variant: increment,
                        },
                        Expr::Int(0),
                    ],
                }],
            }],
        },
    };

    Ok(Program {
        modules: vec![Module {
            name: ModPath(vec![main_mod]),
            types: vec![TypeDef::Enum(EnumDef {
                name: msg_ty,
                variants: vec![increment, decrement],
            })],
            funcs: vec![update_fn, main_fn],
            entry: Some(main_id),
        }],
    })
}

fn missing(detail: &str) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "golden test",
        detail: detail.to_owned(),
    }
}

#[test]
fn m0_emits_byte_identical_main_rs() -> DResult<()> {
    let mut interner = Interner::new();
    let program = build_m0(&mut interner)?;

    let backend = RustBackend::new(&interner);
    let emitted = backend.emit(&program)?;

    let main_rs = emitted
        .files
        .get("src/main.rs")
        .ok_or_else(|| missing("no src/main.rs"))?;
    assert_eq!(
        main_rs.as_str(),
        GOLDEN_MAIN,
        "src/main.rs must match the golden byte-for-byte"
    );
    Ok(())
}

#[test]
fn m0_emits_byte_identical_cargo_toml() -> DResult<()> {
    let mut interner = Interner::new();
    let program = build_m0(&mut interner)?;

    let backend = RustBackend::new(&interner);
    let emitted = backend.emit(&program)?;

    assert_eq!(
        emitted.cargo_toml.as_str(),
        GOLDEN_CARGO,
        "Cargo.toml must match the golden"
    );
    Ok(())
}

#[test]
fn m0_backend_name_is_rust() {
    let interner = Interner::new();
    let backend = RustBackend::new(&interner);
    assert_eq!(backend.name(), "rust");
}
