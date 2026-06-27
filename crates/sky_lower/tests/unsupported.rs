//! Each M0-unsupported feature must lower to a `Diagnostic::Lower` carrying the
//! offending node's span and the right `SKY-L01##` code — never a `CompilerBug`.
//!
//! These build the canonical AST + solved types directly (rather than through
//! the parser/checker) so each lowering arm is exercised in isolation: the
//! upstream stages reject most of these shapes earlier, so a source-level test
//! could never reach the lowerer's arm at all.

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_diagnostics::{
    Code, DResult, Diagnostic, Feature, Located, LowerError, SKY_L0100, SKY_L0101, SKY_L0102,
    SKY_L0103, SKY_L0104, SKY_L0105, SKY_L0106, SKY_L0107, SKY_L0108, Span,
};
use sky_intern::{Interner, Symbol};
use sky_lower::lower;
use sky_types::{SolvedTypes, Ty};

/// Lower a hand-built module + binding environment. `regions` is unused by the
/// lowerer, so an empty map suffices.
fn run(
    unions: Vec<canon::Union>,
    defs: Vec<canon::Def>,
    env: BTreeMap<Symbol, Ty>,
    interner: &Interner,
) -> DResult<sky_ir::Program> {
    let m = canon::Module {
        name: Vec::new(),
        unions,
        defs,
    };
    let types = SolvedTypes {
        env,
        regions: BTreeMap::new(),
    };
    lower(&m, &types, interner)
}

/// Assert the lowering failed with exactly the expected unsupported feature,
/// code, and primary span — and that it is a `Lower`, never a `CompilerBug`.
fn assert_unsupported(res: DResult<sky_ir::Program>, feature: Feature, code: Code, span: Span) {
    assert!(
        res.is_err(),
        "expected an unsupported-feature diagnostic for {feature:?}, got a successful lowering"
    );
    let Err(d) = res else { return };
    assert_eq!(d.code(), code, "code mismatch ({feature:?}): {d:?}");
    assert_eq!(d.primary_span(), span, "span mismatch ({feature:?}): {d:?}");
    assert_ne!(
        d.primary_span(),
        Span::DUMMY,
        "must carry a real span: {d:?}"
    );
    assert!(
        matches!(
            d,
            Diagnostic::Lower {
                msg: LowerError::Unsupported(f),
                ..
            } if f == feature
        ),
        "expected Lower/Unsupported({feature:?}), got {d:?}"
    );
}

/// A trivial integer-literal expression at `span`.
const fn int(span: Span, n: i64) -> canon::Expr {
    Located::new(span, canon::Expr_::Int(n))
}

/// A built-in nullary type constructor `Int` (so a binding's signature lowers
/// without itself tripping an unsupported arm).
fn con_int(interner: &mut Interner) -> DResult<canon::Type> {
    Ok(canon::Type::Con {
        home: Vec::new(),
        name: interner.intern("Int")?,
        args: Vec::new(),
    })
}

#[test]
fn untyped_function_with_parameters() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let x = i.intern("x")?;
    let name = Located::new(Span::new(10, 11), f);
    let patterns = vec![Located::new(Span::new(12, 13), canon::Pattern_::PVar(x))];
    let def = canon::Def::Untyped {
        name,
        patterns,
        body: int(Span::new(14, 15), 0),
    };
    // The binding's name span is blamed.
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::UntypedFunctions,
        SKY_L0106,
        Span::new(10, 11),
    );
    Ok(())
}

#[test]
fn non_variable_parameter_pattern() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let ty = canon::Type::Lambda(Box::new(con_int(&mut i)?), Box::new(con_int(&mut i)?));
    // A `_` parameter: M0 params must be plain names.
    let patterns = vec![Located::new(Span::new(20, 21), canon::Pattern_::PAnything)];
    let def = canon::Def::Typed {
        name: Located::new(Span::new(18, 19), f),
        free_vars: Vec::new(),
        patterns,
        body: int(Span::new(22, 23), 0),
        ty,
    };
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::ParamPatterns,
        SKY_L0105,
        Span::new(20, 21),
    );
    Ok(())
}

#[test]
fn function_type_in_annotation_argument() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let x = i.intern("x")?;
    // f : (Int -> Int) -> Int  — a higher-order argument.
    let arg = canon::Type::Lambda(Box::new(con_int(&mut i)?), Box::new(con_int(&mut i)?));
    let ty = canon::Type::Lambda(Box::new(arg), Box::new(con_int(&mut i)?));
    let patterns = vec![Located::new(Span::new(30, 31), canon::Pattern_::PVar(x))];
    let def = canon::Def::Typed {
        name: Located::new(Span::new(28, 29), f),
        free_vars: Vec::new(),
        patterns,
        body: int(Span::new(32, 33), 0),
        ty,
    };
    // The argument type is blamed via its parameter pattern span.
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::HigherOrderValues,
        SKY_L0103,
        Span::new(30, 31),
    );
    Ok(())
}

#[test]
fn type_variable_in_annotation() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let a = i.intern("a")?;
    // f : a  — M0 is monomorphic.
    let def = canon::Def::Typed {
        name: Located::new(Span::new(40, 41), f),
        free_vars: vec![a],
        patterns: Vec::new(),
        body: int(Span::new(42, 43), 0),
        ty: canon::Type::Var(a),
    };
    // No parameters → the return type is blamed via the binding's name span.
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::Polymorphism,
        SKY_L0102,
        Span::new(40, 41),
    );
    Ok(())
}

#[test]
fn task_with_non_unit_result() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let task = i.intern("Task")?;
    let int_name = i.intern("Int")?;
    // Inferred type: Task Int (only Task () is modelled).
    let mut env = BTreeMap::new();
    env.insert(
        f,
        Ty::Con {
            module: Vec::new(),
            name: task,
            args: vec![Ty::Con {
                module: Vec::new(),
                name: int_name,
                args: Vec::new(),
            }],
        },
    );
    let def = canon::Def::Untyped {
        name: Located::new(Span::new(50, 51), f),
        patterns: Vec::new(),
        body: int(Span::new(52, 53), 0),
    };
    assert_unsupported(
        run(Vec::new(), vec![def], env, &i),
        Feature::TaskResults,
        SKY_L0104,
        Span::new(50, 51),
    );
    Ok(())
}

#[test]
fn inferred_function_type_in_value_position() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let int_name = i.intern("Int")?;
    let con = |name| Ty::Con {
        module: Vec::new(),
        name,
        args: Vec::new(),
    };
    // Inferred type: Int -> Int (a bare function as a value).
    let mut env = BTreeMap::new();
    env.insert(f, Ty::Fun(Box::new(con(int_name)), Box::new(con(int_name))));
    let def = canon::Def::Untyped {
        name: Located::new(Span::new(60, 61), f),
        patterns: Vec::new(),
        body: int(Span::new(62, 63), 0),
    };
    assert_unsupported(
        run(Vec::new(), vec![def], env, &i),
        Feature::HigherOrderValues,
        SKY_L0103,
        Span::new(60, 61),
    );
    Ok(())
}

#[test]
fn bare_function_reference_as_value() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let module = i.intern("String")?;
    let fname = i.intern("fromInt")?;
    let ty = con_int(&mut i)?;
    // body is a bare kernel reference, not a call.
    let body = Located::new(
        Span::new(72, 79),
        canon::Expr_::VarKernel {
            module,
            name: fname,
        },
    );
    let def = canon::Def::Typed {
        name: Located::new(Span::new(70, 71), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::FirstClassFunctions,
        SKY_L0107,
        Span::new(72, 79),
    );
    Ok(())
}

#[test]
fn non_name_call_callee() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let ty = con_int(&mut i)?;
    // A call whose callee is an integer literal — a computed callee.
    let callee = Box::new(int(Span::new(82, 83), 5));
    let body = Located::new(Span::new(82, 90), canon::Expr_::Call(callee, Vec::new()));
    let def = canon::Def::Typed {
        name: Located::new(Span::new(80, 81), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    // The callee node's span is blamed.
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::FirstClassFunctions,
        SKY_L0107,
        Span::new(82, 83),
    );
    Ok(())
}

#[test]
fn unknown_kernel_call() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let module = i.intern("Time")?;
    let fname = i.intern("now")?;
    let ty = con_int(&mut i)?;
    let callee = Box::new(Located::new(
        Span::new(92, 100),
        canon::Expr_::VarKernel {
            module,
            name: fname,
        },
    ));
    let body = Located::new(Span::new(92, 102), canon::Expr_::Call(callee, Vec::new()));
    let def = canon::Def::Typed {
        name: Located::new(Span::new(90, 91), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::Kernels,
        SKY_L0108,
        Span::new(92, 100),
    );
    Ok(())
}

#[test]
fn unsupported_binary_operator() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let op = i.intern("*")?;
    let home = i.intern("Basics")?;
    let func = i.intern("mul")?;
    let ty = con_int(&mut i)?;
    // 1 * 2 — only +/- are modelled.
    let body = Located::new(
        Span::new(112, 117),
        canon::Expr_::Binop {
            op,
            home,
            func,
            lhs: Box::new(int(Span::new(112, 113), 1)),
            rhs: Box::new(int(Span::new(116, 117), 2)),
        },
    );
    let def = canon::Def::Typed {
        name: Located::new(Span::new(110, 111), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    // The whole binop expression span is blamed.
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::BinOps,
        SKY_L0101,
        Span::new(112, 117),
    );
    Ok(())
}

#[test]
fn non_constructor_pattern_in_first_arm() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let x = i.intern("x")?;
    let ty = con_int(&mut i)?;
    // case x of _ -> 0  (a wildcard arm; M0 matches only nullary ctors).
    let scrut = Box::new(Located::new(Span::new(122, 123), canon::Expr_::VarLocal(x)));
    let branch = canon::CaseBranch {
        pat: Located::new(Span::new(126, 127), canon::Pattern_::PAnything),
        body: int(Span::new(130, 131), 0),
    };
    let body = Located::new(Span::new(120, 132), canon::Expr_::Case(scrut, vec![branch]));
    let def = canon::Def::Typed {
        name: Located::new(Span::new(118, 119), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    assert_unsupported(
        run(Vec::new(), vec![def], BTreeMap::new(), &i),
        Feature::CasePatternKinds,
        SKY_L0100,
        Span::new(126, 127),
    );
    Ok(())
}

#[test]
fn non_constructor_pattern_in_later_arm() -> DResult<()> {
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let x = i.intern("x")?;
    let msg = i.intern("Msg")?;
    let inc = i.intern("Increment")?;
    let dec = i.intern("Decrement")?;
    let ty = con_int(&mut i)?;
    // A real union so the first (constructor) arm passes the enum lookup.
    let union = canon::Union {
        name: msg,
        ctors: vec![
            canon::Ctor {
                name: inc,
                index: 0,
                arity: 0,
            },
            canon::Ctor {
                name: dec,
                index: 1,
                arity: 0,
            },
        ],
    };
    let scrut = Box::new(Located::new(Span::new(142, 143), canon::Expr_::VarLocal(x)));
    let arm0 = canon::CaseBranch {
        pat: Located::new(
            Span::new(146, 155),
            canon::Pattern_::PCtor {
                home: Vec::new(),
                type_name: msg,
                name: inc,
                index: 0,
                args: Vec::new(),
            },
        ),
        body: int(Span::new(159, 160), 0),
    };
    // The second arm is a variable pattern — unsupported.
    let arm1 = canon::CaseBranch {
        pat: Located::new(Span::new(163, 164), canon::Pattern_::PVar(x)),
        body: int(Span::new(168, 169), 1),
    };
    let body = Located::new(
        Span::new(140, 170),
        canon::Expr_::Case(scrut, vec![arm0, arm1]),
    );
    let def = canon::Def::Typed {
        name: Located::new(Span::new(138, 139), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    // The later non-constructor arm's pattern span is blamed.
    assert_unsupported(
        run(vec![union], vec![def], BTreeMap::new(), &i),
        Feature::CasePatternKinds,
        SKY_L0100,
        Span::new(163, 164),
    );
    Ok(())
}
