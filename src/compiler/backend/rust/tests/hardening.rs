//! Hardening regression tests for the Rust backend.
//!
//! These exercise the failure-fast and identifier-safety guards that sit
//! *around* the byte-identical golden emission:
//!
//! * reserved-Rust-name mangling at variant / param emit sites,
//! * the bounded emit-depth guard (IPE-L0200) instead of a native stack
//!   overflow on a deeply nested expression,
//! * the checked ident resolver (IPE-I0201) refusing to emit an empty Rust
//!   identifier,
//! * the cross-module type-name collision guard (IPE-N0012, formerly IPE-I0202).
//!
//! The golden byte-equality contract itself lives in `golden.rs`.

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::{DResult, Diagnostic, IPE_I0201, IPE_L0200, IPE_N0012};
use ipe_intern::{Interner, Symbol};
use ipe_ir::{
    Arm, BinOp, EnumDef, Expr, Func, FuncId, IrType, Match, ModPath, Module, Pat, Program, TypeDef,
    Variant,
};

/// A single-module program with the given types and funcs (no entry needed:
/// emission does not require one).
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
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
            uses_cache: false,
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
    }
}

fn emit(interner: &Interner, program: &Program) -> DResult<String> {
    let emitted = RustBackend::new(interner).emit(program)?;
    emitted
        .files
        .get("src/main.rs")
        .cloned()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "hardening test",
            detail: "no src/main.rs".to_owned(),
        })
}

/// A reserved Rust keyword used as a variant name and a param name must be
/// mangled (`type` → `type_`) so the emitted Rust compiles — while the enum's
/// `ipe_show` keeps the original Ipê spelling.
#[test]
fn reserved_names_are_mangled_in_emitted_output() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let kw_ty = interner.intern("Kw")?;
    // Lowercase Rust keywords used as a variant and a parameter name.
    let variant = interner.intern("type")?;
    let param = interner.intern("match")?;
    let func = interner.intern("render")?;

    let en = EnumDef {
        name: kw_ty,
        type_params: vec![],
        variants: vec![Variant {
            name: variant,
            fields: vec![],
        }],
        home: ModPath(vec![]),
    };
    let render_fn = Func {
        id: FuncId::from_raw(0),
        name: func,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![(param, IrType::Int)],
        ret: IrType::Int,
        body: Expr::Var(param),
    };

    let prog = program(main_mod, vec![TypeDef::Enum(en)], vec![render_fn]);
    let out = emit(&interner, &prog)?;

    // Variant declared and matched under its mangled Rust name…
    assert!(out.contains("    type_,\n"), "variant not mangled:\n{out}");
    assert!(
        out.contains("MainKw::type_ => \"type\".to_string(),"),
        "ipe_show must mangle the ident but keep the Ipe display name:\n{out}"
    );
    // …and the keyword parameter is mangled too, with a valid body reference.
    assert!(
        out.contains("pub fn main_render(match_: i64) -> i64 {"),
        "param not mangled:\n{out}"
    );
    assert!(out.contains("    match_\n}"), "var ref not mangled:\n{out}");
    Ok(())
}

/// A deeply nested `BinOp` spine must fail fast with IPE-L0200, not overflow the
/// native stack. The chain is built well past the backend's emit-depth bound.
#[test]
fn deeply_nested_expr_fails_fast_not_stack_overflow() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let func = interner.intern("deep")?;

    // 4096 left-nested additions: (((… + 1) + 1) + 1). The leaf sits far below
    // the backend's MAX_EMIT_DEPTH (~96), so the guard trips long before.
    let mut body = Expr::Int(0);
    for _ in 0..4096 {
        body = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(body),
            rhs: Box::new(Expr::Int(1)),
        };
    }
    let deep_fn = Func {
        id: FuncId::from_raw(0),
        name: func,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![],
        ret: IrType::Int,
        body,
    };

    let prog = program(main_mod, vec![], vec![deep_fn]);
    let res = emit(&interner, &prog);
    assert!(res.is_err(), "deep nesting must error, got {res:?}");
    if let Err(err) = res {
        assert_eq!(err.code(), IPE_L0200, "wrong code for over-deep nesting");
        assert!(
            matches!(err, Diagnostic::Lower { .. }),
            "expected a Lower diagnostic, got {err:?}"
        );
    }
    Ok(())
}

/// An expression at the depth bound still emits successfully — the guard is a
/// ceiling, not an off-by-one rejection of legitimate programs.
#[test]
fn nesting_at_the_bound_still_emits() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let func = interner.intern("ok")?;

    // 64 levels: comfortably under the emit-depth ceiling.
    let mut body = Expr::Int(0);
    for _ in 0..64 {
        body = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(body),
            rhs: Box::new(Expr::Int(1)),
        };
    }
    let ok_fn = Func {
        id: FuncId::from_raw(0),
        name: func,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![],
        ret: IrType::Int,
        body,
    };
    let prog = program(main_mod, vec![], vec![ok_fn]);
    assert!(emit(&interner, &prog).is_ok(), "in-bound nesting must emit");
    Ok(())
}

/// A param symbol that resolves to the empty string is a dangling-symbol
/// invariant violation: emit must fail with IPE-I0201, never produce an empty
/// Rust identifier.
#[test]
fn empty_intended_symbol_is_rejected() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let func = interner.intern("f")?;
    // An interned *empty* identifier — a dangling/empty-intended symbol the
    // lowerer must never produce.
    let empty = interner.intern("")?;

    let f = Func {
        id: FuncId::from_raw(0),
        name: func,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![(empty, IrType::Int)],
        ret: IrType::Int,
        body: Expr::Int(0),
    };
    let prog = program(main_mod, vec![], vec![f]);
    let res = emit(&interner, &prog);
    assert!(res.is_err(), "empty ident must error, got {res:?}");
    if let Err(err) = res {
        assert_eq!(err.code(), IPE_I0201, "wrong code for dangling symbol");
    }
    Ok(())
}

/// Two modules declaring a same-named type intern to the same `Symbol`; the
/// backend cannot tell them apart from the bare key, so it must fail fast with
/// IPE-I0202 rather than silently overwrite one mapping.
#[test]
#[allow(clippy::too_many_lines)] // two full body-free Module literals inline
fn cross_module_type_name_collision_is_rejected() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let other_mod = interner.intern("Other")?;
    // Same Ipê type name in both modules → the *same* interned Symbol.
    let msg = interner.intern("Msg")?;
    let inc = interner.intern("Increment")?;

    let make_enum = || {
        TypeDef::Enum(EnumDef {
            name: msg,
            type_params: vec![],
            variants: vec![Variant {
                name: inc,
                fields: vec![],
            }],
            home: ModPath(vec![]),
        })
    };
    let prog = Program {
        modules: vec![
            Module {
                name: ModPath(vec![main_mod]),
                types: vec![make_enum()],
                funcs: vec![],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_http: false,
                uses_config: false,
                uses_compression: false,
                uses_csv: false,
                uses_cache: false,
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
            },
            Module {
                name: ModPath(vec![other_mod]),
                types: vec![make_enum()],
                funcs: vec![],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_http: false,
                uses_config: false,
                uses_compression: false,
                uses_csv: false,
                uses_cache: false,
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
            },
        ],
    };

    let res = RustBackend::new(&interner).emit(&prog);
    assert!(res.is_err(), "type-name collision must error, got {res:?}");
    if let Err(err) = res {
        // Pre-Defect-2-fix: IPE-I0202 (CompilerBug). Post-fix: clean IPE-N0012
        // (NameError::DuplicateType) so the user sees a structured error not an ICE.
        assert_eq!(err.code(), IPE_N0012, "wrong code for type-name collision");
    }
    Ok(())
}

/// `Expr::Let`, `Expr::If`, and the extended `BinOp` set emit total,
/// well-formed Rust even though the frontend does not yet produce them. The
/// `let`/`if` forms render as self-contained parenthesised expressions so they
/// compose anywhere an expression is expected, and `/=` maps to Rust `!=`.
#[test]
fn let_if_and_extended_binops_emit_total_rust() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let f = interner.intern("f")?;
    let n = interner.intern("n")?;
    let x = interner.intern("x")?;

    // f(n : Int) -> Int =
    //   let x = n * 2 in
    //     if (x >= 10) && (x /= 0) then x / 2 else x + 1
    let body = Expr::Let {
        name: x,
        value: Box::new(Expr::BinOp {
            op: BinOp::Mul,
            lhs: Box::new(Expr::Var(n)),
            rhs: Box::new(Expr::Int(2)),
        }),
        body: Box::new(Expr::If {
            cond: Box::new(Expr::BinOp {
                op: BinOp::And,
                lhs: Box::new(Expr::BinOp {
                    op: BinOp::Ge,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(10)),
                }),
                rhs: Box::new(Expr::BinOp {
                    op: BinOp::Neq,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(0)),
                }),
            }),
            then_: Box::new(Expr::BinOp {
                op: BinOp::Div,
                lhs: Box::new(Expr::Var(x)),
                rhs: Box::new(Expr::Int(2)),
            }),
            else_: Box::new(Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(x)),
                rhs: Box::new(Expr::Int(1)),
            }),
        }),
    };
    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: f,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![(n, IrType::Int)],
        ret: IrType::Int,
        body,
    };

    let out = emit(&interner, &program(main_mod, vec![], vec![f_fn]))?;
    // Rustfmt reformats the emitted let/if block onto multiple lines; assert the
    // key semantic fragments rather than a single-line snapshot.
    assert!(
        out.contains("let x = (n * 2);"),
        "let binding not emitted:\n{out}"
    );
    assert!(
        out.contains("(if ((x >= 10) && (x != 0))"),
        "if condition not emitted:\n{out}"
    );
    assert!(
        out.contains("(x / 2)") && out.contains("(x + 1)"),
        "if branches not emitted:\n{out}"
    );
    Ok(())
}

/// A tuple type in a return position and a tuple constructor in the body emit
/// as genuine Rust tuples: `(i64, i64)` and `(x, 1)`.
#[test]
fn tuple_type_and_expr_emit_as_rust_tuples() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let f = interner.intern("pair")?;
    let n = interner.intern("n")?;

    // pair(n : Int) -> (Int, Int) = (n, 1)
    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: f,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![(n, IrType::Int)],
        ret: IrType::Tuple(vec![IrType::Int, IrType::Int]),
        body: Expr::Tuple(vec![Expr::Var(n), Expr::Int(1)]),
    };

    let out = emit(&interner, &program(main_mod, vec![], vec![f_fn]))?;
    assert!(
        out.contains("-> (i64, i64)"),
        "tuple return type did not emit:\n{out}"
    );
    assert!(out.contains("(n, 1)"), "tuple expr did not emit:\n{out}");
    Ok(())
}

/// CO-BACKEND-005: `Expr::Char` carrying anything but exactly one character
/// is an internal invariant violation (the lexer's char-literal invariant
/// broke somewhere upstream) — it must fail closed as a `CompilerBug`, never
/// silently emit a Rust STRING literal in `char` position (invalid Rust,
/// `cargo`-fails E0308 — the exit-0-then-cargo-fail shape THE SEAL forbids).
#[test]
fn malformed_char_literal_fails_closed_not_invalid_rust() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let f = interner.intern("bad")?;

    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: f,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![],
        ret: IrType::Char,
        // A malformed char literal — two characters, never producible by the
        // real lexer, built directly to reach the emitter's fallback arm.
        body: Expr::Char("ab".to_owned()),
    };

    let res = emit(&interner, &program(main_mod, vec![], vec![f_fn]));
    assert!(
        res.is_err(),
        "a malformed char literal must fail closed, got {res:?}"
    );
    if let Err(err) = res {
        assert!(
            matches!(err, Diagnostic::CompilerBug { .. }),
            "expected a CompilerBug, got {err:?}"
        );
    }
    Ok(())
}

/// CO-BACKEND-005: a tuple-scrutinee `match` whose arm patterns disagree on
/// arity (an internal invariant violation the frontend's Maranget check
/// would never let through — built directly here) must fail closed as a
/// `CompilerBug` when a later arm's pattern reaches past the column table
/// sized from an earlier, narrower arm — never silently default the missing
/// column to `str_mode: false, list_mode: false`, which can emit a binder of
/// the wrong type (an exit-0-then-cargo-fail THE SEAL forbids).
#[test]
fn tuple_arm_wider_than_column_table_fails_closed() -> DResult<()> {
    let mut interner = Interner::new();
    let main_mod = interner.intern("Main")?;
    let func = interner.intern("bad")?;
    let narrow0 = interner.intern("narrow0")?;
    let narrow1 = interner.intern("narrow1")?;
    let wide0 = interner.intern("wide0")?;
    let wide1 = interner.intern("wide1")?;
    let wide2 = interner.intern("wide2")?;

    // A 2-element scrutinee tuple; the first arm's arity (2) sizes the
    // column table, but the second arm's pattern is a 3-element tuple.
    let arms = vec![
        Arm::new(
            Pat::Tuple(vec![Pat::Var(narrow0), Pat::Var(narrow1)]),
            Expr::Var(narrow0),
        ),
        Arm::new(
            Pat::Tuple(vec![Pat::Var(wide0), Pat::Var(wide1), Pat::Var(wide2)]),
            Expr::Var(wide0),
        ),
    ];
    let scrutinee = Expr::Tuple(vec![Expr::Int(1), Expr::Int(2)]);
    let f_fn = Func {
        id: FuncId::from_raw(0),
        name: func,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![],
        ret: IrType::Int,
        body: Expr::Match(Match::new_flat(scrutinee, arms)?),
    };

    let res = emit(&interner, &program(main_mod, vec![], vec![f_fn]));
    assert!(
        res.is_err(),
        "a wider-than-scrutinee arm must fail closed, got {res:?}"
    );
    if let Err(err) = res {
        assert!(
            matches!(err, Diagnostic::CompilerBug { .. }),
            "expected a CompilerBug, got {err:?}"
        );
    }
    Ok(())
}
