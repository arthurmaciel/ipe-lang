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
    SKY_L0104, SKY_L0105, SKY_L0106, SKY_L0107, SKY_L0108, SKY_L0110, Span,
};
use sky_intern::{Interner, Symbol};
use sky_ir::{Callee, Expr, IrType, KernelFn};
use sky_lower::lower;
use sky_types::{SolvedTypes, Ty};

/// Extract the single lowered function from a one-binding program, or `None` if
/// the shape is unexpected (the caller asserts on the `Option`).
fn single_func(res: &DResult<sky_ir::Program>) -> Option<&sky_ir::Func> {
    res.as_ref().ok()?.modules.first()?.funcs.first()
}

/// Lower a hand-built module + binding environment, with no per-region types.
/// Suffices for the arms that never consult `regions` (most unsupported gates).
fn run(
    unions: Vec<canon::Union>,
    defs: Vec<canon::Def>,
    env: BTreeMap<Symbol, Ty>,
    interner: &Interner,
) -> DResult<sky_ir::Program> {
    run_with_regions(unions, defs, env, BTreeMap::new(), interner)
}

/// Lower a hand-built module with an explicit per-region (`span` → solved `Ty`)
/// map — needed by the arms that reify a value's solved type (a first-class
/// function reference, a function-typed lambda parameter).
fn run_with_regions(
    unions: Vec<canon::Union>,
    defs: Vec<canon::Def>,
    env: BTreeMap<Symbol, Ty>,
    regions: BTreeMap<Span, Ty>,
    interner: &Interner,
) -> DResult<sky_ir::Program> {
    let m = canon::Module {
        name: Vec::new(),
        unions,
        defs,
    };
    let types = SolvedTypes { env, regions };
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
fn function_type_in_annotation_argument_lowers_to_fun() -> DResult<()> {
    // `f : (Int -> Int) -> Int` — a higher-order argument now lowers to a boxed
    // `Fn` parameter type `Fun([Int], Int)` (M1 first-class functions), not an
    // unsupported-feature diagnostic.
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let x = i.intern("x")?;
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
    let res = run(Vec::new(), vec![def], BTreeMap::new(), &i);
    let func = single_func(&res);
    let param_ty = func.and_then(|fc| fc.params.first()).map(|(_, t)| t);
    assert_eq!(
        param_ty,
        Some(&IrType::Fun(vec![IrType::Int], Box::new(IrType::Int))),
        "higher-order param must lower to Fun([Int], Int): {res:?}"
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
fn inferred_function_type_in_value_position_lowers_to_fun() -> DResult<()> {
    // An inferred function type `Int -> Int` in value position now lowers to the
    // boxed `Fn` return type `Fun([Int], Int)` (M1 first-class functions).
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let int_name = i.intern("Int")?;
    let con = |name| Ty::Con {
        module: Vec::new(),
        name,
        args: Vec::new(),
    };
    let mut env = BTreeMap::new();
    env.insert(f, Ty::Fun(Box::new(con(int_name)), Box::new(con(int_name))));
    let def = canon::Def::Untyped {
        name: Located::new(Span::new(60, 61), f),
        patterns: Vec::new(),
        body: int(Span::new(62, 63), 0),
    };
    let res = run(Vec::new(), vec![def], env, &i);
    assert_eq!(
        single_func(&res).map(|fc| &fc.ret),
        Some(&IrType::Fun(vec![IrType::Int], Box::new(IrType::Int))),
        "function-typed binding must lower its return to Fun([Int], Int): {res:?}"
    );
    Ok(())
}

#[test]
fn bare_function_reference_lowers_to_func_value() -> DResult<()> {
    // A kernel named as a bare *value* (not called) reifies into a first-class
    // `Expr::FuncValue` carrying its callee and boxed function type — the M1
    // first-class-function value path, not an unsupported-feature diagnostic.
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let module = i.intern("String")?;
    let fname = i.intern("fromInt")?;
    let int_name = i.intern("Int")?;
    let string_name = i.intern("String")?;
    // `f` is annotated with the function type `Int -> String` so the reified
    // kernel value's shape is consistent end-to-end.
    let ty = canon::Type::Lambda(Box::new(con_int(&mut i)?), {
        Box::new(canon::Type::Con {
            home: Vec::new(),
            name: string_name,
            args: Vec::new(),
        })
    });
    let body_span = Span::new(72, 79);
    // body is a bare kernel reference, not a call.
    let body = Located::new(
        body_span,
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
    // The solver records the kernel reference's region type `Int -> String`.
    let con = |name| Ty::Con {
        module: Vec::new(),
        name,
        args: Vec::new(),
    };
    let mut regions = BTreeMap::new();
    regions.insert(
        body_span,
        Ty::Fun(Box::new(con(int_name)), Box::new(con(string_name))),
    );

    let res = run_with_regions(Vec::new(), vec![def], BTreeMap::new(), regions, &i);
    let body = single_func(&res).map(|fc| &fc.body);
    assert_eq!(
        body,
        Some(&Expr::FuncValue {
            callee: Callee::Kernel(KernelFn::StringFromInt),
            ty: IrType::Fun(vec![IrType::Int], Box::new(IrType::Str)),
        }),
        "a bare kernel reference must lower to FuncValue: {res:?}"
    );
    Ok(())
}

#[test]
fn function_value_in_record_field_is_unsupported() -> DResult<()> {
    // Storing a function value in a record field can't compile (a boxed `dyn Fn`
    // satisfies none of the record struct's derived `Clone`/`Debug`/`PartialEq`),
    // so it lowers to the SKY-L0107 first-class-function gap — blaming the field
    // value's span — rather than emitting Rust that does not build.
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let field = i.intern("step")?;
    let int_name = i.intern("Int")?;
    let ty = con_int(&mut i)?;
    // body: `{ step = inc }` where `inc` is a kernel named as a value. The field
    // value's region type is the function type, which trips the gate.
    let field_span = Span::new(82, 85);
    let module = i.intern("String")?;
    let fname = i.intern("fromInt")?;
    let field_value = Located::new(
        field_span,
        canon::Expr_::VarKernel {
            module,
            name: fname,
        },
    );
    let body = Located::new(
        Span::new(80, 90),
        canon::Expr_::Record(vec![(field, field_value)]),
    );
    let def = canon::Def::Typed {
        name: Located::new(Span::new(70, 71), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    let con = |name| Ty::Con {
        module: Vec::new(),
        name,
        args: Vec::new(),
    };
    let mut regions = BTreeMap::new();
    regions.insert(
        field_span,
        Ty::Fun(Box::new(con(int_name)), Box::new(con(int_name))),
    );
    assert_unsupported(
        run_with_regions(Vec::new(), vec![def], BTreeMap::new(), regions, &i),
        Feature::FirstClassFunctions,
        SKY_L0107,
        field_span,
    );
    Ok(())
}

#[test]
fn value_callee_lowers_to_apply() -> DResult<()> {
    // A call whose callee is not a kernel/top-level name (here a local that
    // would hold a function value) lowers to `Expr::Apply`, the first-class
    // application path — distinct from the direct `Expr::Call` callee path.
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let g = i.intern("g")?;
    let ty = con_int(&mut i)?;
    let callee_ref = Box::new(Located::new(Span::new(82, 83), canon::Expr_::VarLocal(g)));
    let arg = int(Span::new(84, 85), 5);
    let body = Located::new(Span::new(82, 90), canon::Expr_::Call(callee_ref, vec![arg]));
    let def = canon::Def::Typed {
        name: Located::new(Span::new(80, 81), f),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty,
    };
    let res = run(Vec::new(), vec![def], BTreeMap::new(), &i);
    let body = single_func(&res).map(|fc| &fc.body);
    assert!(
        matches!(
            body,
            Some(Expr::Apply { func, args })
                if matches!(func.as_ref(), Expr::Var(s) if *s == g) && args.len() == 1
        ),
        "a value callee must lower to Apply over the local, got {body:?}"
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
    let callee_ref = Box::new(Located::new(
        Span::new(92, 100),
        canon::Expr_::VarKernel {
            module,
            name: fname,
        },
    ));
    let body = Located::new(
        Span::new(92, 102),
        canon::Expr_::Call(callee_ref, Vec::new()),
    );
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
    let op = i.intern("++")?;
    let home = i.intern("Basics")?;
    let func = i.intern("append")?;
    let ty = con_int(&mut i)?;
    // 1 ++ 2 — list/string operators (`++` → append, `::` → cons) await those
    // types; the M1-core arithmetic/comparison/boolean set is supported.
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

#[test]
fn partial_application_of_top_level_fn() -> DResult<()> {
    // `add` declares two parameters; `add 2` passes one. Partial application
    // cannot lower to a saturated `Expr::Call`, so it fails closed with
    // SKY-L0110 (carrying the call's span), never broken Rust. [fix: M1 b4]
    let mut i = Interner::new();
    let add = i.intern("add")?;
    let caller = i.intern("caller")?;
    let a = i.intern("a")?;
    let b = i.intern("b")?;
    // add : Int -> Int -> Int   (two parameters)
    let add_ty = canon::Type::Lambda(
        Box::new(con_int(&mut i)?),
        Box::new(canon::Type::Lambda(
            Box::new(con_int(&mut i)?),
            Box::new(con_int(&mut i)?),
        )),
    );
    let add_def = canon::Def::Typed {
        name: Located::new(Span::new(10, 13), add),
        free_vars: Vec::new(),
        patterns: vec![
            Located::new(Span::new(14, 15), canon::Pattern_::PVar(a)),
            Located::new(Span::new(16, 17), canon::Pattern_::PVar(b)),
        ],
        body: int(Span::new(20, 21), 0),
        ty: add_ty,
    };
    // caller : Int -> Int   — its body `add 2` is a one-argument call.
    let call_span = Span::new(40, 45);
    let callee_ref = Box::new(Located::new(
        Span::new(40, 43),
        canon::Expr_::VarTopLevel {
            module: Vec::new(),
            name: add,
        },
    ));
    let body = Located::new(
        call_span,
        canon::Expr_::Call(callee_ref, vec![int(Span::new(44, 45), 2)]),
    );
    let caller_ty = canon::Type::Lambda(Box::new(con_int(&mut i)?), Box::new(con_int(&mut i)?));
    let caller_def = canon::Def::Typed {
        name: Located::new(Span::new(30, 36), caller),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty: caller_ty,
    };
    // The under-saturated call site is blamed.
    assert_unsupported(
        run(Vec::new(), vec![add_def, caller_def], BTreeMap::new(), &i),
        Feature::PartialOverApplication,
        SKY_L0110,
        call_span,
    );
    Ok(())
}

#[test]
fn over_application_of_top_level_fn() -> DResult<()> {
    // `f` declares one parameter; `f 1 2` passes two — over-application across
    // the arity boundary. The first call would saturate, but the extra argument
    // cannot lower to a saturated `Expr::Call`, so it fails closed with
    // SKY-L0110 rather than emit `f(1, 2)` that the Rust toolchain rejects.
    let mut i = Interner::new();
    let f = i.intern("f")?;
    let caller = i.intern("caller")?;
    let x = i.intern("x")?;
    // f : Int -> Int   (one parameter)
    let f_ty = canon::Type::Lambda(Box::new(con_int(&mut i)?), Box::new(con_int(&mut i)?));
    let f_def = canon::Def::Typed {
        name: Located::new(Span::new(10, 11), f),
        free_vars: Vec::new(),
        patterns: vec![Located::new(Span::new(12, 13), canon::Pattern_::PVar(x))],
        body: int(Span::new(16, 17), 0),
        ty: f_ty,
    };
    // caller : Int   — its body `f 1 2` is a two-argument call of a unary `f`.
    let call_span = Span::new(40, 47);
    let callee_ref = Box::new(Located::new(
        Span::new(40, 41),
        canon::Expr_::VarTopLevel {
            module: Vec::new(),
            name: f,
        },
    ));
    let body = Located::new(
        call_span,
        canon::Expr_::Call(
            callee_ref,
            vec![int(Span::new(44, 45), 1), int(Span::new(46, 47), 2)],
        ),
    );
    let caller_def = canon::Def::Typed {
        name: Located::new(Span::new(30, 36), caller),
        free_vars: Vec::new(),
        patterns: Vec::new(),
        body,
        ty: con_int(&mut i)?,
    };
    // The over-saturated call site is blamed.
    assert_unsupported(
        run(Vec::new(), vec![f_def, caller_def], BTreeMap::new(), &i),
        Feature::PartialOverApplication,
        SKY_L0110,
        call_span,
    );
    Ok(())
}
