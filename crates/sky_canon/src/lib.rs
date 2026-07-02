#![forbid(unsafe_code)]
//! `sky_canon` — name resolution / canonicalisation for the Milestone-0 subset
//! of Sky.
//!
//! Entry point: [`canonicalise`]. It consumes a [`sky_syntax::Module`] (the raw
//! parse tree) plus a mutable [`Interner`] and produces a name-resolved
//! [`ast::Module`], or a typed [`sky_diagnostics::Diagnostic`]. Every variable
//! reference is classified — local binding, top-level binding, stdlib kernel,
//! or data constructor — by porting the M0 subset of the Haskell compiler's
//! `Sky.Canonicalise.{Module,Expression,Pattern,Type,Environment}`.

pub mod ast;
mod env;
pub mod link;
mod resolve;

use std::collections::{BTreeMap, BTreeSet};

use sky_diagnostics::DResult;
use sky_intern::{Interner, Symbol};

pub use env::{CtorHome, Env, VarHome};

/// A type alias exported by a module in its raw (unresolved) source form.
///
/// Carried in [`ModuleExports`] so importing modules can inject it into their
/// own alias table and expand it there. Fields mirror the private `AliasDef`
/// in `resolve.rs`; the public counterpart lets the multi-module driver pass
/// exports across the boundary without exposing resolver internals.
#[derive(Clone, Debug)]
pub struct ExportedAlias {
    /// Declared type-parameter names, in source order.
    pub params: Vec<Symbol>,
    /// The right-hand-side of the `type alias` declaration, kept unresolved.
    pub body: sky_syntax::TypeAnnotation,
}

/// The public exports of a canonicalised module: the names and resolved
/// locations of every value, type, constructor, and alias the module exposes
/// via its `exposing` list.
///
/// Used by [`canonicalise_module`] as the `deps` map entries so importing
/// modules can inject the right resolved names into their environments.
#[derive(Clone, Debug, Default)]
pub struct ModuleExports {
    /// The module's own path, e.g. `[Lib, Utils]`.
    pub path: Vec<Symbol>,
    /// Exported value names (without their resolved `VarHome`; the home is
    /// always `TopLevel(path)`, reconstructed at injection time).
    pub values: BTreeSet<Symbol>,
    /// Exported type names mapped to their home module path. For a type
    /// `Widget` declared in `Lib.Utils`, this entry is `Widget → [Lib, Utils]`.
    pub types: BTreeMap<Symbol, Vec<Symbol>>,
    /// Exported constructors by name.
    pub ctors: BTreeMap<Symbol, CtorHome>,
    /// Exported type aliases by name.
    pub aliases: BTreeMap<Symbol, ExportedAlias>,
}

/// Canonicalise a parsed module into its name-resolved canonical AST.
///
/// # Errors
/// Returns a [`sky_diagnostics::Diagnostic`] for any name that resolves to
/// neither a constructor, a bound variable, a top-level binding, nor a kernel
/// function (an [`sky_diagnostics::NameError`] payload variant carrying a
/// deterministic did-you-mean), or for a duplicated value/constructor/type
/// name.
pub fn canonicalise(m: &sky_syntax::Module, interner: &mut Interner) -> DResult<ast::Module> {
    resolve::canonicalise(m, interner)
}

/// Canonicalise a module in a multi-module project context.
///
/// Unlike [`canonicalise`], this function:
/// * validates `m`'s declared module name against `expected_path` — emits
///   [`sky_diagnostics::NameError::ModulePathMismatch`] when they disagree
/// * rejects `Sky` / `Std` as the first path segment — emits
///   [`sky_diagnostics::NameError::ReservedNamespace`]
/// * resolves each local `import` against `deps`, injecting exports into the
///   name-resolution environment — emits
///   [`sky_diagnostics::NameError::ModuleNotFound`] /
///   [`sky_diagnostics::NameError::NameNotExposed`] /
///   [`sky_diagnostics::NameError::AmbiguousImport`] on violations
/// * returns the resolved [`ast::Module`] plus a [`ModuleExports`] summary
///   derived from the module's own `exposing` list
///
/// # Errors
/// Any of the above [`sky_diagnostics::NameError`] variants, or any error that
/// [`canonicalise`] can return.
pub fn canonicalise_module(
    m: &sky_syntax::Module,
    expected_path: &[Symbol],
    deps: &BTreeMap<Vec<Symbol>, ModuleExports>,
    interner: &mut Interner,
) -> DResult<(ast::Module, ModuleExports)> {
    resolve::canonicalise_module(m, expected_path, deps, interner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast::{Def, Expr, Expr_, Pattern_};
    use sky_diagnostics::{Diagnostic, NameError};
    use sky_intern::Symbol;

    const GOLDEN: &str = include_str!("../../../tests/golden/m0/Main.sky");

    /// Parse + canonicalise the golden M0 module. Returns `None` (failing the
    /// caller's assertions) rather than panicking, per the no-panic gate.
    fn canon_golden(i: &mut Interner) -> Option<ast::Module> {
        let src = sky_parse::parse_module(GOLDEN, i).ok()?;
        canonicalise(&src, i).ok()
    }

    fn find_def<'a>(m: &'a ast::Module, i: &Interner, name: &str) -> Option<&'a Def> {
        m.defs
            .iter()
            .find(|d| i.resolve(d.name().value) == Some(name))
    }

    /// Drill into a [`Call`] node, returning callee + args.
    fn as_call(e: &Expr) -> Option<(&Expr_, &[Expr])> {
        match &e.value {
            Expr_::Call(callee, args) => Some((&callee.value, args)),
            _ => None,
        }
    }

    /// Parse + canonicalise inline source, returning the module + interner.
    fn canon_src(src: &str) -> Option<(ast::Module, Interner)> {
        let mut i = Interner::new();
        let parsed = sky_parse::parse_module(src, &mut i).ok()?;
        let m = canonicalise(&parsed, &mut i).ok()?;
        Some((m, i))
    }

    #[test]
    fn lambda_binds_params_locally_and_captures_outer_names() {
        // `f = \x -> x + n` (with top-level `n`): inside the lambda body `x`
        // resolves to a local (the parameter) and `n` to the captured top-level
        // binding.
        let src = "module Main exposing (f)\n\
                   n : Int\n\
                   n = 10\n\
                   f =\n    \\x -> x + n\n";
        let opt = canon_src(src);
        assert!(opt.is_some(), "must parse + canonicalise");
        let Some((m, i)) = opt else { return };
        let def = find_def(&m, &i, "f");
        assert!(matches!(def, Some(Def::Untyped { .. })), "f is untyped");
        let Some(Def::Untyped { body, .. }) = def else {
            return;
        };
        assert!(
            matches!(&body.value, Expr_::Lambda(..)),
            "f body is a lambda"
        );
        let Expr_::Lambda(params, lam_body) = &body.value else {
            return;
        };
        assert_eq!(params.len(), 1, "one parameter");
        assert!(
            matches!(params.first().map(|p| &p.value), Some(Pattern_::PVar(s)) if i.resolve(*s) == Some("x"))
        );
        // The body `x + n`: x is a local, n is the captured top-level binding.
        assert!(
            matches!(&lam_body.value, Expr_::Binop { .. }),
            "body is x + n"
        );
        let Expr_::Binop { lhs, rhs, .. } = &lam_body.value else {
            return;
        };
        assert!(matches!(lhs.value, Expr_::VarLocal(s) if i.resolve(s) == Some("x")));
        assert!(
            matches!(&rhs.value, Expr_::VarTopLevel { name, .. } if i.resolve(*name) == Some("n"))
        );
    }

    #[test]
    fn module_name_and_union_resolve() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden must parse + canonicalise");
        let Some(m) = m else { return };

        assert_eq!(m.name.len(), 1);
        assert_eq!(m.name.first().and_then(|&s| i.resolve(s)), Some("Main"));

        // The `Msg` union with two nullary constructors.
        assert_eq!(m.unions.len(), 1);
        let Some(union) = m.unions.first() else {
            return;
        };
        assert_eq!(i.resolve(union.name), Some("Msg"));
        let names: Vec<(&str, usize)> = union
            .ctors
            .iter()
            .filter_map(|c| i.resolve(c.name).map(|n| (n, c.index)))
            .collect();
        assert_eq!(names, vec![("Increment", 0), ("Decrement", 1)]);
    }

    #[test]
    fn update_body_resolves_locals_and_ctor_patterns() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden");
        let Some(m) = m else { return };

        let def = find_def(&m, &i, "update");
        assert!(
            matches!(def, Some(Def::Typed { .. })),
            "update is a typed def"
        );
        let Some(Def::Typed { patterns, body, .. }) = def else {
            return;
        };
        assert_eq!(patterns.len(), 2);

        // case msg of ...
        assert!(
            matches!(&body.value, Expr_::Case(..)),
            "update body is a case"
        );
        let Expr_::Case(scrut, branches) = &body.value else {
            return;
        };
        assert!(matches!(scrut.value, Expr_::VarLocal(s) if i.resolve(s) == Some("msg")));
        assert_eq!(branches.len(), 2);

        // First arm: `Increment -> count + 1`.
        let Some(inc) = branches.first() else { return };
        assert!(
            matches!(&inc.pat.value, Pattern_::PCtor { .. }),
            "arm pattern is a ctor"
        );
        let Pattern_::PCtor {
            type_name,
            name,
            index,
            ..
        } = &inc.pat.value
        else {
            return;
        };
        assert_eq!(i.resolve(*type_name), Some("Msg"));
        assert_eq!(i.resolve(*name), Some("Increment"));
        assert_eq!(*index, 0);

        // Body `count + 1` → Binop resolving to Basics.add over a local lhs.
        assert!(
            matches!(&inc.body.value, Expr_::Binop { .. }),
            "arm body is a binop"
        );
        let Expr_::Binop {
            home, func, lhs, ..
        } = &inc.body.value
        else {
            return;
        };
        assert_eq!(i.resolve(*home), Some("Basics"));
        assert_eq!(i.resolve(*func), Some("add"));
        assert!(matches!(lhs.value, Expr_::VarLocal(s) if i.resolve(s) == Some("count")));

        // Second arm resolves `-` to Basics.sub.
        let Some(dec) = branches.get(1) else { return };
        assert!(
            matches!(&dec.body.value, Expr_::Binop { .. }),
            "arm body is a binop"
        );
        let Expr_::Binop { func, .. } = &dec.body.value else {
            return;
        };
        assert_eq!(i.resolve(*func), Some("sub"));
    }

    #[test]
    fn main_body_resolves_kernel_toplevel_and_ctor() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden");
        let Some(m) = m else { return };

        let def = find_def(&m, &i, "main");
        assert!(
            matches!(def, Some(Def::Untyped { .. })),
            "main is an untyped def"
        );
        let Some(Def::Untyped { body, .. }) = def else {
            return;
        };

        // main = println (String.fromInt (update Increment 0))
        let outer = as_call(body);
        assert!(
            matches!(outer, Some((Expr_::VarKernel { .. }, _))),
            "main body is a call to a kernel"
        );
        let Some((Expr_::VarKernel { module, name }, outer_args)) = outer else {
            return;
        };
        assert_eq!(i.resolve(*module), Some("Log"));
        assert_eq!(i.resolve(*name), Some("println"));
        assert_eq!(outer_args.len(), 1);

        // String.fromInt → VarKernel { String, fromInt }.
        let Some(arg0) = outer_args.first() else {
            return;
        };
        let mid = as_call(arg0);
        assert!(
            matches!(mid, Some((Expr_::VarKernel { .. }, _))),
            "arg is a call to a kernel"
        );
        let Some((Expr_::VarKernel { module, name }, mid_args)) = mid else {
            return;
        };
        assert_eq!(i.resolve(*module), Some("String"));
        assert_eq!(i.resolve(*name), Some("fromInt"));

        // update Increment 0 → VarTopLevel update applied to VarCtor + Int.
        let Some(mid0) = mid_args.first() else { return };
        let inner = as_call(mid0);
        assert!(
            matches!(inner, Some((Expr_::VarTopLevel { .. }, _))),
            "arg is a call to a top-level"
        );
        let Some((Expr_::VarTopLevel { module, name }, inner_args)) = inner else {
            return;
        };
        assert_eq!(module.first().and_then(|&s| i.resolve(s)), Some("Main"));
        assert_eq!(i.resolve(*name), Some("update"));
        assert_eq!(inner_args.len(), 2);

        // `Increment` used as a value → VarCtor of Main.Msg.
        let Some(ctor_arg) = inner_args.first() else {
            return;
        };
        assert!(
            matches!(&ctor_arg.value, Expr_::VarCtor { .. }),
            "Increment is a ctor value"
        );
        let Expr_::VarCtor {
            type_name,
            name,
            index,
            home,
        } = &ctor_arg.value
        else {
            return;
        };
        assert_eq!(i.resolve(*type_name), Some("Msg"));
        assert_eq!(i.resolve(*name), Some("Increment"));
        assert_eq!(*index, 0);
        assert_eq!(home.first().and_then(|&s| i.resolve(s)), Some("Main"));

        // `0` literal.
        assert!(matches!(
            inner_args.get(1).map(|a| &a.value),
            Some(Expr_::Int(0))
        ));
    }

    #[test]
    fn typed_def_carries_arrow_annotation() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden");
        let Some(m) = m else { return };

        let def = find_def(&m, &i, "update");
        assert!(matches!(def, Some(Def::Typed { .. })), "update is typed");
        let Some(Def::Typed { ty, free_vars, .. }) = def else {
            return;
        };
        // No type variables in `Msg -> Int -> Int`.
        assert!(free_vars.is_empty());
        // Outer arrow: Msg -> (Int -> Int).
        assert!(
            matches!(ty, ast::Type::Lambda(_, _)),
            "annotation is an arrow"
        );
        let ast::Type::Lambda(arg, rest) = ty else {
            return;
        };
        assert!(
            matches!(arg.as_ref(), ast::Type::Con { .. }),
            "first arg is a constructor type"
        );
        let ast::Type::Con { name, home, .. } = arg.as_ref() else {
            return;
        };
        assert_eq!(i.resolve(*name), Some("Msg"));
        // `Msg` is a local union → home is this module.
        assert_eq!(home.first().and_then(|&s| i.resolve(s)), Some("Main"));
        // Tail is Int -> Int.
        assert!(matches!(rest.as_ref(), ast::Type::Lambda(_, _)));
    }

    /// Parse `src_text` and canonicalise it, returning the diagnostic (if any).
    /// Returns `None` from the parse step rather than panicking.
    fn canon_err(src_text: &str) -> Option<Diagnostic> {
        let mut i = Interner::new();
        let src = sky_parse::parse_module(src_text, &mut i).ok()?;
        canonicalise(&src, &mut i).err()
    }

    #[test]
    fn unknown_name_is_a_value_not_found() {
        let err = canon_err("module Main exposing (main)\n\nmain = nope\n");
        assert!(matches!(
            err,
            Some(Diagnostic::Name {
                msg: NameError::ValueNotFound { .. },
                ..
            })
        ));
    }

    #[test]
    fn unknown_value_suggests_close_name() {
        // `printn` is one edit from the in-scope kernel value `println`.
        let err = canon_err("module Main exposing (main)\n\nmain = printn\n");
        assert!(
            matches!(
                &err,
                Some(Diagnostic::Name {
                    msg: NameError::ValueNotFound { .. },
                    ..
                })
            ),
            "expected ValueNotFound, got {err:?}"
        );
        let Some(Diagnostic::Name {
            msg: NameError::ValueNotFound { name, suggestions },
            ..
        }) = err
        else {
            return;
        };
        assert_eq!(&*name, "printn");
        assert!(
            suggestions.iter().any(|s| &**s == "println"),
            "suggestions should include `println`, got {suggestions:?}"
        );
    }

    #[test]
    fn unknown_value_far_from_everything_has_no_suggestions() {
        // `zzzzzzzz` is > 2 edits from every in-scope name → silence.
        let err = canon_err("module Main exposing (main)\n\nmain = zzzzzzzz\n");
        let Some(Diagnostic::Name {
            msg: NameError::ValueNotFound { suggestions, .. },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected ValueNotFound");
            return;
        };
        assert!(
            suggestions.is_empty(),
            "no suggestion within edit-distance 2, got {suggestions:?}"
        );
    }

    #[test]
    fn suggestions_sorted_by_distance_then_name() {
        // Several `List`/`Basics` members sit at equal edit distance from
        // `ma`; assert the rendered list is `(distance, name)`-sorted.
        let err = canon_err("module Main exposing (main)\n\nmain = List.ma\n");
        let Some(Diagnostic::Name {
            msg: NameError::NoSuchMember { suggestions, .. },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected NoSuchMember");
            return;
        };
        let keys: Vec<(usize, String)> = suggestions
            .iter()
            .map(|s| (test_levenshtein("ma", s), s.to_string()))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "suggestions must be (distance, name)-sorted");
    }

    #[test]
    fn unknown_qualifier_is_unknown_module() {
        let err = canon_err("module Main exposing (main)\n\nmain = Strng.fromInt\n");
        let Some(Diagnostic::Name {
            msg:
                NameError::UnknownModule {
                    qualifier,
                    suggestions,
                },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected UnknownModule");
            return;
        };
        assert_eq!(&*qualifier, "Strng");
        assert!(
            suggestions.iter().any(|s| &**s == "String"),
            "should suggest `String`, got {suggestions:?}"
        );
    }

    #[test]
    fn known_qualifier_missing_member_is_no_such_member() {
        // `fromInr` is one edit (substitution) from the `String` member
        // `fromInt`.
        let err = canon_err("module Main exposing (main)\n\nmain = String.fromInr\n");
        let Some(Diagnostic::Name {
            msg:
                NameError::NoSuchMember {
                    module,
                    member,
                    suggestions,
                },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected NoSuchMember");
            return;
        };
        assert_eq!(&*module, "String");
        assert_eq!(&*member, "fromInr");
        assert!(
            suggestions.iter().any(|s| &**s == "fromInt"),
            "should suggest `fromInt`, got {suggestions:?}"
        );
    }

    #[test]
    fn unknown_constructor_pattern_is_constructor_not_found() {
        let src = "module Main exposing (main)\n\n\
                   type Msg = Increment | Decrement\n\n\
                   f x =\n    case x of\n        Incremen -> 0\n\n\
                   main = f Increment\n";
        let err = canon_err(src);
        let Some(Diagnostic::Name {
            msg: NameError::ConstructorNotFound { name, suggestions },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected ConstructorNotFound, got {err:?}");
            return;
        };
        assert_eq!(&*name, "Incremen");
        assert!(
            suggestions.iter().any(|s| &**s == "Increment"),
            "should suggest `Increment`, got {suggestions:?}"
        );
    }

    #[test]
    fn duplicate_value_points_at_both_spans() {
        let src = "module Main exposing (main)\n\nmain = 1\n\nmain = 2\n";
        let err = canon_err(src);
        let Some(Diagnostic::Name {
            span,
            msg: NameError::DuplicateValue { name, first },
        }) = err
        else {
            assert!(false_marker(), "expected DuplicateValue, got {err:?}");
            return;
        };
        assert_eq!(&*name, "main");
        // The second definition (primary) is strictly after the first.
        assert!(
            first.lo < span.lo,
            "first span {first:?} must precede the duplicate {span:?}"
        );
    }

    #[test]
    fn duplicate_type_points_at_both_spans() {
        let src = "module Main exposing (main)\n\n\
                   type Msg = A\n\ntype Msg = B\n\nmain = 0\n";
        let err = canon_err(src);
        let Some(Diagnostic::Name {
            span,
            msg: NameError::DuplicateType { name, first },
        }) = err
        else {
            assert!(false_marker(), "expected DuplicateType, got {err:?}");
            return;
        };
        assert_eq!(&*name, "Msg");
        assert!(first.lo < span.lo, "first span precedes duplicate");
    }

    #[test]
    fn duplicate_constructor_across_unions_points_at_both_spans() {
        // Same constructor name `A` in two distinct unions.
        let src = "module Main exposing (main)\n\n\
                   type Foo = A\n\ntype Bar = A\n\nmain = 0\n";
        let err = canon_err(src);
        let Some(Diagnostic::Name {
            span,
            msg: NameError::DuplicateConstructor { name, first },
        }) = err
        else {
            assert!(false_marker(), "expected DuplicateConstructor, got {err:?}");
            return;
        };
        assert_eq!(&*name, "A");
        assert!(first.lo < span.lo, "first span precedes duplicate");
    }

    #[test]
    fn free_type_vars_ordered_by_name_not_symbol_id() {
        // Source order of the tyvars is `z`, `a`; an id-ordered result would be
        // `[z, a]`, but the name order is `[a, z]`.
        let src = "module Main exposing (main)\n\n\
                   f : z -> a -> z\nf x y = x\n\nmain = 0\n";
        let mut i = Interner::new();
        let parsed = sky_parse::parse_module(src, &mut i);
        assert!(parsed.is_ok(), "source parses");
        let Ok(srcm) = parsed else { return };
        let m = canonicalise(&srcm, &mut i);
        assert!(m.is_ok(), "canonicalises: {m:?}");
        let Ok(m) = m else { return };
        let def = m
            .defs
            .iter()
            .find(|d| i.resolve(d.name().value) == Some("f"));
        let Some(Def::Typed { free_vars, .. }) = def else {
            assert!(false_marker(), "f is a typed def");
            return;
        };
        let names: Vec<&str> = free_vars.iter().filter_map(|&v| i.resolve(v)).collect();
        assert_eq!(names, vec!["a", "z"], "free vars sorted by name");
    }

    /// A runtime `false` the optimiser cannot fold, so `assert!(false_marker())`
    /// fails the test (the desired "wrong variant" signal) without tripping
    /// `clippy::assertions_on_constants`, which fires on a literal `false`.
    fn false_marker() -> bool {
        std::hint::black_box(false)
    }

    /// Stand-alone Levenshtein for the ordering assertion, kept separate from
    /// the production helper (which is private to `resolve`).
    fn test_levenshtein(a: &str, b: &str) -> usize {
        let bc: Vec<char> = b.chars().collect();
        let mut prev: Vec<usize> = (0..=bc.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut curr = vec![i + 1];
            let mut diag = i;
            for (cb, &up) in bc.iter().zip(prev.iter().skip(1)) {
                let cost = usize::from(ca != *cb);
                let left = curr.last().copied().unwrap_or(i + 1);
                curr.push((up + 1).min(left + 1).min(diag + cost));
                diag = up;
            }
            prev = curr;
        }
        prev.last().copied().unwrap_or(0)
    }

    /// Parse + canonicalise a free-standing module body, returning the resolved
    /// body expression of the binding named `which`.
    fn canon_body(i: &mut Interner, source: &str, which: &str) -> Option<Expr_> {
        let src = sky_parse::parse_module(source, i).ok()?;
        let m = canonicalise(&src, i).ok()?;
        let def = find_def(&m, i, which)?;
        match def {
            Def::Typed { body, .. } | Def::Untyped { body, .. } => Some(body.value.clone()),
        }
    }

    /// Destructure a resolved binop into `(func-name, lhs, rhs)`.
    fn as_binop<'a>(i: &Interner, e: &'a Expr_) -> Option<(String, &'a Expr, &'a Expr)> {
        match e {
            Expr_::Binop { func, lhs, rhs, .. } => Some((i.resolve(*func)?.to_owned(), lhs, rhs)),
            _ => None,
        }
    }

    #[test]
    fn mul_binds_tighter_than_add() {
        // `2 + 3 * 4` must associate as `add(2, mul(3, 4))`, never `mul(add(2,3), 4)`.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv : Int\nv =\n    2 + 3 * 4\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(body) = body else { return };
        let top = as_binop(&i, &body);
        assert!(top.is_some(), "top is a binop");
        let Some((top, lhs, rhs)) = top else { return };
        assert_eq!(top, "add", "outer op is +");
        assert!(matches!(lhs.value, Expr_::Int(2)), "lhs is literal 2");
        let inner = as_binop(&i, &rhs.value);
        assert!(inner.is_some(), "rhs is the * subtree");
        let Some((inner, il, ir)) = inner else { return };
        assert_eq!(inner, "mul", "inner op is *");
        assert!(matches!(il.value, Expr_::Int(3)));
        assert!(matches!(ir.value, Expr_::Int(4)));
    }

    #[test]
    fn left_associative_subtraction_chains_left() {
        // `10 - 3 - 2` is `sub(sub(10, 3), 2)` (left-assoc), not `sub(10, sub(3, 2))`.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv : Int\nv =\n    10 - 3 - 2\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(body) = body else { return };
        let top = as_binop(&i, &body);
        assert!(top.is_some(), "top is a binop");
        let Some((top, lhs, rhs)) = top else { return };
        assert_eq!(top, "sub");
        assert!(
            matches!(rhs.value, Expr_::Int(2)),
            "rhs is the last operand"
        );
        assert_eq!(
            as_binop(&i, &lhs.value).map(|t| t.0),
            Some("sub".to_owned())
        );
    }

    #[test]
    fn comparison_below_arithmetic_and_above_boolean() {
        // `n > 10 && n < 100` ⇒ `and(gt(n, 10), lt(n, 100))`: `&&` is the root,
        // each comparison its own subtree (comparison binds tighter than `&&`).
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (f)\nf : Int -> Bool\nf n =\n    n > 10 && n < 100\n",
            "f",
        );
        assert!(body.is_some(), "f must canonicalise");
        let Some(body) = body else { return };
        let top = as_binop(&i, &body);
        assert!(top.is_some(), "top is a binop");
        let Some((top, lhs, rhs)) = top else { return };
        assert_eq!(top, "and", "root is &&");
        assert_eq!(as_binop(&i, &lhs.value).map(|t| t.0), Some("gt".to_owned()));
        assert_eq!(as_binop(&i, &rhs.value).map(|t| t.0), Some("lt".to_owned()));
    }

    #[test]
    fn parenthesised_group_is_not_reassociated() {
        // `(2 + 3) * 4` ⇒ `mul(add(2, 3), 4)`. Parens override precedence.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv : Int\nv =\n    (2 + 3) * 4\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(body) = body else { return };
        let top = as_binop(&i, &body);
        assert!(top.is_some(), "top is a binop");
        let Some((top, lhs, rhs)) = top else { return };
        assert_eq!(top, "mul", "root is *");
        assert!(matches!(rhs.value, Expr_::Int(4)));
        assert_eq!(
            as_binop(&i, &lhs.value).map(|t| t.0),
            Some("add".to_owned())
        );
    }

    #[test]
    fn or_is_right_associative() {
        // `a || b || c` ⇒ `or(a, or(b, c))` (right-assoc, prec 2).
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (f)\nf : Bool -> Bool -> Bool -> Bool\nf a b c =\n    a || b || c\n",
            "f",
        );
        assert!(body.is_some(), "f must canonicalise");
        let Some(body) = body else { return };
        let top = as_binop(&i, &body);
        assert!(top.is_some(), "top is a binop");
        let Some((top, lhs, rhs)) = top else { return };
        assert_eq!(top, "or");
        assert!(
            matches!(lhs.value, Expr_::VarLocal(_)),
            "lhs is the lone `a`"
        );
        assert_eq!(as_binop(&i, &rhs.value).map(|t| t.0), Some("or".to_owned()));
    }

    #[test]
    fn append_is_right_associative_and_maps_to_append_kernel() {
        // `a ++ b ++ c` ⇒ `append(a, append(b, c))` (right-assoc, prec 5), and
        // the `++` operator resolves to the `append` kernel.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (f)\nf : String -> String -> String -> String\nf a b c =\n    a ++ b ++ c\n",
            "f",
        );
        assert!(body.is_some(), "f must canonicalise");
        let Some(body) = body else { return };
        let top = as_binop(&i, &body);
        assert!(top.is_some(), "top is a binop");
        let Some((top, lhs, rhs)) = top else { return };
        assert_eq!(top, "append", "`++` resolves to the append kernel");
        assert!(
            matches!(lhs.value, Expr_::VarLocal(_)),
            "lhs is the lone `a` (right-assoc keeps the tail nested)"
        );
        assert_eq!(
            as_binop(&i, &rhs.value).map(|t| t.0),
            Some("append".to_owned()),
            "the right operand is itself an append"
        );
    }

    #[test]
    fn let_binds_names_as_locals() {
        // `let x = 2 in x + x` → a `Let` whose in-body is a Binop over the
        // let-bound local `x`.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv : Int\nv =\n    let x = 2 in x + x\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(Expr_::Let(bindings, in_body)) = body else {
            assert!(false_marker(), "v body is a Let");
            return;
        };
        assert_eq!(bindings.len(), 1, "one binding");
        assert!(
            bindings.first().is_some_and(|b| matches!(
                &b.pat.value,
                Pattern_::PVar(s) if i.resolve(*s) == Some("x")
            )),
            "binding name is x"
        );
        let Some((func, lhs, rhs)) = as_binop(&i, &in_body.value) else {
            assert!(false_marker(), "in-body is a binop");
            return;
        };
        assert_eq!(func, "add");
        assert!(matches!(lhs.value, Expr_::VarLocal(s) if i.resolve(s) == Some("x")));
        assert!(matches!(rhs.value, Expr_::VarLocal(s) if i.resolve(s) == Some("x")));
    }

    #[test]
    fn let_later_binding_sees_earlier() {
        // Sequential (`let*`) scoping: `b = a` resolves `a` to the earlier
        // let-bound local, not to an error.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv : Int\nv =\n    let\n        a = 1\n        b = a\n    in\n    b\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(Expr_::Let(bindings, _)) = body else {
            assert!(false_marker(), "v body is a Let");
            return;
        };
        let second = bindings.get(1);
        assert!(
            second.is_some_and(
                |b| matches!(b.body.value, Expr_::VarLocal(s) if i.resolve(s) == Some("a"))
            ),
            "the second binding's value resolves `a` to a local"
        );
    }

    #[test]
    fn if_resolves_conditions_and_branches() {
        // `if x > 0 then x else 0` over a parameter `x`: the condition and both
        // branches resolve against the same scope (the parameter is in scope in
        // each). `if` introduces no bindings.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (f)\nf : Int -> Int\nf x =\n    if x > 0 then x else 0\n",
            "f",
        );
        assert!(body.is_some(), "f must canonicalise");
        let Some(Expr_::If(branches, els)) = body else {
            assert!(false_marker(), "f body is an If");
            return;
        };
        assert_eq!(branches.len(), 1, "one `(cond, branch)` pair");
        let Some((cond, branch)) = branches.first() else {
            assert!(false_marker(), "the pair is present");
            return;
        };
        // The condition is `x > 0` — a binop reading the local `x`.
        let Some((func, lhs, _)) = as_binop(&i, &cond.value) else {
            assert!(false_marker(), "cond is a binop");
            return;
        };
        assert_eq!(func, "gt", "condition op is >");
        assert!(matches!(lhs.value, Expr_::VarLocal(s) if i.resolve(s) == Some("x")));
        // The `then` branch reads the same local; the `else` is the literal 0.
        assert!(matches!(branch.value, Expr_::VarLocal(s) if i.resolve(s) == Some("x")));
        assert!(matches!(els.value, Expr_::Int(0)));
    }

    #[test]
    fn let_forward_reference_rejects_cleanly() {
        // `y = x` before `x = 2`: with sequential scoping `x` is not yet bound
        // and there is no outer `x`, so it resolves to nothing — a clean
        // ValueNotFound, never a miscompile.
        let err = canon_err(
            "module Main exposing (v)\nv : Int\nv =\n    let\n        y = x\n        x = 2\n    in\n    y\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::ValueNotFound { .. },
                    ..
                })
            ),
            "forward reference must reject as ValueNotFound, got {err:?}"
        );
    }

    #[test]
    fn tuple_canonicalises_element_wise() {
        // `(1, x)` resolves each element against the enclosing scope; the second
        // element is the parameter `x`, bound to a local.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv : Int -> Int\nv x =\n    (1, x)\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(body) = body else { return };
        assert!(
            matches!(&body, Expr_::Tuple(es)
                if es.len() == 2
                    && matches!(es.first().map(|e| &e.value), Some(Expr_::Int(1)))
                    && matches!(es.get(1).map(|e| &e.value), Some(Expr_::VarLocal(_)))),
            "(1, x) resolves to a 2-tuple of Int and a local, got {body:?}"
        );
    }

    #[test]
    fn record_literal_canonicalises_field_wise() {
        // `{ x = 1, y = a }` resolves each field value against scope; the second
        // is the parameter `a`, a local. Field labels are carried unresolved.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv : Int -> Int\nv a =\n    { x = 1, y = a }\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(body) = body else { return };
        assert!(
            matches!(&body, Expr_::Record(fields)
                if fields.len() == 2
                    && matches!(fields.first().map(|(_, e)| &e.value), Some(Expr_::Int(1)))
                    && matches!(fields.get(1).map(|(_, e)| &e.value), Some(Expr_::VarLocal(_)))),
            "`{{ x = 1, y = a }}` resolves to a 2-field Record, got {body:?}"
        );
    }

    #[test]
    fn field_access_canonicalises_over_its_record() {
        // `p.x` resolves the record sub-expression (the local `p`); the field is
        // a label carried unresolved.
        let mut i = Interner::new();
        let body = canon_body(&mut i, "module Main exposing (v)\nv p =\n    p.x\n", "v");
        assert!(body.is_some(), "v must canonicalise");
        let Some(body) = body else { return };
        assert!(
            matches!(&body, Expr_::Access(rec, field)
                if matches!(rec.value, Expr_::VarLocal(_)) && i.resolve(*field) == Some("x")),
            "`p.x` resolves to an Access over a local, got {body:?}"
        );
    }

    #[test]
    fn record_update_canonicalises_base_and_fields() {
        // `{ p | x = 41 }` resolves the base `p` (the parameter, a local) and the
        // updated field value; the field name is a label carried unresolved.
        let mut i = Interner::new();
        let body = canon_body(
            &mut i,
            "module Main exposing (v)\nv p =\n    { p | x = 41 }\n",
            "v",
        );
        assert!(body.is_some(), "v must canonicalise");
        let Some(body) = body else { return };
        assert!(
            matches!(&body, Expr_::Update(base, fields)
                if matches!(base.value, Expr_::VarLocal(_))
                    && fields.len() == 1
                    && matches!(fields.first().map(|(_, e)| &e.value), Some(Expr_::Int(41)))),
            "`{{ p | x = 41 }}` resolves to an Update over a local, got {body:?}"
        );
    }

    #[test]
    fn duplicate_record_update_field_is_rejected() {
        // `{ p | x = 1, x = 2 }` updates `x` twice — rejected (SKY-N0010), as on
        // a record literal.
        let mut i = Interner::new();
        let src = sky_parse::parse_module(
            "module Main exposing (v)\nv p =\n    { p | x = 1, x = 2 }\n",
            &mut i,
        );
        assert!(src.is_ok(), "must parse");
        let Ok(src) = src else { return };
        let r = canonicalise(&src, &mut i);
        assert!(
            matches!(
                r,
                Err(sky_diagnostics::Diagnostic::Name {
                    msg: sky_diagnostics::NameError::DuplicateValue { .. },
                    ..
                })
            ),
            "duplicate update field must be a DuplicateValue, got {r:?}"
        );
    }

    #[test]
    fn duplicate_record_field_is_rejected() {
        // `{ x = 1, x = 2 }` defines `x` twice — rejected (SKY-N0010) rather than
        // silently collapsing to one field.
        let mut i = Interner::new();
        let src = sky_parse::parse_module(
            "module Main exposing (v)\nv =\n    { x = 1, x = 2 }\n",
            &mut i,
        );
        assert!(src.is_ok(), "must parse");
        let Ok(src) = src else { return };
        let r = canonicalise(&src, &mut i);
        assert!(
            matches!(
                r,
                Err(sky_diagnostics::Diagnostic::Name {
                    msg: sky_diagnostics::NameError::DuplicateValue { .. },
                    ..
                })
            ),
            "duplicate record field must be a DuplicateValue, got {r:?}"
        );
    }

    #[test]
    fn env_var_homes_compare() {
        // Exercise the VarHome surface for PartialEq coverage.
        assert_eq!(VarHome::Local, VarHome::Local);
        let m: Vec<Symbol> = vec![Symbol::from_raw(1)];
        assert_ne!(VarHome::TopLevel(m.clone()), VarHome::Local);
        assert_eq!(VarHome::TopLevel(m.clone()), VarHome::TopLevel(m));
    }

    // ---- type aliases (B2) ------------------------------------------------

    /// Parse `source` and canonicalise it, returning the module on success.
    fn canon_ok(i: &mut Interner, source: &str) -> Option<ast::Module> {
        let src = sky_parse::parse_module(source, i).ok()?;
        canonicalise(&src, i).ok()
    }

    /// The annotation type of a named typed def, cloned for inspection.
    fn typed_ann(m: &ast::Module, i: &Interner, name: &str) -> Option<ast::Type> {
        match find_def(m, i, name)? {
            Def::Typed { ty, .. } => Some(ty.clone()),
            Def::Untyped { .. } => None,
        }
    }

    #[test]
    fn non_parametric_alias_expands_to_its_body() {
        // `type alias Count = Int` then `inc : Count -> Count` must canonicalise
        // exactly as if written `inc : Int -> Int` — the alias is gone.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (inc)\n\
             type alias Count = Int\n\n\
             inc : Count -> Count\n\
             inc n =\n    n\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        let ty = typed_ann(&m, &i, "inc");
        let Some(ast::Type::Lambda(arg, rest)) = ty else {
            assert!(false_marker(), "inc annotation is an arrow");
            return;
        };
        // Both sides are `Int` (a built-in con, empty home) — no `Count` survives.
        for side in [arg.as_ref(), rest.as_ref()] {
            let ast::Type::Con { name, home, args } = side else {
                assert!(false_marker(), "alias expanded to a constructor type");
                return;
            };
            assert_eq!(i.resolve(*name), Some("Int"));
            assert!(home.is_empty(), "Int is a built-in: empty home");
            assert!(args.is_empty());
        }
    }

    #[test]
    fn chained_alias_expands_through() {
        // `B = A`, `A = Int`: a reference to `B` expands through `A` to `Int`.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (v)\n\
             type alias A = Int\n\
             type alias B = A\n\n\
             v : B\n\
             v =\n    0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        let ty = typed_ann(&m, &i, "v");
        let Some(ast::Type::Con { name, home, .. }) = ty else {
            assert!(false_marker(), "v annotation is a constructor type");
            return;
        };
        assert_eq!(i.resolve(name), Some("Int"));
        assert!(home.is_empty());
    }

    #[test]
    fn alias_to_local_union_preserves_home() {
        // An alias whose body names a local union keeps that union's home, so the
        // expansion is identical to naming the union directly.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (v)\n\
             type Color = Red | Green\n\
             type alias C = Color\n\n\
             v : C -> Int\n\
             v c =\n    0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        let ty = typed_ann(&m, &i, "v");
        let Some(ast::Type::Lambda(arg, _)) = ty else {
            assert!(false_marker(), "v annotation is an arrow");
            return;
        };
        let ast::Type::Con { name, home, .. } = arg.as_ref() else {
            assert!(false_marker(), "arg is a constructor type");
            return;
        };
        assert_eq!(i.resolve(*name), Some("Color"));
        assert_eq!(home.first().and_then(|&s| i.resolve(s)), Some("Main"));
    }

    #[test]
    fn parametric_alias_substitutes_and_expands() {
        // `type alias Pair a = (a, a)` applied as `Pair Int` must expand, with
        // the parameter `a` substituted by `Int`, to the tuple `(Int, Int)` —
        // exactly as if the annotation read `(Int, Int) -> Int`. No `Pair` and no
        // free `a` survive.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (addPair)\n\
             type alias Pair a = (a, a)\n\n\
             addPair : Pair Int -> Int\n\
             addPair p =\n    0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        // The binding generalises over nothing — `a` was bound to `Int`.
        let Some(Def::Typed { free_vars, .. }) = find_def(&m, &i, "addPair") else {
            assert!(false_marker(), "addPair is a typed def");
            return;
        };
        assert!(free_vars.is_empty(), "no free type variable survives");
        let Some(ast::Type::Lambda(arg, _)) = typed_ann(&m, &i, "addPair") else {
            assert!(false_marker(), "addPair annotation is an arrow");
            return;
        };
        let ast::Type::Tuple(elems) = arg.as_ref() else {
            assert!(false_marker(), "argument expanded to a tuple");
            return;
        };
        assert_eq!(elems.len(), 2, "Pair expands to a 2-tuple");
        for e in elems {
            let ast::Type::Con { name, home, args } = e else {
                assert!(false_marker(), "each tuple member is `Int`");
                return;
            };
            assert_eq!(i.resolve(*name), Some("Int"));
            assert!(
                home.is_empty() && args.is_empty(),
                "Int is a nullary builtin"
            );
        }
    }

    #[test]
    fn parametric_alias_keeps_a_free_argument_variable() {
        // `Pair a` applied to a *variable* argument (`Pair b`) leaves `b` free, so
        // the binding generalises over it: `f : Pair b -> b` is `(b, b) -> b`.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (f)\n\
             type alias Pair a = (a, a)\n\n\
             f : Pair b -> b\n\
             f p =\n    p\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        let Some(Def::Typed { free_vars, .. }) = find_def(&m, &i, "f") else {
            assert!(false_marker(), "f is a typed def");
            return;
        };
        let names: Vec<_> = free_vars.iter().filter_map(|s| i.resolve(*s)).collect();
        assert_eq!(names, vec!["b"], "the argument variable `b` stays free");
    }

    #[test]
    fn alias_applied_with_too_many_arguments_is_an_arity_error() {
        // `Pair` declares one parameter; `Pair Int Bool` supplies two — a coded
        // SKY-N0013 arity error with a span, never a crash.
        let err = canon_err(
            "module Main exposing (v)\n\
             type alias Pair a = (a, a)\n\n\
             v : Pair Int Bool\n\
             v =\n    0\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::AliasArity {
                        expected: 1,
                        found: 2,
                        ..
                    },
                    ..
                })
            ),
            "expected an AliasArity Name diagnostic (1 expected, 2 found), got {err:?}"
        );
    }

    #[test]
    fn parametric_alias_under_applied_is_an_arity_error() {
        // A bare `Pair` supplies zero arguments to a one-parameter alias — a type
        // alias must be fully applied, so this is an arity error, not an opaque
        // constructor.
        let err = canon_err(
            "module Main exposing (v)\n\
             type alias Pair a = (a, a)\n\n\
             v : Pair\n\
             v =\n    0\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::AliasArity {
                        expected: 1,
                        found: 0,
                        ..
                    },
                    ..
                })
            ),
            "expected an AliasArity Name diagnostic (1 expected, 0 found), got {err:?}"
        );
    }

    #[test]
    fn duplicate_alias_name_is_a_duplicate_type() {
        let err = canon_err(
            "module Main exposing (v)\n\
             type alias X = Int\n\
             type alias X = Bool\n\n\
             v : Int\n\
             v =\n    0\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::DuplicateType { .. },
                    ..
                })
            ),
            "expected DuplicateType, got {err:?}"
        );
    }

    #[test]
    fn alias_colliding_with_a_union_is_a_duplicate_type() {
        let err = canon_err(
            "module Main exposing (v)\n\
             type Color = Red\n\
             type alias Color = Int\n\n\
             v : Int\n\
             v =\n    0\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::DuplicateType { .. },
                    ..
                })
            ),
            "expected DuplicateType, got {err:?}"
        );
    }

    /// **Phase-A tripwire: registry → canon parity.**
    ///
    /// For every [`sky_kernels::StdlibKernel`] variant in `ALL`, if the
    /// variant's declared qualifier IS present in `Env.qual_vars`, then the
    /// variant's declared name must ALSO be present in that qualifier's member
    /// map.  A failure here means `QUALIFIERS` in `env.rs` diverged from
    /// `StdlibKernel::ALL + decl()` — the anti-drift invariant is broken.
    ///
    /// The check is intentionally one-directional (registry → canon): names
    /// present in `QUALIFIERS` but absent from the registry (e.g. `Basics.*`
    /// helper aliases) are NOT an error.  Qualifiers absent from `qual_vars`
    /// entirely (e.g. `"Log"`, `"PubSub"`) are skipped automatically.
    #[test]
    fn canon_equals_registry() {
        use sky_intern::Interner;
        use sky_kernels::StdlibKernel;

        let mut interner = Interner::new();
        let env = Env::initial(vec![], &mut interner)
            .expect("Env::initial must not fail in the tripwire test");

        for sk in StdlibKernel::ALL {
            let decl = sk.decl();

            // Skip internal-only qualifiers (e.g. "_internal_").
            if decl.qualifier.starts_with('_') {
                continue;
            }

            // Intern qualifier + name.  If they were already interned by
            // install_prelude_qualifiers we get the same symbol; if not, the
            // fresh symbol will simply not appear in qual_vars (correct skip).
            // `Interner::intern` is infallible in practice (OOM only).
            let qual_sym = interner
                .intern(decl.qualifier)
                .expect("tripwire: intern qualifier OOM");
            let name_sym = interner
                .intern(decl.name)
                .expect("tripwire: intern name OOM");

            // If the qualifier is not in qual_vars at all (e.g. "Log" is only
            // in `vars`, not `qual_vars`; "PubSub" is not yet wired), skip.
            let Some(members) = env.qual_vars.get(&qual_sym) else {
                continue;
            };

            // The qualifier IS registered — so the name must also be present.
            assert!(
                members.contains_key(&name_sym),
                "StdlibKernel::{sk:?} declares ({:?}, {:?}) but {:?} is missing \
                 from env.qual_vars[{:?}]; update QUALIFIERS in env.rs to match \
                 StdlibKernel::decl()",
                decl.qualifier,
                decl.name,
                decl.name,
                decl.qualifier,
            );

            // Also verify the stdlib_index was populated for this entry.
            assert!(
                env.stdlib_index.contains_key(&(qual_sym, name_sym)),
                "StdlibKernel::{sk:?} is in qual_vars but missing from stdlib_index; \
                 the Phase-A registry-population loop in install_prelude_qualifiers \
                 must have skipped it",
            );
        }
    }
}
