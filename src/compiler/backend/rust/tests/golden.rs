//! End-to-end byte-equality gate for the Rust backend.
//!
//! Builds the canonical golden IR `Program` by hand (the same program the full
//! pipeline lowers `tests/golden/basics/Main.ipe` into) and asserts that
//! [`RustBackend::emit`] reproduces the golden `main.rs` and `Cargo.toml`
//! byte-for-byte. The golden is the correctness contract.

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::{DResult, Diagnostic};
use ipe_intern::Interner;
use ipe_ir::{
    Arm, BinOp, CallPin, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath,
    Module, OnFormKind, Pat, Program, TypeDef, Variant,
};

const GOLDEN_MAIN: &str = include_str!("../../../../../tests/golden/basics/main.rs");
const GOLDEN_CARGO: &str = include_str!("../../../../../tests/golden/basics/Cargo.toml");

/// Build the golden program:
/// ```ipe
/// type Msg = Increment | Decrement
/// update msg count =
///     case msg of
///         Increment -> count + 1
///         Decrement -> count - 1
/// main = Io.println (String.fromInt (update Increment 0))
/// ```
#[allow(clippy::too_many_lines)]
fn build_m0(interner: &mut Interner) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let msg_ty = interner.intern("Msg")?;
    let increment = interner.intern("Increment")?;
    let decrement = interner.intern("Decrement")?;
    let update = interner.intern("update")?;
    let main = interner.intern("main")?;
    let msg = interner.intern("msg")?;
    let count = interner.intern("count")?;

    let update_id = FuncId::from_raw(0);
    let main_id = FuncId::from_raw(1);

    let arms = vec![
        Arm {
            pat: Pat::Ctor {
                home: ModPath(vec![]),
                ty: msg_ty,
                variant: increment,
                args: vec![],
            },
            body: Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(count)),
                rhs: Box::new(Expr::Int(1)),
            },
            guard: None,
        },
        Arm {
            pat: Pat::Ctor {
                home: ModPath(vec![]),
                ty: msg_ty,
                variant: decrement,
                args: vec![],
            },
            body: Expr::BinOp {
                op: BinOp::Sub,
                lhs: Box::new(Expr::Var(count)),
                rhs: Box::new(Expr::Int(1)),
            },
            guard: None,
        },
    ];
    let update_match = Match::new(Expr::Var(msg), arms, &[increment, decrement])?;

    let update_fn = Func {
        id: update_id,
        name: update,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![
            (
                msg,
                IrType::Enum {
                    home: ModPath(vec![]),
                    name: msg_ty,
                    args: vec![],
                },
            ),
            (count, IrType::Int),
        ],
        ret: IrType::Int,
        body: Expr::Match(update_match),
    };

    let main_fn = Func {
        id: main_id,
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
                    callee: Callee::Func(update_id),
                    args: vec![
                        Expr::Ctor {
                            home: ModPath(vec![]),
                            ty: msg_ty,
                            variant: increment,
                            args: vec![],
                        },
                        Expr::Int(0),
                    ],
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

    Ok(Program {
        modules: vec![Module {
            name: ModPath(vec![main_mod]),
            types: vec![TypeDef::Enum(EnumDef {
                name: msg_ty,
                type_params: vec![],
                variants: vec![
                    Variant {
                        name: increment,
                        fields: vec![],
                    },
                    Variant {
                        name: decrement,
                        fields: vec![],
                    },
                ],
                home: ModPath(vec![]),
            })],
            funcs: vec![update_fn, main_fn],
            entry: Some(main_id),
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
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

/// A program with no user types and no user functions in `main.rs`. Exercises
/// the emitter's empty-section branches: the USER-TYPES banner is followed
/// directly by the runtime bindings, and the bindings are followed directly by
/// the epilogue, each separated by exactly one blank line.
fn build_no_user_items(interner: &mut Interner) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let main = interner.intern("main")?;
    let main_id = FuncId::from_raw(0);

    // `main = Io.println "hi"` — no user types, and the single `main` function's
    // body is a kernel call, so no user-defined helper functions are emitted.
    let main_fn = Func {
        id: main_id,
        name: main,
        home: ModPath(vec![]),
        type_params: vec![],
        params: vec![],
        ret: IrType::Task(Box::new(IrType::Unit)),
        body: Expr::Call {
            callee: Callee::Kernel(KernelFn::IoPrintln),
            args: vec![Expr::Str("hi".to_owned())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        },
    };

    Ok(Program {
        modules: vec![Module {
            name: ModPath(vec![main_mod]),
            types: vec![],
            funcs: vec![main_fn],
            entry: Some(main_id),
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
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

/// The emitter never produces two consecutive blank lines, so its raw
/// (pre-rustfmt) output already satisfies rustfmt's max-one-consecutive-blank
/// rule. This pins the empty-section blank-line guards: without them, a program
/// with no user types (banner immediately followed by the runtime bindings) or
/// no `main.rs` functions (bindings immediately followed by the epilogue)
/// emitted a double blank that only the rustfmt pass papered over — a silent
/// drift from `cargo fmt --check`-clean the moment the pass is skipped (the
/// `ipe watch` hot loop, and the whole `wasm32` playground, which cannot spawn
/// rustfmt).
///
/// Uses [`RustBackend::emit_spine`], which returns the raw pre-rustfmt spine
/// text directly (no environment toggle, so no cross-test formatting race). The
/// spine carries no user functions, exercising the bindings-to-epilogue guard;
/// `build_m0` (user types present) and `build_no_user_items` (types absent)
/// exercise the banner-to-bindings guard on both branches.
#[test]
fn emitter_spine_has_no_consecutive_blank_lines() -> DResult<()> {
    for build in [
        build_m0 as fn(&mut Interner) -> DResult<Program>,
        build_no_user_items,
    ] {
        let mut interner = Interner::new();
        let program = build(&mut interner)?;
        let backend = RustBackend::new(&interner);
        let spine = backend.emit_spine(&program)?;
        assert!(
            !spine.contains("\n\n\n"),
            "raw emitter output must not contain two consecutive blank lines \
             (rustfmt collapses them, so this drifts from fmt-clean whenever the \
             fmt pass is skipped):\n{spine}"
        );
    }
    Ok(())
}
