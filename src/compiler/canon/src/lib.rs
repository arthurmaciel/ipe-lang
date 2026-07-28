#![forbid(unsafe_code)]
//! `ipe_canon` — name resolution / canonicalisation for the Milestone-0 subset
//! of Ipê.
//!
//! Entry point: [`canonicalise`]. It consumes a [`ipe_syntax::Module`] (the raw
//! parse tree) plus a mutable [`Interner`] and produces a name-resolved
//! [`ast::Module`], or a typed [`ipe_diagnostics::Diagnostic`]. Every variable
//! reference is classified — local binding, top-level binding, stdlib kernel,
//! or data constructor — by porting the supported subset of the Haskell compiler's
//! `Ipe.Canonicalise.{Module,Expression,Pattern,Type,Environment}`.

pub mod ast;
pub mod builtins;
mod env;
pub mod link;
pub mod module_classify;
mod resolve;
pub mod target_gate;

use std::collections::{BTreeMap, BTreeSet};

use ipe_diagnostics::DResult;
use ipe_intern::{Interner, Symbol};

pub use env::{CtorHome, Env, STDLIB_MODULE_QUALIFIERS, VarHome};
pub use resolve::{ModuleOrigin, is_reserved_builtin_type_name};

/// A type alias exported by a module in its raw (unresolved) source form.
///
/// Carried in [`ModuleExports`] so importing modules can inject it into their
/// own alias table and expand it there. Fields mirror the private `AliasDef`
/// in `resolve.rs`; the public counterpart lets the multi-module driver pass
/// exports across the boundary without exposing resolver internals.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExportedAlias {
    /// Declared type-parameter names, in source order.
    pub params: Vec<Symbol>,
    /// The right-hand-side of the `type alias` declaration, kept unresolved.
    pub body: ipe_syntax::TypeAnnotation,
}

/// The public exports of a canonicalised module: the names and resolved
/// locations of every value, type, constructor, and alias the module exposes
/// via its `exposing` list.
///
/// Used by [`canonicalise_module`] as the `deps` map entries so importing
/// modules can inject the right resolved names into their environments.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
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
    /// The complete set of type names in scope after this module was
    /// canonicalised, mapped to their home module path.  Includes all types
    /// imported from dep modules PLUS the module's own union ADTs.
    ///
    /// Stored here so importing modules can use it when expanding alias bodies
    /// that reference types from this module's own dep scope — without having
    /// to re-import those deps themselves.  See [`AliasDef::dep_scope_types`]
    /// in `ipe_canon::resolve`.
    pub scope_types: BTreeMap<Symbol, Vec<Symbol>>,
    /// The complete set of type *aliases* in scope after this module was
    /// canonicalised — own local aliases PLUS aliases injected from dep modules.
    ///
    /// Parallel to `scope_types` but for aliases.  Importing modules that
    /// expand an alias body whose fields reference ALIAS types from THIS
    /// module's own dep scope need access to those alias definitions — otherwise
    /// e.g. `Piece` (a record-alias from `Chess.Piece`) would be invisible when
    /// an importer of `State` expands `Model`'s body.
    pub scope_aliases: BTreeMap<Symbol, ExportedAlias>,
    /// Exported Stage-4 kernel aliases: value names whose binding is
    /// `f = Ffi.kernel "Module_function"`, mapped to the resolved kernel target
    /// `(StdlibKernel, module, function)`.
    ///
    /// A name here is ALSO present in `values` (it is an exported value), but an
    /// importer must register it as a [`VarHome::Kernel`] — routing every
    /// `Alias.f` reference straight to the kernel — rather than the default
    /// `TopLevel(path)`, because the alias emits no top-level body. The
    /// disjointness with a normal value is by construction: `detect_kernel_alias`
    /// classifies each binding exactly once.
    pub kernel_aliases: BTreeMap<Symbol, ExportedKernelAlias>,
}

/// The resolved target of an exported Stage-4 kernel alias — the `(StdlibKernel,
/// module, function)` an `Ffi.kernel "Module_function"` binding routes to. See
/// [`ModuleExports::kernel_aliases`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExportedKernelAlias {
    /// The registered kernel this alias routes to.
    pub id: ipe_kernels::StdlibKernel,
    /// Canonical kernel-module symbol (the `Module` half of the split string).
    pub module: Symbol,
    /// Canonical kernel-function symbol (the `function` half of the split).
    pub function: Symbol,
}

/// Canonicalise a parsed module into its name-resolved canonical AST.
///
/// # Errors
/// Returns a [`ipe_diagnostics::Diagnostic`] for any name that resolves to
/// neither a constructor, a bound variable, a top-level binding, nor a kernel
/// function (an [`ipe_diagnostics::NameError`] payload variant carrying a
/// deterministic did-you-mean), or for a duplicated value/constructor/type
/// name.
pub fn canonicalise(m: &ipe_syntax::Module, interner: &mut Interner) -> DResult<ast::Module> {
    resolve::canonicalise(m, interner)
}

/// Canonicalise a module in a multi-module project context.
///
/// Unlike [`canonicalise`], this function:
/// * validates `m`'s declared module name against `expected_path` — emits
///   [`ipe_diagnostics::NameError::ModulePathMismatch`] when they disagree
/// * rejects `Ipê` / `Std` as the first path segment — emits
///   [`ipe_diagnostics::NameError::ReservedNamespace`]
/// * resolves each local `import` against `deps`, injecting exports into the
///   name-resolution environment — emits
///   [`ipe_diagnostics::NameError::ModuleNotFound`] /
///   [`ipe_diagnostics::NameError::NameNotExposed`] /
///   [`ipe_diagnostics::NameError::AmbiguousImport`] on violations
/// * returns the resolved [`ast::Module`] plus a [`ModuleExports`] summary
///   derived from the module's own `exposing` list
///
/// # Errors
/// Any of the above [`ipe_diagnostics::NameError`] variants, or any error that
/// [`canonicalise`] can return.
pub fn canonicalise_module(
    m: &ipe_syntax::Module,
    expected_path: &[Symbol],
    deps: &BTreeMap<Vec<Symbol>, ModuleExports>,
    interner: &mut Interner,
) -> DResult<(ast::Module, ModuleExports)> {
    resolve::canonicalise_module(m, expected_path, deps, interner)
}

/// Canonicalise a module carrying an explicit trust [`ModuleOrigin`].
///
/// Like [`canonicalise_module`] but lets the build driver vouch that a module's
/// source came from the compiler's own embedded stdlib table
/// ([`ModuleOrigin::EmbeddedStdlib`]) — the ONLY way to legitimately declare a
/// `module Ipe.…` / `module Ipe.…` home without tripping IPE-N0025. The trust tag
/// is unforgeable from module text: a user file named `Ipe.Foo` reaches this
/// function as [`ModuleOrigin::User`] and stays rejected.
///
/// # Errors
/// Any error [`canonicalise_module`] can return, plus a fail-closed
/// [`ipe_diagnostics::Diagnostic::CompilerBug`] when an `EmbeddedStdlib` module
/// carries an un-annotated top-level binding.
pub fn canonicalise_module_with_origin(
    m: &ipe_syntax::Module,
    expected_path: &[Symbol],
    deps: &BTreeMap<Vec<Symbol>, ModuleExports>,
    origin: ModuleOrigin,
    interner: &mut Interner,
) -> DResult<(ast::Module, ModuleExports)> {
    resolve::canonicalise_module_with_origin(m, expected_path, deps, origin, interner)
}

/// Canonicalise a module for the incremental (salsa) build driver.
///
/// Like [`canonicalise_module_with_origin`] but takes the dep interfaces by
/// reference (the `module_interface` query memos — no per-importer deep clone)
/// and an explicit `known_modules` universe (dot-joined module paths) used
/// ONLY for the IPE-N0020 did-you-mean list. `deps` must contain exactly this
/// module's resolved imports; `known_modules` should list every module in the
/// project. Strings only on the suggestion path — it never interns.
///
/// # Errors
/// Same set as [`canonicalise_module_with_origin`].
pub fn canonicalise_module_in_project(
    m: &ipe_syntax::Module,
    expected_path: &[Symbol],
    deps: &BTreeMap<Vec<Symbol>, &ModuleExports>,
    known_modules: &BTreeSet<Box<str>>,
    origin: ModuleOrigin,
    interner: &mut Interner,
) -> DResult<(ast::Module, ModuleExports)> {
    resolve::canonicalise_module_in_project(m, expected_path, deps, known_modules, origin, interner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast::{Def, Expr, Expr_, Pattern_};
    use ipe_diagnostics::{Diagnostic, NameError};
    use ipe_intern::Symbol;

    const GOLDEN: &str = include_str!("../../../../tests/golden/basics/Main.ipe");

    /// Parse + canonicalise the golden M0 module. Returns `None` (failing the
    /// caller's assertions) rather than panicking, per the no-panic gate.
    fn canon_golden(i: &mut Interner) -> Option<ast::Module> {
        let src = ipe_parse::parse_module(GOLDEN, i).ok()?;
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
        let parsed = ipe_parse::parse_module(src, &mut i).ok()?;
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

        // main = Io.println (String.fromInt (update Increment 0))
        let outer = as_call(body);
        assert!(
            matches!(outer, Some((Expr_::VarKernel { .. }, _))),
            "main body is a call to a kernel"
        );
        let Some((
            Expr_::VarKernel {
                id: _,
                module,
                name,
            },
            outer_args,
        )) = outer
        else {
            return;
        };
        assert_eq!(i.resolve(*module), Some("Io"));
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
        let Some((
            Expr_::VarKernel {
                id: _,
                module,
                name,
            },
            mid_args,
        )) = mid
        else {
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
        let src = ipe_parse::parse_module(src_text, &mut i).ok()?;
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
        // `printn` is one edit from the `Ipe.Io` member `println`.
        let err = canon_err("module Main exposing (main)\n\nmain = Io.printn\n");
        assert!(
            matches!(
                &err,
                Some(Diagnostic::Name {
                    msg: NameError::NoSuchMember { .. },
                    ..
                })
            ),
            "expected NoSuchMember, got {err:?}"
        );
        let Some(Diagnostic::Name {
            msg:
                NameError::NoSuchMember {
                    member,
                    suggestions,
                    ..
                },
            ..
        }) = err
        else {
            return;
        };
        assert_eq!(&*member, "printn");
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

    /// ADR 0047 Tier A (`Ipe.Basics`) and Tier B (core type vocabulary) are
    /// ambient: a module reaches for `identity` / `always` / `not`, the type
    /// names `Maybe` / `Result` / `List`, and the constructors `Just` /
    /// `Nothing` / `Ok` / `Err` / `True` / `False` with NO import line. This is
    /// what makes the retired `import Ipe.Prelude exposing (..)` value-flood
    /// redundant.
    #[test]
    fn tier_a_and_b_resolve_ambiently_without_import() {
        let src = "module Main exposing (main)\n\
                   \n\
                   wrap : Int -> Maybe (Result String (List Int))\n\
                   wrap n =\n\
                   \x20   if not (always False n) then\n\
                   \x20       Just (Ok [ identity n ])\n\
                   \x20   else\n\
                   \x20       Nothing\n\
                   \n\
                   main =\n\
                   \x20   case wrap 1 of\n\
                   \x20       Just _ -> LT\n\
                   \x20       Nothing -> GT\n";
        let opt = canon_src(src);
        assert!(
            opt.is_some(),
            "Tier-A/B names must canonicalise with no import line"
        );
    }

    /// ADR 0047 Tier B allows a local definition to shadow a core-vocabulary
    /// name without a diagnostic — a user `map` binds locally.
    #[test]
    fn tier_b_name_may_be_shadowed_locally() {
        let src = "module Main exposing (map)\n\
                   \n\
                   map : Int -> Int\n\
                   map n = n\n";
        let opt = canon_src(src);
        assert!(
            opt.is_some(),
            "a local `map` must shadow the ambient vocabulary without a diagnostic"
        );
    }

    /// ADR 0047 Tier C: a qualified reference to a module the compiler does not
    /// place in ambient scope fails to resolve (IPE-N0004) at its use site,
    /// never a silent success — the import list stays a complete inventory of a
    /// file's capabilities.
    #[test]
    fn tier_c_unimported_qualifier_is_unknown_module() {
        let err = canon_err("module Main exposing (main)\n\nmain = Widgets.render 0\n");
        let Some(Diagnostic::Name {
            msg: NameError::UnknownModule { qualifier, .. },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected UnknownModule (IPE-N0004)");
            return;
        };
        assert_eq!(&*qualifier, "Widgets");
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
    fn user_type_shadowing_builtin_rejected() {
        // `Length` is a reserved built-in (`Ipe.Ui` nullary type) that the
        // lowerer matches ahead of the user-enum lookup; a user `type Length`
        // would be silently overridden, so canon must reject it (IPE-N0026).
        let src = "module Main exposing (main)\n\n\
                   type Length = Red | Green\n\nmain = 0\n";
        let err = canon_err(src);
        let Some(Diagnostic::Name {
            msg: NameError::ReservedBuiltinType { name },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected ReservedBuiltinType, got {err:?}");
            return;
        };
        assert_eq!(&*name, "Length");
    }

    #[test]
    fn non_reserved_user_type_still_compiles() {
        // A same-shaped ADT under a NON-reserved name must canonicalise cleanly —
        // the gate is scoped to reserved built-in names only.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\n\
             type Swatch = Red | Green\n\nmain = 0\n",
        );
        assert!(m.is_some(), "non-reserved `type Swatch` must canonicalise");
    }

    #[test]
    fn type_alias_shadowing_builtin_rejected() {
        // Aliases are gated identically — `type alias Html = String` shadows the
        // built-in `Ipe.Html.Html`, which the lowerer maps to `IrType::Ui`.
        let src = "module Main exposing (main)\n\n\
                   type alias Html = String\n\nmain = 0\n";
        let err = canon_err(src);
        let Some(Diagnostic::Name {
            msg: NameError::ReservedBuiltinType { name },
            ..
        }) = err
        else {
            assert!(
                false_marker(),
                "expected ReservedBuiltinType for the alias, got {err:?}"
            );
            return;
        };
        assert_eq!(&*name, "Html");
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
        let parsed = ipe_parse::parse_module(src, &mut i);
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

    // ─────────────────────────────────────────────────────────────────────────
    // Stdlib import-alias registration.
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse + canonicalise a single module through the MULTI-module entry
    /// (`canonicalise_module`) with no deps — the path the build driver uses for
    /// a project with a `ipe.toml`, and the one that processes `import`
    /// declarations. Returns the canonical module + interner.
    fn canon_module_src(src: &str) -> Option<(ast::Module, Interner)> {
        let mut i = Interner::new();
        let parsed = ipe_parse::parse_module(src, &mut i).ok()?;
        let expected: Vec<Symbol> = parsed.name.value.clone();
        let deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        let (m, _exports) = canonicalise_module(&parsed, &expected, &deps, &mut i).ok()?;
        Some((m, i))
    }

    /// Assert the body of `main` is a bare `Qualifier.member` reference resolving
    /// to a kernel with the given canonical module + name.
    fn assert_main_is_kernel(m: &ast::Module, i: &Interner, module: &str, name: &str) {
        let Some(Def::Untyped { body, .. }) = find_def(m, i, "main") else {
            assert!(false_marker(), "main should be an untyped def");
            return;
        };
        let Expr_::VarKernel {
            module: mo,
            name: na,
            ..
        } = &body.value
        else {
            assert!(
                false_marker(),
                "main body should be a kernel reference, got {:?}",
                body.value
            );
            return;
        };
        assert_eq!(i.resolve(*mo), Some(module), "kernel module");
        assert_eq!(i.resolve(*na), Some(name), "kernel name");
    }

    #[test]
    fn stdlib_alias_registers_multisegment_json_encode() {
        // The reported failure: `import Ipe.Json.Encode as Encode` then
        // `Encode.string` used to error IPE-N0004 (unknown module `Encode`)
        // because the alias was never registered against the canonical `JsonEnc`.
        let src = "module Main exposing (main)\n\
                   import Ipe.Json.Encode as Encode\n\n\
                   main = Encode.string\n";
        let Some((m, i)) = canon_module_src(src) else {
            assert!(false_marker(), "aliased stdlib import must canonicalise");
            return;
        };
        assert_main_is_kernel(&m, &i, "JsonEnc", "string");
    }

    #[test]
    fn stdlib_alias_registers_multisegment_json_decode_pipeline() {
        // Deepest path (5 segments) → canonical `JsonDecP`.
        let src = "module Main exposing (main)\n\
                   import Ipe.Json.Decode.Pipeline as P\n\n\
                   main = P.required\n";
        let Some((m, i)) = canon_module_src(src) else {
            assert!(false_marker(), "aliased pipeline import must canonicalise");
            return;
        };
        assert_main_is_kernel(&m, &i, "JsonDecP", "required");
    }

    #[test]
    fn stdlib_alias_registers_std_module() {
        // Completeness: a `Ipe.*` module aliased to a name differing from both
        // the last segment and the canonical qualifier.
        let src = "module Main exposing (main)\n\
                   import Ipe.Ui as U\n\n\
                   main = U.text\n";
        let Some((m, i)) = canon_module_src(src) else {
            assert!(false_marker(), "aliased Ipe.Ui import must canonicalise");
            return;
        };
        assert_main_is_kernel(&m, &i, "Ui", "text");
    }

    #[test]
    fn stdlib_import_no_as_uses_last_segment() {
        // No `as`: Elm exposes the module under its LAST path segment. Here the
        // last segment (`Encode`) differs from the canonical qualifier
        // (`JsonEnc`), so the fix must register `Encode`, not only `JsonEnc`.
        let src = "module Main exposing (main)\n\
                   import Ipe.Json.Encode\n\n\
                   main = Encode.string\n";
        let Some((m, i)) = canon_module_src(src) else {
            assert!(false_marker(), "no-as stdlib import must canonicalise");
            return;
        };
        assert_main_is_kernel(&m, &i, "JsonEnc", "string");
    }

    #[test]
    fn stdlib_alias_works_on_single_module_path() {
        // The single-module `canonicalise` entry also registers stdlib aliases
        // (it previously ignored imports entirely).
        let src = "module Main exposing (main)\n\
                   import Ipe.Json.Encode as Encode\n\n\
                   main = Encode.int\n";
        let mut i = Interner::new();
        let Ok(parsed) = ipe_parse::parse_module(src, &mut i) else {
            assert!(false_marker(), "parse");
            return;
        };
        let Ok(m) = canonicalise(&parsed, &mut i) else {
            assert!(false_marker(), "single-module canonicalise must succeed");
            return;
        };
        assert_main_is_kernel(&m, &i, "JsonEnc", "int");
    }

    #[test]
    fn unknown_stdlib_alias_stays_fail_closed() {
        // A `Ipê.*` path with no registered canonical qualifier must NOT invent
        // one: the alias reference surfaces UnknownModule at its use site.
        let src = "module Main exposing (main)\n\
                   import Ipe.Nonexistent as N\n\n\
                   main = N.foo\n";
        let mut i = Interner::new();
        let Ok(parsed) = ipe_parse::parse_module(src, &mut i) else {
            assert!(false_marker(), "parse");
            return;
        };
        let deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        let expected = parsed.name.value.clone();
        let err = canonicalise_module(&parsed, &expected, &deps, &mut i).err();
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::UnknownModule { .. },
                    ..
                })
            ),
            "unknown stdlib alias must fail closed with UnknownModule, got {err:?}"
        );
    }

    #[test]
    fn stdlib_module_paths_target_a_known_qualifier() {
        // Anti-drift (no dangling target): every canonical named in the path
        // table is a real registered qualifier, and every path is `Ipê.*`/`Ipe.*`.
        let mut i = Interner::new();
        let home = vec![i.intern("Main").expect("intern Main")];
        let env = Env::initial(home, &mut i).expect("build env");
        for (path, canonical) in crate::env::STDLIB_MODULE_QUALIFIERS {
            assert!(
                matches!(path.first(), Some(&"Ipe")),
                "path {path:?} must start with Ipe or Std"
            );
            let sym = i.intern(canonical).expect("intern canonical");
            assert!(
                env.qual_members(sym).is_some(),
                "canonical `{canonical}` for path {path:?} is not a registered qualifier"
            );
        }
    }

    #[test]
    fn every_canonical_qualifier_has_an_import_path() {
        // Anti-drift (total coverage): every PRIMARY qualifier the registry
        // defines is reachable via at least one import path, so a new kernel
        // module cannot ship without an `import … as Alias` route.
        //
        // Primary qualifiers are the bare short-names (no `.`) plus the sole
        // dotted canonical `Db.Decode`; the other dotted `qual_vars` keys are the
        // inline-qualifier convenience aliases (`Ipe.Html`, …), not import targets.
        let mut i = Interner::new();
        let home = vec![i.intern("Main").expect("intern Main")];
        let env = Env::initial(home, &mut i).expect("build env");
        let targets: BTreeSet<&str> = crate::env::STDLIB_MODULE_QUALIFIERS
            .iter()
            .map(|(_, c)| *c)
            .collect();
        for &key in env.qual_vars.keys() {
            let Some(name) = i.resolve(key) else { continue };
            let is_primary = !name.contains('.') || name == "Db.Decode";
            if is_primary {
                assert!(
                    targets.contains(name),
                    "canonical qualifier `{name}` has no STDLIB_MODULE_QUALIFIERS \
                     import path — add one so `import …Path… as Alias` can register it"
                );
            }
        }
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
        let src = ipe_parse::parse_module(source, i).ok()?;
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
        // `{ p | x = 1, x = 2 }` updates `x` twice — rejected (IPE-N0010), as on
        // a record literal.
        let mut i = Interner::new();
        let src = ipe_parse::parse_module(
            "module Main exposing (v)\nv p =\n    { p | x = 1, x = 2 }\n",
            &mut i,
        );
        assert!(src.is_ok(), "must parse");
        let Ok(src) = src else { return };
        let r = canonicalise(&src, &mut i);
        assert!(
            matches!(
                r,
                Err(ipe_diagnostics::Diagnostic::Name {
                    msg: ipe_diagnostics::NameError::DuplicateValue { .. },
                    ..
                })
            ),
            "duplicate update field must be a DuplicateValue, got {r:?}"
        );
    }

    #[test]
    fn duplicate_record_field_is_rejected() {
        // `{ x = 1, x = 2 }` defines `x` twice — rejected (IPE-N0010) rather than
        // silently collapsing to one field.
        let mut i = Interner::new();
        let src = ipe_parse::parse_module(
            "module Main exposing (v)\nv =\n    { x = 1, x = 2 }\n",
            &mut i,
        );
        assert!(src.is_ok(), "must parse");
        let Ok(src) = src else { return };
        let r = canonicalise(&src, &mut i);
        assert!(
            matches!(
                r,
                Err(ipe_diagnostics::Diagnostic::Name {
                    msg: ipe_diagnostics::NameError::DuplicateValue { .. },
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
        let src = ipe_parse::parse_module(source, i).ok()?;
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
        // IPE-N0013 arity error with a span, never a crash.
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
    fn unparenthesised_nested_container_is_a_builtin_arity_error() {
        // `Maybe List Int` parses as `Maybe` over TWO args (`List`, `Int`) with
        // `List` itself nullary — the exact shape that ICE'd the lowerer
        // (IPE-I0001, empty-home `List`). Arguments canonicalise depth-first, so
        // the bare `List` is caught first: a clean IPE-N0031 pointing straight
        // at the constructor missing its element type. (Were `List` well-formed,
        // the over-applied `Maybe` would then be rejected — either way the ICE
        // is unreachable.)
        let err = canon_err(
            "module Main exposing (v)\n\
             v : Maybe List Int\n\
             v =\n    Nothing\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::BuiltinTypeArity {
                        ref name,
                        expected: 1,
                        found: 0,
                    },
                    ..
                }) if name.as_ref() == "List"
            ),
            "expected a BuiltinTypeArity(List, 1, 0) diagnostic, got {err:?}"
        );
    }

    #[test]
    fn over_applied_maybe_is_a_builtin_arity_error() {
        // Well-formed inner, over-applied outer: `Maybe (List Int) Bool` gives
        // `Maybe` two arguments. The inner `(List Int)` passes, so the outer
        // over-application is the caught error.
        let err = canon_err(
            "module Main exposing (v)\n\
             v : Maybe (List Int) Bool\n\
             v =\n    Nothing\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::BuiltinTypeArity {
                        ref name,
                        expected: 1,
                        found: 2,
                    },
                    ..
                }) if name.as_ref() == "Maybe"
            ),
            "expected a BuiltinTypeArity(Maybe, 1, 2) diagnostic, got {err:?}"
        );
    }

    #[test]
    fn under_applied_dict_is_a_builtin_arity_error() {
        // `Dict` takes two arguments; `Dict String` supplies one.
        let err = canon_err(
            "module Main exposing (v)\n\
             v : Dict String\n\
             v =\n    Nothing\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::BuiltinTypeArity {
                        ref name,
                        expected: 2,
                        found: 1,
                    },
                    ..
                }) if name.as_ref() == "Dict"
            ),
            "expected a BuiltinTypeArity(Dict, 2, 1) diagnostic, got {err:?}"
        );
    }

    #[test]
    fn parenthesised_nested_container_stays_well_formed() {
        // The fix must not reject the correct spelling — `Maybe (List Int)` is
        // `Maybe` over exactly one argument.
        let err = canon_err(
            "module Main exposing (v)\n\
             v : Maybe (List Int)\n\
             v =\n    Nothing\n",
        );
        assert!(
            !matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::BuiltinTypeArity { .. },
                    ..
                })
            ),
            "well-formed `Maybe (List Int)` must not trip IPE-N0031, got {err:?}"
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

    // ---------------------------------------------------------------------
    // A LOCAL `type X` / `type alias X` shadowing a dep-imported `X`
    // must be rejected at the declaration with IPE-N0012 (`DuplicateType`),
    // not a downstream IPE-T0001. See `canonicalise_with_env`'s dep-shadow
    // pre-pass and docs/adr/0010-pattern-and-lowering-completeness.md
    // (item D).
    // ---------------------------------------------------------------------

    /// Canonicalise a `Dep` source (no deps of its own), then canonicalise a
    /// `Main` source with `Dep`'s exports available for import. Returns the
    /// diagnostic (if any) from canonicalising `Main`. Returns `None` from the
    /// parse/Dep-canon steps rather than panicking, per the no-panic gate.
    fn canon_main_with_dep(dep_src: &str, main_src: &str) -> Option<Diagnostic> {
        let mut i = Interner::new();
        let dep_parsed = ipe_parse::parse_module(dep_src, &mut i).ok()?;
        let dep_expected = dep_parsed.name.value.clone();
        let empty: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        let (_dep_m, dep_exports) =
            canonicalise_module(&dep_parsed, &dep_expected, &empty, &mut i).ok()?;

        let mut deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        deps.insert(dep_exports.path.clone(), dep_exports);

        let main_parsed = ipe_parse::parse_module(main_src, &mut i).ok()?;
        let main_expected = main_parsed.name.value.clone();
        canonicalise_module(&main_parsed, &main_expected, &deps, &mut i).err()
    }

    #[test]
    fn local_type_shadowing_dep_imported_type_is_duplicate_type() {
        let err = canon_main_with_dep(
            "module Dep exposing (Color(..))\n\
             type Color = Red | Green | Blue\n",
            "module Main exposing (main)\n\
             import Dep exposing (Color(..))\n\
             import Ipe.Io as Io\n\n\
             type Color = Warm | Cool\n\n\
             describe : Color -> String\n\
             describe c =\n    case c of\n        Warm -> \"warm\"\n        Cool -> \"cool\"\n\n\
             main =\n    Io.println (describe Warm)\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::DuplicateType { .. },
                    ..
                })
            ),
            "local `type Color` shadowing imported Dep.Color must be a \
             DuplicateType (IPE-N0012) at the declaration, got {err:?}"
        );
    }

    #[test]
    fn local_type_alias_shadowing_dep_imported_type_is_duplicate_type() {
        // Same shape, but the LOCAL declaration is a `type alias`, proving the
        // alias-side gap is ALSO closed.
        let err = canon_main_with_dep(
            "module Dep exposing (Color(..))\n\
             type Color = Red | Green | Blue\n",
            "module Main exposing (main)\n\
             import Dep exposing (Color(..))\n\
             import Ipe.Io as Io\n\n\
             type alias Color = Int\n\n\
             main =\n    Io.println \"hi\"\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::DuplicateType { .. },
                    ..
                })
            ),
            "local `type alias Color` shadowing imported Dep.Color must be a \
             DuplicateType (IPE-N0012) at the declaration, got {err:?}"
        );
    }

    #[test]
    fn two_modules_each_declaring_unrelated_same_named_type_without_import_is_fine() {
        // Non-regression control: `Dep` declares `type Color` but `Main` never
        // imports it, so `type_home_map` in `Main`'s resolution never gains a
        // `Dep.Color` entry — the dep-shadow pre-pass sees `None` and nothing
        // rejects. `Main`'s own unrelated `type Color` compiles cleanly.
        let err = canon_main_with_dep(
            "module Dep exposing (Color(..))\n\
             type Color = Red | Green | Blue\n",
            "module Main exposing (main)\n\
             import Ipe.Io as Io\n\n\
             type Color = Warm | Cool\n\n\
             describe : Color -> String\n\
             describe c =\n    case c of\n        Warm -> \"warm\"\n        Cool -> \"cool\"\n\n\
             main =\n    Io.println (describe Warm)\n",
        );
        assert!(
            err.is_none(),
            "an unrelated same-named local type with NO import of the dep must \
             compile cleanly, got {err:?}"
        );
    }

    #[test]
    fn same_module_duplicate_type_still_uses_first_declared_span() {
        // The same-module duplicate path (the `seen_types`
        // loop) is separate from the dep-shadow pre-pass: two `type Color`
        // declarations in ONE module still report the FIRST-declared span, not
        // the `Span::DUMMY` the dep-shadow path uses.
        let err = canon_err(
            "module Main exposing (main)\n\
             type Color = Warm\n\
             type Color = Cool\n\n\
             main =\n    Io.println \"hi\"\n",
        );
        let Some(Diagnostic::Name {
            msg: NameError::DuplicateType { first, .. },
            ..
        }) = err
        else {
            assert!(false_marker(), "expected DuplicateType, got {err:?}");
            return;
        };
        assert_ne!(
            first,
            ipe_diagnostics::Span::DUMMY,
            "same-module duplicate must carry the first-declared span, not DUMMY"
        );
    }

    /// **Tripwire: registry ↔ canon parity, a full subset gate.**
    ///
    /// Forward direction (registry → canon): for every
    /// [`ipe_kernels::StdlibKernel`] variant in `ALL`, if the variant's
    /// declared qualifier IS present in `Env.qual_vars`, then the variant's
    /// declared name must ALSO be present in that qualifier's member map.  A
    /// failure here means `QUALIFIERS` in `env.rs` diverged from
    /// `StdlibKernel::ALL + decl()` — the anti-drift invariant is broken.
    ///
    /// The forward check is intentionally one-directional: names present in
    /// `QUALIFIERS` but absent from the registry (e.g. `Basics.*` helper
    /// aliases) are NOT an error.  Qualifiers absent from `qual_vars` entirely
    /// (e.g. `"Log"`, `"PubSub"`) are skipped automatically.
    ///
    /// Reverse direction (canon → registry, "G1"): for every non-excluded
    /// qualifier's members, `VarHome::Kernel(Some(sk), ..)` entries are
    /// checked for exact id propagation. As a FULL SUBSET GATE it additionally
    /// asserts no non-excluded member sits on `VarHome::Kernel(None, ..)` — i.e. every
    /// kernel the resolver/canon layer recognises (every `qual_vars` member,
    /// short of the deliberately-excluded alias namespaces and the
    /// structurally-unreachable `KNOWN_UNBACKED` set proven disjoint by
    /// [`known_unbacked_disjoint_from_qual_vars`]) resolves to a concrete
    /// [`ipe_kernels::StdlibKernel`] id.
    ///
    /// **Scope note (this crate has no dependency on `ipe_types`):** this
    /// test proves `QUALIFIERS` (env.rs) stays consistent with
    /// `StdlibKernel::ALL` — it does NOT re-verify the type-scheme table's
    /// own fail-closed behaviour. That guarantee (`ipe`'s exit-0-then-
    /// cargo-fail class `PRINCIPLES.md` calls out: a kernel the resolver
    /// recognises but the type-scheme table does not cover) is a SEPARATE
    /// invariant owned by
    /// `ipe_types::constrain::kernel_scheme_or_unsupported`'s unconditional
    /// `.ok_or(Err(..))` (no flexible-type-variable fallback exists there).
    /// A future regression in that function would sail through this test
    /// untouched — don't treat `canon_equals_registry` as a substitute
    /// regression test for it.
    #[test]
    #[allow(clippy::too_many_lines)] // declarative tripwire — two subset-gate directions + a documented exception table; splitting would obscure the invariant
    fn canon_equals_registry() {
        use crate::env::VarHome;
        use ipe_intern::Interner;
        use ipe_kernels::StdlibKernel;

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

        // ── G1 reverse check: canon → registry ───────────────────────────────
        // For every qual_vars entry that carries id = Some(actual_sk) (i.e.
        // was resolved against stdlib_index at parse time), verify that the
        // stored id EXACTLY MATCHES stdlib_index[(qual, name)].  A second pass
        // additionally proves the reverse direction is a FULL
        // SUBSET GATE: every non-excluded qual_vars member must resolve to a
        // concrete id in the first place — a `VarHome::Kernel(None, ..)` here
        // means QUALIFIERS in env.rs names a member with NO matching
        // StdlibKernel::ALL entry, an anti-drift bug in this crate's own
        // table (not, by itself, the exit-0-then-cargo-fail hole -- that
        // hole is independently closed on the types side by
        // ipe_types::constrain::kernel_scheme_or_unsupported, which this
        // test does not exercise; see the doc comment above).
        //
        // SCOPE — propagation wiring AND totality: verifies that
        // install_prelude_qualifiers stored the id it read from stdlib_index,
        // not some transposed or stale copy, AND that every reachable member
        // has an id at all.  It does NOT verify injectivity
        // of decl() (covered by ipe_kernels::tests::no_colliding_qualifier_name_pairs)
        // and does NOT verify decl-equiv-legacy equivalence (covered by
        // ipe_lower::tests::decl_equiv_legacy_match).
        //
        // Excluded qualifier namespaces (sanctioned aliases — their members
        // share ids with the canonical qualifier but have DIFFERENT qual_syms,
        // so stdlib_index keys differ; the forward check already covers the
        // canonical side):
        //   Basics, Attr, Event                    — non-module prelude names
        //   Ipe.Html, Ipe.Ui, Ipe.Html.Attributes,
        //   Ipe.Html.Events, Ipe.Web, Ipe.Tui, Ipe.WebView  — Ipe.* aliases
        //   Ipe.Html, Ipe.Ui, Ipe.Web, Ipe.Tui    — Ipê.* aliases
        //
        // Note: QUALIFIER_ALIASES clone their members INCLUDING the id, so the
        // alias entries are correct by construction.  The exclusion exists only
        // because stdlib_index is keyed by (canonical_qual, name) so a lookup
        // for (alias_qual, name) would return None — giving a false failure.
        let excluded_quals: std::collections::BTreeSet<&str> = [
            "Basics",
            "Attr",
            "Event",
            "Ipe.Html",
            "Ipe.Ui",
            "Ipe.Html.Attributes",
            "Ipe.Html.Events",
            "Ipe.Web",
            "Ipe.Tui",
            "Ipe.WebView",
            "Ipe.Html",
            "Ipe.Ui",
            "Ipe.Web",
            "Ipe.Tui",
        ]
        .into_iter()
        .collect();

        // Deliberately-unbacked-but-reachable members. `excluded_quals` is
        // qualifier-granular and would blind the whole gate for e.g. String's
        // ~30 real kernels; a single known-holdout member needs a
        // member-granular exception instead.
        // Unlike `excluded_quals` (alias namespaces that resolve elsewhere),
        // these are genuine `id = None` holes: `QUALIFIERS` in env.rs names
        // them (so they parse + canonicalise) but `ipe_kernels::StdlibKernel`
        // has NO variant for them yet (confirmed by grepping
        // `crates/ipe_kernels/src/lib.rs` for each — zero hits). They already
        // fail CLOSED at type-check today with a loud IPE-L0108
        // (`error[IPE-L0108]: kernel function not available yet`) via
        // `constrain_var_kernel`'s existing `id.and_then(..)` — never
        // silently — so listing them here does not violate the
        // exit-0-then-cargo-fail property this gate exists to protect; it makes
        // the fail-closed state explicit and machine-checked instead
        // of an untracked hole the gate would otherwise panic on.
        //
        // Two are deliberate holdouts documented in
        // `crates/ipe/tests/golden_core_stdlib.rs`'s header comment:
        //   - `String.toChar` — no runtime fn; ambiguous Char-vs-Maybe-Char
        //     semantics.
        //   - `Basics.toString` — polymorphic `a -> String` needs a
        //     Display/Stringify bound HM cannot express; already excluded via
        //     the "Basics" qualifier-level entry above, listed here too for
        //     self-documentation (harmless duplicate — `excluded_quals` skips
        //     the whole qualifier before this set is even consulted).
        //
        // The remaining 20 are a CANON-side hole one layer earlier than the
        // `docs/adr/0020-html-ui-live-kernel-arity-tripwire.md` derivation,
        // which only audited kernels that already had a `StdlibKernel` id (a
        // TYPES-side `Ty::Var(u32::MAX)`-class hole); these have no id at all
        // yet. Each is a real Ipe.Ui / Ipe.Html surface named in `QUALIFIERS`
        // (env.rs) with no matching `ipe_kernels::StdlibKernel::decl()` entry —
        // i.e. not-yet-ported advanced UI features (pseudo-classes, media
        // queries, gradients, HTML-document-level nodes), not typos or drift.
        // Implementing 20 kernels with real runtime backing is out of scope
        // for the exhaustiveness gate.
        let deliberately_unbacked_members: std::collections::BTreeSet<(&str, &str)> = [
            ("String", "toChar"),
            ("Basics", "toString"),
            // Ipe.Ui — sized elements / attrs / pseudo-classes / media queries.
            ("Ui", "image"),
            ("Ui", "disabled"),
            ("Ui", "paddingEach"),
            ("Ui", "clipX"),
            ("Ui", "clipY"),
            ("Ui", "scrollbarX"),
            ("Ui", "scrollbarY"),
            ("Ui", "onFile"),
            // ("Ui", "mediaQuery") — REMOVED 2026-07-11: backed by
            // `StdlibKernel::UiMediaQuery` (runtime `ui_media_query_` +
            // `live::style_inject::build_mq` consumer). The last of the 20
            // deferred members from this list to be wired; see
            // docs/adr/0019-ui-mediaquery-safe-boundary.md.
            ("Ui", "onPseudo"),
            ("Ui", "hover"),
            ("Ui", "focus"),
            ("Ui", "focusVisible"),
            ("Ui", "active"),
            // Ipe.Html — document-level nodes + display helper.
            ("Html", "toString"),
            ("Html", "voidNode"),
            ("Html", "doctype"),
            ("Html", "titleNode"),
            ("Html", "htmlNode"),
            ("Html", "headNode"),
            // Ipe.Ui.Background — gradients.
            ("Background", "linearGradient"),
        ]
        .into_iter()
        .collect();

        // G1 forward-propagation check: every
        // `VarHome::Kernel(Some(actual_sk), m, f)` entry's id must match
        // `stdlib_index[(m, f)]` exactly.
        for (qual_sym, members) in env.qual_vars.iter() {
            let qual_str = interner.resolve(*qual_sym).unwrap_or("<unknown>");
            if excluded_quals.contains(qual_str) {
                continue;
            }
            for (name_sym, home) in members {
                if let VarHome::Kernel(Some(actual_sk), m, f) = home {
                    // This entry carries a pre-resolved id — verify it
                    // against stdlib_index using the CANONICAL (module, name)
                    // stored in VarHome, not the qual_vars KEY.
                    //
                    // For plain entries: m == qual_sym, f == name_sym.
                    // For FUNC_ALIASES: name_sym is the ALIAS (e.g.
                    // "htmlRender") while f is the CANONICAL name (e.g.
                    // "render").  stdlib_index is keyed by
                    // (qual_sym, canonical_name), so using (m, f) is always
                    // correct for both.
                    let expected = env.stdlib_index.get(&(*m, *f));
                    let name_str = interner.resolve(*name_sym).unwrap_or("<unknown>");
                    let canon_str = interner.resolve(*f).unwrap_or("<unknown>");
                    assert_eq!(
                        Some(actual_sk),
                        expected,
                        "G1 reverse: VarHome::Kernel in qual_vars[{qual_str:?}][{name_str:?}] \
                         (canonical fn={canon_str:?}) has id={actual_sk:?} but \
                         stdlib_index has {expected:?}; \
                         install_prelude_qualifiers propagation is incorrect",
                    );
                }
            }
        }

        // Subset gate: every non-excluded qual_vars member
        // must resolve to a concrete StdlibKernel id. Pulled into a free
        // function (below) so `qual_vars_subset_gate_catches_unbacked_member`
        // can prove — with a tiny synthetic fixture, no full `Env::initial`
        // needed — that the gate actually fires on a genuine miss, not just
        // that it stays silent on the real (currently clean-of-undocumented-
        // holes) table.
        let unbacked = qual_vars_none_violations(
            &env.qual_vars,
            &interner,
            &excluded_quals,
            &deliberately_unbacked_members,
        );
        assert!(
            unbacked.is_empty(),
            "Task 3 subset gate: every non-excluded qual_vars member must \
             resolve to a concrete StdlibKernel id (a `None` here means \
             QUALIFIERS in env.rs names a member with NO matching \
             StdlibKernel::ALL entry; either add the missing StdlibKernel \
             variant + scheme, or add the qualifier/member to the \
             excluded_quals / deliberately_unbacked_members exclusion sets \
             above with a comment explaining why it is deliberately \
             unbacked, mirroring KNOWN_UNBACKED in constrain.rs). {} \
             violation(s):\n{}",
            unbacked.len(),
            unbacked.join("\n"),
        );
    }

    /// Core of the subset gate: collect one description string
    /// per `VarHome::Kernel(None, ..)` member found in `qual_vars`, skipping
    /// qualifiers in `excluded_quals` and `(qualifier, name)` pairs in
    /// `deliberately_unbacked_members`. An empty return means every
    /// non-excluded, non-deliberately-unbacked member resolves to a concrete
    /// `StdlibKernel` id — the property the gate exists to enforce
    /// (`crates/ipe_canon/src/lib.rs`'s `canon_equals_registry`).
    fn qual_vars_none_violations(
        qual_vars: &std::collections::BTreeMap<
            ipe_intern::Symbol,
            std::collections::BTreeMap<ipe_intern::Symbol, crate::env::VarHome>,
        >,
        interner: &ipe_intern::Interner,
        excluded_quals: &std::collections::BTreeSet<&str>,
        deliberately_unbacked_members: &std::collections::BTreeSet<(&str, &str)>,
    ) -> Vec<String> {
        use crate::env::VarHome;

        let mut unbacked = Vec::new();
        for (qual_sym, members) in qual_vars {
            let qual_str = interner.resolve(*qual_sym).unwrap_or("<unknown>");
            if excluded_quals.contains(qual_str) {
                continue;
            }
            for (name_sym, home) in members {
                let name_str = interner.resolve(*name_sym).unwrap_or("<unknown>");
                if deliberately_unbacked_members.contains(&(qual_str, name_str)) {
                    continue;
                }
                if let VarHome::Kernel(None, m, f) = home {
                    let m_str = interner.resolve(*m).unwrap_or("<unknown>");
                    let f_str = interner.resolve(*f).unwrap_or("<unknown>");
                    unbacked.push(format!(
                        "qual_vars[{qual_str:?}][{name_str:?}] is \
                         VarHome::Kernel(None, {m_str:?}, {f_str:?})",
                    ));
                }
            }
        }
        unbacked
    }

    /// **Regression proof**: the subset gate
    /// (`qual_vars_none_violations`, exercised for real by
    /// `canon_equals_registry`) actually FIRES on a kernel-without-scheme,
    /// rather than being a check that just happens to stay silent on today's
    /// (already-clean-of-undocumented-holes) table. This is the direct,
    /// durable proof that the gate provably catches the exact
    /// exit-0-then-cargo-fail defect class `PRINCIPLES.md` names: "a kernel
    /// that the resolver recognises but the type-scheme table does not cover."
    ///
    /// Builds a minimal synthetic `qual_vars`-shaped fixture — bypassing
    /// `Env::initial` entirely — with one qualifier holding a
    /// `VarHome::Kernel(None, ..)` member that is in neither exclusion set,
    /// and asserts the gate reports exactly that violation.
    #[test]
    fn qual_vars_subset_gate_catches_unbacked_member() {
        use crate::env::VarHome;
        use ipe_intern::Interner;

        let mut interner = Interner::new();
        let qual_sym = interner.intern("Totally.Fake").expect("intern OOM");
        let name_sym = interner.intern("madeUpKernel").expect("intern OOM");

        let mut members = std::collections::BTreeMap::new();
        // A kernel the resolver recognises (it is a real qual_vars member)
        // but the type-scheme table cannot possibly cover, because — like
        // real drift such as `Ui.image` / `Html.doctype` — no matching
        // `StdlibKernel::ALL` entry exists for it at all.
        members.insert(name_sym, VarHome::Kernel(None, qual_sym, name_sym));

        let mut qual_vars = std::collections::BTreeMap::new();
        qual_vars.insert(qual_sym, members);

        let excluded_quals: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let deliberately_unbacked_members: std::collections::BTreeSet<(&str, &str)> =
            std::collections::BTreeSet::new();

        let violations = qual_vars_none_violations(
            &qual_vars,
            &interner,
            &excluded_quals,
            &deliberately_unbacked_members,
        );

        assert_eq!(
            violations.len(),
            1,
            "expected exactly one violation for the synthetic unbacked \
             member, got {violations:?}",
        );
        assert!(
            violations
                .iter()
                .any(|v| v.contains("Totally.Fake") && v.contains("madeUpKernel")),
            "violation message doesn't name the synthetic kernel: {violations:?}",
        );

        // Sanity check on the OTHER direction: excluding the qualifier makes
        // the same fixture report zero violations, proving the exclusion
        // mechanism itself works (not just that the detector fires
        // unconditionally).
        let excluded_this_time: std::collections::BTreeSet<&str> =
            std::iter::once("Totally.Fake").collect();
        let violations_excluded = qual_vars_none_violations(
            &qual_vars,
            &interner,
            &excluded_this_time,
            &deliberately_unbacked_members,
        );
        assert!(
            violations_excluded.is_empty(),
            "expected zero violations once the qualifier is excluded, got {violations_excluded:?}",
        );

        // And via the member-granular exception set.
        let member_excluded: std::collections::BTreeSet<(&str, &str)> =
            std::iter::once(("Totally.Fake", "madeUpKernel")).collect();
        let violations_member_excluded =
            qual_vars_none_violations(&qual_vars, &interner, &excluded_quals, &member_excluded);
        assert!(
            violations_member_excluded.is_empty(),
            "expected zero violations once the member is excluded, got {violations_member_excluded:?}",
        );
    }

    /// The `KNOWN_UNBACKED` kernels (`PubSub.publish` /
    /// `PubSub.publishNoEcho`) are UNREACHABLE from any user program: their
    /// `"PubSub"` qualifier is absent from `env.qual_vars`, so canonicalisation
    /// never mints a `VarKernel` for them. This is the fact that keeps the
    /// `stdlib_scheme` totality flip sound — the `None` arm the
    /// two `PubSub` variants return can never be hit on a real resolution path.
    ///
    /// Mirrors `ipe_types` `KNOWN_UNBACKED` (guarded there by
    /// `known_unbacked_never_schemed`); enumerated here directly because that
    /// const is private to `ipe_types`.
    #[test]
    fn known_unbacked_disjoint_from_qual_vars() {
        use ipe_intern::Interner;
        use ipe_kernels::StdlibKernel;

        let mut interner = Interner::new();
        let env = Env::initial(vec![], &mut interner)
            .expect("Env::initial must not fail in the tripwire test");

        for sk in [
            StdlibKernel::PubSubPublish,
            StdlibKernel::PubSubPublishNoEcho,
        ] {
            let qual = sk.decl().qualifier;
            let Ok(qual_sym) = interner.intern(qual) else {
                continue;
            };
            assert!(
                !env.qual_vars.contains_key(&qual_sym),
                "KNOWN_UNBACKED {sk:?} qualifier {qual:?} IS present in \
                 env.qual_vars — it is reachable, so the stdlib_scheme None arm \
                 could be hit and the Phase E totality flip would be unsound. \
                 Either scheme it or remove it from qual_vars.",
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Record type-alias auto-constructor (IPE-N0001).
    // ─────────────────────────────────────────────────────────────────────────

    /// Flatten an arrow type into `(arg types…, final result type)`.
    fn arrow_spine(ty: &ast::Type) -> (Vec<&ast::Type>, &ast::Type) {
        let mut args = Vec::new();
        let mut cur = ty;
        while let ast::Type::Lambda(a, b) = cur {
            args.push(a.as_ref());
            cur = b.as_ref();
        }
        (args, cur)
    }

    #[test]
    fn record_alias_synthesizes_typed_ctor() {
        // `type alias Profile = { name : String, age : Int }` introduces a value
        // `Profile : String -> Int -> { name:String, age:Int }`.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Profile =\n    { name : String, age : Int }\n\n\
             main = 0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        let Some(Def::Typed {
            patterns,
            body,
            ty,
            free_vars,
            ..
        }) = find_def(&m, &i, "Profile")
        else {
            assert!(false_marker(), "Profile is a synthesized typed def");
            return;
        };
        assert!(free_vars.is_empty(), "monomorphic record: no free vars");
        // Two params, in declared order.
        let pnames: Vec<&str> = patterns
            .iter()
            .filter_map(|p| match &p.value {
                Pattern_::PVar(s) => i.resolve(*s),
                _ => None,
            })
            .collect();
        assert_eq!(pnames, vec!["name", "age"], "params in declared order");
        // Body is a record literal of VarLocal refs in declared order.
        let Expr_::Record(fields) = &body.value else {
            assert!(false_marker(), "body is a record literal");
            return;
        };
        let bnames: Vec<&str> = fields.iter().filter_map(|(f, _)| i.resolve(*f)).collect();
        assert_eq!(bnames, vec!["name", "age"]);
        for (f, e) in fields {
            assert!(
                matches!(&e.value, Expr_::VarLocal(s) if s == f),
                "each field value is the eponymous local"
            );
        }
        // Arrow: String -> Int -> { name, age }.
        let (arg_tys, result) = arrow_spine(ty);
        assert_eq!(arg_tys.len(), 2, "two arrow arguments");
        assert!(
            matches!(arg_tys.first(), Some(ast::Type::Con { name, .. }) if i.resolve(*name) == Some("String"))
        );
        assert!(
            matches!(arg_tys.get(1), Some(ast::Type::Con { name, .. }) if i.resolve(*name) == Some("Int"))
        );
        let ast::Type::Record(rfields) = result else {
            assert!(false_marker(), "result is a closed record type");
            return;
        };
        let rnames: Vec<&str> = rfields.iter().filter_map(|(f, _)| i.resolve(*f)).collect();
        assert_eq!(
            rnames,
            vec!["name", "age"],
            "record fields in declared order"
        );
    }

    #[test]
    fn record_alias_ctor_field_order_is_declared_not_alphabetical() {
        // Non-alphabetical declared order `{ zebra, apple }` must produce the
        // ctor `zebra -> apple`, NOT alphabetised — the field-order guarantee.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Row =\n    { zebra : Int, apple : String }\n\n\
             main = 0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        let Some(Def::Typed { patterns, ty, .. }) = find_def(&m, &i, "Row") else {
            assert!(false_marker(), "Row is a synthesized typed def");
            return;
        };
        let pnames: Vec<&str> = patterns
            .iter()
            .filter_map(|p| match &p.value {
                Pattern_::PVar(s) => i.resolve(*s),
                _ => None,
            })
            .collect();
        assert_eq!(pnames, vec!["zebra", "apple"], "declared, not alphabetical");
        let (arg_tys, _) = arrow_spine(ty);
        // First arg `zebra : Int`, second `apple : String` — positional binding.
        assert!(
            matches!(arg_tys.first(), Some(ast::Type::Con { name, .. }) if i.resolve(*name) == Some("Int")),
            "first arg type is Int (zebra), got {:?}",
            arg_tys.first()
        );
        assert!(
            matches!(arg_tys.get(1), Some(ast::Type::Con { name, .. }) if i.resolve(*name) == Some("String")),
            "second arg type is String (apple), got {:?}",
            arg_tys.get(1)
        );
    }

    #[test]
    fn record_alias_ctor_resolves_as_a_value() {
        // Bare use of the alias name as a value resolves to a top-level binding,
        // not a name error — the IPE-N0001 fix.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias P =\n    { a : Int }\n\n\
             mk = P\n\
             main = 0\n",
        );
        assert!(m.is_some(), "bare `P` used as a value must resolve");
        let Some(m) = m else { return };
        let body = match find_def(&m, &i, "mk") {
            Some(Def::Untyped { body, .. } | Def::Typed { body, .. }) => Some(&body.value),
            None => None,
        };
        assert!(
            matches!(body, Some(Expr_::VarTopLevel { name, .. }) if i.resolve(*name) == Some("P")),
            "`mk = P` resolves P to a top-level ctor, got {body:?}"
        );
    }

    #[test]
    fn parametric_record_alias_generalises_over_used_params() {
        // `type alias Box a = { value : a, tag : String }` → the ctor generalises
        // over `a`: `Box : a -> String -> { value:a, tag:String }`. The param
        // canonicalises to `Type::Var`, never an unknown/opaque `Con`.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Box a =\n    { value : a, tag : String }\n\n\
             main = 0\n",
        );
        assert!(m.is_some(), "parametric record alias must canonicalise");
        let Some(m) = m else { return };
        let Some(Def::Typed { free_vars, ty, .. }) = find_def(&m, &i, "Box") else {
            assert!(false_marker(), "Box is a synthesized typed def");
            return;
        };
        let fv: Vec<&str> = free_vars.iter().filter_map(|s| i.resolve(*s)).collect();
        assert_eq!(fv, vec!["a"], "generalises over the used param `a` only");
        let (arg_tys, _) = arrow_spine(ty);
        assert!(
            matches!(arg_tys.first(), Some(ast::Type::Var(s)) if i.resolve(*s) == Some("a")),
            "first arg is the type variable `a` (not UnknownType/Con), got {:?}",
            arg_tys.first()
        );
    }

    #[test]
    fn phantom_param_drops_out_of_ctor_scheme() {
        // A declared-but-unused param must NOT appear in the ctor's free vars.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Tagged phantom =\n    { label : String }\n\n\
             main = 0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        let Some(Def::Typed { free_vars, .. }) = find_def(&m, &i, "Tagged") else {
            assert!(false_marker(), "Tagged is a synthesized typed def");
            return;
        };
        assert!(
            free_vars.is_empty(),
            "phantom param must not generalise the ctor, got {:?}",
            free_vars
                .iter()
                .filter_map(|s| i.resolve(*s))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_record_alias_has_no_ctor_and_still_errors_as_value() {
        // `type alias Count = Int` gets NO value binding; using it as a value
        // stays an ordinary IPE-N0001 ValueNotFound (Elm parity).
        let mut i = Interner::new();
        let ok = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Count = Int\n\n\
             main = 0\n",
        );
        assert!(ok.is_some(), "control module canonicalises");
        if let Some(m) = ok {
            assert!(
                find_def(&m, &i, "Count").is_none(),
                "non-record alias must not synthesize a def"
            );
        }
        // Now use it as a value → ValueNotFound.
        let err = canon_err(
            "module Main exposing (main)\n\
             type alias Count = Int\n\n\
             main = Count\n",
        );
        assert!(
            matches!(
                err,
                Some(Diagnostic::Name {
                    msg: NameError::ValueNotFound { .. },
                    ..
                })
            ),
            "non-record alias used as a value must be ValueNotFound, got {err:?}"
        );
    }

    #[test]
    fn head_alias_to_record_alias_gets_no_ctor() {
        // `type alias U = P` where P is a record alias: U's SOURCE body is a
        // TType, not a literal TRecord, so U gets NO constructor (Elm parity).
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias P =\n    { a : Int }\n\
             type alias U = P\n\n\
             main = 0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        assert!(
            find_def(&m, &i, "P").is_some(),
            "the literal record alias P has a ctor"
        );
        assert!(
            find_def(&m, &i, "U").is_none(),
            "the head alias U must NOT get a ctor"
        );
    }

    #[test]
    fn record_alias_name_coinciding_with_data_ctor_is_allowed() {
        // A record alias whose name also names a data constructor is VALID: the
        // TYPE namespace (`type alias Foo`) and the CONSTRUCTOR namespace
        // (`type Bar = Foo`) are distinct. The upstream Haskell (`registerAliases`)
        // inserts into `_vars` without checking `_ctors`, so both coexist.
        //
        // Previously this wrongly emitted IPE-N0010 (DuplicateValue). The fix
        // changes `synthesize_record_alias_ctors` to `continue` (skip synthesis)
        // when the alias name coincides with a known ADT constructor, instead of
        // erroring — achieving the same "ADT ctor wins in expression position"
        // outcome more cleanly.
        //
        // Ref: upstream `Ipe.Canonicalise.Module.registerAliases`.
        let src = "module Main exposing (main)\n\
                   type alias Foo =\n    { x : Int }\n\
                   type Bar = Foo\n\n\
                   main = 0\n";
        let mut i = Interner::new();
        let Ok(parsed) = ipe_parse::parse_module(src, &mut i) else {
            assert!(false_marker(), "source must parse");
            return;
        };
        assert!(
            canonicalise(&parsed, &mut i).is_ok(),
            "record alias `Foo` and ADT ctor `Foo` in `type Bar = Foo` must \
             coexist without N0010 — they live in separate namespaces"
        );
    }

    #[test]
    fn explicit_binding_suppresses_record_alias_ctor_synthesis() {
        // A user-written top-level value sharing a record alias's name IS the
        // constructor — the explicit binding provides the implementation and
        // SUPPRESSES synthesis of the auto-ctor (upstream Rust emitter's
        // `existingNames` guard). The two do NOT collide as DuplicateValue; the
        // module canonicalises, and the single `Mk` def is the user's binding.
        //
        // This is the `06-json` pattern: `type alias Profile = { … }` plus an
        // explicit `Profile name age active = { … }` record-constructor helper.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Mk =\n    { x : Int }\n\n\
             Mk n =\n    { x = n }\n\n\
             main = 0\n",
        );
        assert!(
            m.is_some(),
            "an explicit record-ctor binding sharing the alias name must \
             canonicalise (synthesis suppressed), not fail with DuplicateValue"
        );
        let Some(m) = m else { return };
        // Exactly ONE `Mk` def — the user's binding, not a synthesized ctor
        // duplicated alongside it.
        let mk_defs = m
            .defs
            .iter()
            .filter(|d| i.resolve(d.name().value) == Some("Mk"))
            .count();
        assert_eq!(
            mk_defs, 1,
            "the user's explicit `Mk` binding is the sole def; the auto-ctor \
             must be suppressed, not emitted alongside it"
        );
    }

    #[test]
    fn function_field_record_alias_has_no_ctor() {
        // A config-record alias with an ARROW-headed field must NOT synthesize a
        // constructor — its body would be a record literal with a function field,
        // which the lowerer rejects (IPE-L0107), and there is no DCE to prune an
        // unused one. It stays a type only, with no synthesized constructor.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Cfg =\n    { run : Int -> Int, label : String }\n\n\
             main = 0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        assert!(
            find_def(&m, &i, "Cfg").is_none(),
            "function-field record alias must not get a constructor"
        );
    }

    #[test]
    fn generic_carrier_field_without_function_still_gets_ctor() {
        // A field carrying a GENERIC (non-function, non-opaque) argument
        // (`List a`) is derivable, so the struct-derivability gate keeps its
        // constructor. This is the control that the recursive
        // `field_type_nonderivable` predicate does NOT over-gate an ordinary
        // parametric container — only an embedded arrow (see
        // `nested_function_in_derive_carrier_has_no_ctor`) or an opaque wrapper
        // (see `opaque_wrapper_field_record_alias_gets_no_ctor`) blocks synthesis.
        let mut i = Interner::new();
        let m = canon_ok(
            &mut i,
            "module Main exposing (main)\n\
             type alias Wrap a =\n    { items : List a, count : Int }\n\n\
             main = 0\n",
        );
        assert!(m.is_some(), "module must canonicalise");
        let Some(m) = m else { return };
        assert!(
            find_def(&m, &i, "Wrap").is_some(),
            "a record alias with only non-function-embedding fields keeps its ctor"
        );
    }

    #[test]
    fn nested_function_in_derive_carrier_has_no_ctor() {
        // SEAL FIX. A field whose function is NESTED inside a derive carrier
        // — `List (Int -> Bool)` (head `Con "List"`, not `Lambda`) — was MISSED by
        // the earlier head-only gate: a ctor was synthesised, the backend emitted a
        // `#[derive(Clone, Debug, PartialEq)]` struct over a `Box<dyn Fn>` field,
        // and ipe exited 0 while cargo failed (the seal violation). The recursive
        // gate now declines synthesis, so merely NAMING the alias builds clean.
        let cases = [
            "type alias T =\n    { xs : List (Int -> Int) }\n",
            "type alias T =\n    { f : Maybe (Int -> Int) }\n",
            "type alias T =\n    { p : (Int -> Int, Bool) }\n",
            "type alias T =\n    { g : Result Error (Int -> Int) }\n",
            "type alias T =\n    { inner : { h : Int -> Int } }\n",
        ];
        for body in cases {
            let mut i = Interner::new();
            let src = format!("module Main exposing (main)\n{body}\nmain = 0\n");
            let m = canon_ok(&mut i, &src);
            assert!(m.is_some(), "module must canonicalise: {body}");
            let Some(m) = m else { continue };
            assert!(
                find_def(&m, &i, "T").is_none(),
                "a record alias embedding a nested function must NOT get a ctor: {body}"
            );
        }
    }

    #[test]
    fn opaque_wrapper_field_record_alias_gets_no_ctor() {
        // ROUND-2 SEAL FIX. An opaque boxed-wrapper in FIELD position
        // (`Decoder` / `Cmd` / `Sub` / `Task`) is ITSELF non-derivable as a
        // struct field — its runtime rep (`Box<dyn Fn>` / boxed-thunk enum /
        // `Pin<Box<dyn Future>>`) impls no Clone/Debug/PartialEq/IpeStringify.
        // Round-1 synthesised a ctor here, so the backend emitted a
        // `#[derive(…)]` struct over the wrapper and ipe-0 then cargo-101 (the
        // seal hole). The struct-derivability gate now DECLINES synthesis, so
        // merely NAMING the alias builds clean and no dangling ctor value exists.
        // Use only builtin type args (`Int`, `Error`) so the test is not
        // sensitive to whether an undefined user ADT (`Msg`) compiles.
        // Unknown unqualified type names fail closed with IPE-N0002;
        // the test's intent (no ctor for opaque-field alias) does not depend on
        // the specific type argument — `Cmd Int` tests the same gate as `Cmd Msg`.
        for (decl, field_ty) in [
            ("Dec", "Decoder Int"),
            ("Ev", "Cmd Int"),
            ("Sb", "Sub Int"),
            ("Tk", "Task Error Int"),
        ] {
            let mut i = Interner::new();
            let src = format!(
                "module Main exposing (main)\n\
                 type alias {decl} =\n    {{ payload : {field_ty} }}\n\n\
                 main = 0\n"
            );
            let m = canon_ok(&mut i, &src);
            assert!(m.is_some(), "module must canonicalise: {field_ty}");
            let Some(m) = m else { continue };
            // NO constructor Def is synthesised for an opaque-wrapper-field alias.
            assert!(
                find_def(&m, &i, decl).is_none(),
                "an opaque-wrapper-field record alias must NOT get a ctor: {field_ty}"
            );
        }
    }

    #[test]
    fn function_field_alias_is_not_exported_as_a_value() {
        // Exports must match synthesis: a gated-out function-field alias exports
        // its TYPE but NOT a (non-existent) constructor value — otherwise an
        // importer would inject a dangling binding.
        let mut i = Interner::new();
        let src = "module Lib exposing (..)\n\
                   type alias Cfg =\n    { run : Int -> Int }\n";
        let Ok(parsed) = ipe_parse::parse_module(src, &mut i) else {
            assert!(false_marker(), "parse");
            return;
        };
        let expected = parsed.name.value.clone();
        let deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        let Ok((_m, exports)) = canonicalise_module(&parsed, &expected, &deps, &mut i) else {
            assert!(false_marker(), "Lib must canonicalise");
            return;
        };
        let cfg = i.intern("Cfg").expect("intern Cfg");
        assert!(
            exports.aliases.contains_key(&cfg),
            "Cfg is still exported as a type alias"
        );
        assert!(
            !exports.values.contains(&cfg),
            "Cfg must NOT be exported as a value (no ctor synthesized)"
        );
    }

    #[test]
    fn exposed_record_alias_exports_its_ctor_value() {
        // `exposing (..)` on a module with a record alias must export the alias
        // name in BOTH the type namespace (aliases) and the value namespace
        // (values), so an importer can use it as a constructor.
        let mut i = Interner::new();
        let src = "module Lib exposing (..)\n\
                   type alias Widget =\n    { w : Int, h : Int }\n";
        let Ok(parsed) = ipe_parse::parse_module(src, &mut i) else {
            assert!(false_marker(), "parse");
            return;
        };
        let expected = parsed.name.value.clone();
        let deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        let Ok((_m, exports)) = canonicalise_module(&parsed, &expected, &deps, &mut i) else {
            assert!(false_marker(), "Lib must canonicalise");
            return;
        };
        let widget = i.intern("Widget").expect("intern Widget");
        assert!(
            exports.aliases.contains_key(&widget),
            "Widget is exported as a type alias"
        );
        assert!(
            exports.values.contains(&widget),
            "Widget's auto-constructor is exported as a value"
        );
    }

    #[test]
    fn exposed_record_alias_via_list_exports_its_ctor_value() {
        // Explicit `exposing (Widget)` (list form) must also export the value.
        let mut i = Interner::new();
        let src = "module Lib exposing (Widget)\n\
                   type alias Widget =\n    { w : Int }\n";
        let Ok(parsed) = ipe_parse::parse_module(src, &mut i) else {
            assert!(false_marker(), "parse");
            return;
        };
        let expected = parsed.name.value.clone();
        let deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        let Ok((_m, exports)) = canonicalise_module(&parsed, &expected, &deps, &mut i) else {
            assert!(false_marker(), "Lib must canonicalise");
            return;
        };
        let widget = i.intern("Widget").expect("intern Widget");
        assert!(
            exports.values.contains(&widget),
            "list-exposed record alias must export its ctor value"
        );
    }

    // ── `import Ipê.*/Ipe.* exposing (member)` brings stdlib VALUE members
    // into UNQUALIFIED scope ─────────────────────────────────────────────────

    #[test]
    fn stdlib_exposing_brings_value_into_unqualified_scope() {
        // `import Ipe.Tea.Web exposing (app, route)` → bare `app` resolves to the
        // same `VarKernel { module: Web, name: app }` a `Web.app` reference
        // would. Previously this was `IPE-N0001` "app not found".
        let src = "module Main exposing (main)\n\
                   import Ipe.Tea.Web exposing (app, route)\n\n\
                   main = app\n";
        let Some((m, i)) = canon_src(src) else {
            assert!(false_marker(), "exposing (app, route) must canonicalise");
            return;
        };
        assert_main_is_kernel(&m, &i, "Web", "app");
    }

    #[test]
    fn stdlib_exposing_println_resolves_unqualified() {
        // `import Ipe.Io exposing (println)` → bare `println` resolves via the
        // exposing path to `VarKernel { module: Io, name: println }`.
        let src = "module Main exposing (main)\n\
                   import Ipe.Io exposing (println)\n\n\
                   main = println\n";
        let Some((m, i)) = canon_src(src) else {
            assert!(false_marker(), "exposing (println) must canonicalise");
            return;
        };
        assert_main_is_kernel(&m, &i, "Io", "println");
    }

    #[test]
    fn stdlib_exposing_nonmember_is_name_not_exposed() {
        // Fail-closed: a lowercase name that is NOT a real value member of the
        // module surfaces `NameNotExposed`, never a dangling unqualified binding.
        let err = canon_err(
            "module Main exposing (main)\n\
             import Ipe.Tea.Web exposing (bogusFn)\n\
             main = 0\n",
        );
        let Some(Diagnostic::Name {
            msg: NameError::NameNotExposed { module, name, .. },
            ..
        }) = &err
        else {
            assert!(false_marker(), "expected NameNotExposed, got {err:?}");
            return;
        };
        assert_eq!(&**name, "bogusFn");
        assert_eq!(&**module, "Ipe.Web");
    }

    #[test]
    fn stdlib_exposed_name_colliding_with_local_is_duplicate_value() {
        // An exposed name folds into `seen_values`, so a user top-level value of
        // the same name is a genuine conflict (`DuplicateValue`), matching Elm's
        // rule that importing a name and defining it locally clash.
        let err = canon_err(
            "module Main exposing (main)\n\
             import Ipe.Tea.Web exposing (app)\n\
             app = 1\n\
             main = 0\n",
        );
        assert!(
            matches!(
                &err,
                Some(Diagnostic::Name {
                    msg: NameError::DuplicateValue { .. },
                    ..
                })
            ),
            "expected DuplicateValue, got {err:?}"
        );
    }

    #[test]
    fn stdlib_exposing_type_is_untouched() {
        // Capitalized TYPE exposures (`exposing (Element)`) are kernel-implicit
        // types resolved elsewhere — the value-injection pass must NOT reject
        // them as non-members. This must canonicalise cleanly.
        let ok = canon_src(
            "module Main exposing (main)\n\
             import Ipe.Ui exposing (Element)\n\
             main = 0\n",
        );
        assert!(
            ok.is_some(),
            "type exposure of a stdlib module must not be rejected as a non-member value"
        );
    }

    #[test]
    fn stdlib_exposing_wildcard_allows_local_shadow() {
        // `exposing (..)` on a stdlib module floods the LOW-PRIORITY
        // wildcard tier. A local `map` must NOT collide (no `DuplicateValue`) and
        // a bare `map` use must resolve to the LOCAL binding, silently shadowing
        // the wildcard member.
        let src = "module Main exposing (main)\n\
                   import Ipe.Prelude exposing (..)\n\
                   map = 1\n\
                   main = map\n";
        let Some((m, i)) = canon_src(src) else {
            assert!(false_marker(), "local shadow of a wildcard member is legal");
            return;
        };
        let body = match find_def(&m, &i, "main") {
            Some(Def::Untyped { body, .. } | Def::Typed { body, .. }) => Some(&body.value),
            None => None,
        };
        assert!(
            matches!(body, Some(Expr_::VarTopLevel { name, .. }) if i.resolve(*name) == Some("map")),
            "bare `map` must resolve to the LOCAL top-level binding, got {body:?}"
        );
    }

    // ── `import Ipê.*/Ipe.* exposing (..)` floods the low-priority wildcard
    // tier ─────────────────────────────────────────────────────────────────────

    #[test]
    fn stdlib_wildcard_brings_member_into_unqualified_scope() {
        // `import Ipe.Html exposing (..)` → bare `div` resolves to the same
        // `VarKernel { module: Html, name: div }` a `Html.div` reference would.
        // This is the exact 09-live-counter blocker (IPE-N0001 on bare `div`).
        let src = "module Main exposing (main)\n\
                   import Ipe.Html exposing (..)\n\n\
                   main = div\n";
        let Some((m, i)) = canon_src(src) else {
            assert!(false_marker(), "wildcard `div` must canonicalise");
            return;
        };
        assert_main_is_kernel(&m, &i, "Html", "div");
    }

    #[test]
    fn stdlib_wildcard_member_lowers_identically_to_qualified() {
        // A wildcard `text` and a qualified `Html.text` must produce the same
        // `VarKernel` (identical module + name), so lowering is unaffected.
        let bare = "module Main exposing (main)\n\
                    import Ipe.Html exposing (..)\n\n\
                    main = text\n";
        let qual = "module Main exposing (main)\n\
                    import Ipe.Html\n\n\
                    main = Html.text\n";
        let Some((mb, ib)) = canon_src(bare) else {
            assert!(false_marker(), "bare wildcard `text` must canonicalise");
            return;
        };
        let Some((mq, iq)) = canon_src(qual) else {
            assert!(false_marker(), "qualified `Html.text` must canonicalise");
            return;
        };
        let kernel_of = |m: &ast::Module, i: &Interner| -> Option<(String, String)> {
            match find_def(m, i, "main") {
                Some(Def::Untyped { body, .. } | Def::Typed { body, .. }) => match &body.value {
                    Expr_::VarKernel { module, name, .. } => Some((
                        i.resolve(*module)?.to_string(),
                        i.resolve(*name)?.to_string(),
                    )),
                    _ => None,
                },
                None => None,
            }
        };
        assert_eq!(
            kernel_of(&mb, &ib),
            Some(("Html".to_string(), "text".to_string())),
            "bare wildcard `text` resolves to VarKernel(Html, text)"
        );
        assert_eq!(
            kernel_of(&mb, &ib),
            kernel_of(&mq, &iq),
            "wildcard and qualified references must lower identically"
        );
    }

    #[test]
    fn two_stdlib_wildcards_same_name_is_ambiguous_at_use() {
        // Both `Ipe.Html` and `Ipe.Ui` export `text`. Two `exposing (..)` imports
        // are BOTH legal at import time; a bare `text` USE is `AmbiguousImport`
        // (IPE-N0024), never a silent last-wins.
        let err = canon_err(
            "module Main exposing (main)\n\
             import Ipe.Html exposing (..)\n\
             import Ipe.Ui exposing (..)\n\
             main = text\n",
        );
        let Some(Diagnostic::Name {
            msg: NameError::AmbiguousImport { name, modules },
            ..
        }) = &err
        else {
            assert!(false_marker(), "expected AmbiguousImport, got {err:?}");
            return;
        };
        assert_eq!(&**name, "text");
        assert!(
            modules.iter().any(|m| &**m == "Ipe.Html") && modules.iter().any(|m| &**m == "Ipe.Ui"),
            "both origins named, got {modules:?}"
        );
    }

    #[test]
    fn two_stdlib_wildcards_shared_name_ok_when_unused() {
        // The ambiguity is deferred: two wildcards exposing `text` are legal as
        // long as no bare `text` is used (a non-shared name still resolves).
        let ok = canon_src(
            "module Main exposing (main)\n\
             import Ipe.Html exposing (..)\n\
             import Ipe.Ui exposing (..)\n\
             main = div\n",
        );
        assert!(
            ok.is_some(),
            "unused shared wildcard name must not fail at import time"
        );
    }

    #[test]
    fn two_stdlib_wildcards_ambiguity_resolved_by_local() {
        // A local binding silently shadows BOTH wildcard origins — no ambiguity,
        // no `DuplicateValue`.
        let src = "module Main exposing (main)\n\
                   import Ipe.Html exposing (..)\n\
                   import Ipe.Ui exposing (..)\n\
                   text = 1\n\
                   main = text\n";
        let Some((m, i)) = canon_src(src) else {
            assert!(false_marker(), "local shadow resolves the ambiguity");
            return;
        };
        let body = match find_def(&m, &i, "main") {
            Some(Def::Untyped { body, .. } | Def::Typed { body, .. }) => Some(&body.value),
            None => None,
        };
        assert!(
            matches!(body, Some(Expr_::VarTopLevel { name, .. }) if i.resolve(*name) == Some("text")),
            "local `text` shadows both wildcards, got {body:?}"
        );
    }

    #[test]
    fn stdlib_wildcard_shadowed_by_explicit_exposing() {
        // An explicit `exposing (text)` (higher priority, in `env.vars`) wins over
        // a wildcard `text`; the pair is NOT ambiguous. Resolves to Html.text.
        let src = "module Main exposing (main)\n\
                   import Ipe.Ui exposing (..)\n\
                   import Ipe.Html exposing (text)\n\
                   main = text\n";
        let Some((m, i)) = canon_src(src) else {
            assert!(false_marker(), "explicit exposure wins over wildcard");
            return;
        };
        assert_main_is_kernel(&m, &i, "Html", "text");
    }

    #[test]
    fn stdlib_wildcard_same_module_twice_not_ambiguous() {
        // Importing the same module under an alias AND a wildcard must not fake a
        // self-ambiguity (dedup by canonical qualifier).
        let src = "module Main exposing (main)\n\
                   import Ipe.Html exposing (..)\n\
                   import Ipe.Html as H exposing (..)\n\
                   main = div\n";
        let Some((m, i)) = canon_src(src) else {
            assert!(false_marker(), "same module twice must not be ambiguous");
            return;
        };
        assert_main_is_kernel(&m, &i, "Html", "div");
    }

    #[test]
    fn explicit_exposing_still_collides_with_local() {
        // An EXPLICIT `exposing (app)` hard-collides
        // with a local `app` (`DuplicateValue`) — unlike a wildcard member.
        let err = canon_err(
            "module Main exposing (main)\n\
             import Ipe.Tea.Web exposing (app)\n\
             app = 1\n\
             main = 0\n",
        );
        assert!(
            matches!(
                &err,
                Some(Diagnostic::Name {
                    msg: NameError::DuplicateValue { .. },
                    ..
                })
            ),
            "explicit exposure must still collide, got {err:?}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // ModuleOrigin: unforgeable stdlib trust tag.
    // ─────────────────────────────────────────────────────────────────────────

    /// Canonicalise `src` with an explicit [`ModuleOrigin`], returning the result.
    fn canon_with_origin(src: &str, origin: ModuleOrigin) -> DResult<(ast::Module, ModuleExports)> {
        let mut i = Interner::new();
        let parsed = ipe_parse::parse_module(src, &mut i).expect("spike source parses");
        let expected: Vec<Symbol> = parsed.name.value.clone();
        let deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
        canonicalise_module_with_origin(&parsed, &expected, &deps, origin, &mut i)
    }

    /// The exact spike module, declaring `module Ipe.Palette` and matching its
    /// own ctor.
    const PALETTE_SRC: &str = "module Ipe.Palette exposing (Shade(..), toHex)\n\
         type Shade = Dark | Light\n\
         toHex : Shade -> String\n\
         toHex shade =\n    case shade of\n        Dark -> \"#000\"\n        Light -> \"#fff\"\n";

    #[test]
    fn embedded_stdlib_origin_exempts_reserved_namespace() {
        // The compiled-source path: a driver-vouched `Ipe.Palette` is accepted,
        // reserved-namespace gate exempted — it is the legitimate definer.
        let res = canon_with_origin(PALETTE_SRC, ModuleOrigin::EmbeddedStdlib);
        assert!(
            res.is_ok(),
            "EmbeddedStdlib Ipe.Palette must canonicalise: {:?}",
            res.err()
        );
    }

    #[test]
    fn user_origin_std_module_is_reserved_namespace() {
        // SECURITY: the SAME text, tagged User (a hostile file literally named
        // `Ipe.Palette`), stays N0025-rejected. Trust is the tag, not the name.
        let err = canon_with_origin(PALETTE_SRC, ModuleOrigin::User)
            .expect_err("user Ipe.Palette must be rejected");
        assert!(
            matches!(
                &err,
                Diagnostic::Name {
                    msg: NameError::ReservedNamespace { .. },
                    ..
                }
            ),
            "hostile user Ipe.Palette must be IPE-N0025, got {err:?}"
        );
    }

    #[test]
    fn embedded_stdlib_unannotated_binding_fails_closed() {
        // The fail-closed annotation gate: an EmbeddedStdlib module with an
        // un-annotated top-level binding is a compiler-internal error at canon,
        // never an exit-0-then-cargo-fail. `bad` has no signature.
        let src = "module Ipe.Foo exposing (bad)\n\
             good : Int\n\
             good = 1\n\
             bad = good\n";
        let err = canon_with_origin(src, ModuleOrigin::EmbeddedStdlib)
            .expect_err("unannotated stdlib binding must fail closed");
        assert!(
            matches!(
                &err,
                Diagnostic::CompilerBug {
                    where_: "canon.stdlib_unannotated",
                    ..
                }
            ),
            "unannotated EmbeddedStdlib binding must be a fail-closed CompilerBug, got {err:?}"
        );
    }

    #[test]
    fn user_unannotated_binding_is_fine() {
        // The gate can NEVER fire for user code: an un-annotated top-level in a
        // normal user module is business as usual.
        let res = canon_with_origin(
            "module Main exposing (main)\nmain = 0\n",
            ModuleOrigin::User,
        );
        assert!(
            res.is_ok(),
            "user unannotated main is fine: {:?}",
            res.err()
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // ModuleOrigin-gated reserved-builtin exemption.
    //
    // The unforgeable `ModuleOrigin` and the home-aware lowerer (the nullary
    // Ipe.Ui opaque names sit BELOW the `enum_variants` guard) together mean the
    // canon reservation of those names is not load-bearing
    // for lowering-soundness, and a trusted `EmbeddedStdlib` module — the
    // canonical definer — is exempt for that subset while USER modules stay
    // rejected (keeping the user-facing "cannot shadow Length" guarantee).
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn embedded_stdlib_origin_exempts_reserved_ui_type() {
        // The capability compiled-source `Ipe.Css` needs: DEFINE `type Length`
        // (a reserved built-in name). Exempt ONLY because the driver vouched for
        // the origin (unforgeable). The same text tagged User is N0026-rejected
        // — see `user_type_shadowing_builtin_rejected`.
        let src = "module Ipe.Css exposing (Length(..))\n\
             type Length = Px Int\n";
        let res = canon_with_origin(src, ModuleOrigin::EmbeddedStdlib);
        assert!(
            res.is_ok(),
            "EmbeddedStdlib `type Length` must be exempt from IPE-N0026: {:?}",
            res.err()
        );
    }

    #[test]
    fn user_origin_reserved_ui_type_still_rejected() {
        // The mirror of the exemption: the identical `type Length`, in a
        // non-Std user module (so N0025 does not pre-empt), stays IPE-N0026.
        // A hostile author gets NEITHER the namespace nor the builtin exemption.
        let src = "module Main exposing (main)\n\
             type Length = Px Int\n\
             main = 0\n";
        let err = canon_with_origin(src, ModuleOrigin::User)
            .expect_err("user `type Length` must stay reserved");
        assert!(
            matches!(
                &err,
                Diagnostic::Name {
                    msg: NameError::ReservedBuiltinType { .. },
                    ..
                }
            ),
            "user `type Length` must be IPE-N0026, got {err:?}"
        );
    }

    #[test]
    fn embedded_stdlib_origin_still_rejects_load_bearing_builtin() {
        // The carve-out is SCOPED to the below-guard nullary UI set
        // (`STDLIB_DEFINABLE_UI_TYPES`). `Html`'s lowerer arm sits ABOVE the
        // home-aware `enum_variants` guard, so a same-named union would be
        // hijacked to `IrType::Ui` and mis-lower — even trusted stdlib must not
        // redefine it. Stays IPE-N0026 for EVERY origin.
        let src = "module Ipe.Css exposing (Html(..))\n\
             type Html = Blob\n";
        let err = canon_with_origin(src, ModuleOrigin::EmbeddedStdlib)
            .expect_err("EmbeddedStdlib `type Html` must still be reserved");
        assert!(
            matches!(
                &err,
                Diagnostic::Name {
                    msg: NameError::ReservedBuiltinType { .. },
                    ..
                }
            ),
            "load-bearing builtin `Html` must stay IPE-N0026 even for stdlib, got {err:?}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // IPE-N0010 regression: type-alias name coinciding with an ADT constructor
    // must NOT produce a DuplicateValue error.  The TYPE namespace (`type alias`)
    // and the CONSTRUCTOR namespace (`type … = Ctor`) are distinct in both
    // Elm and Ipê.  Reproduces the failure seen in
    // examples/25-ipe-console/src/State.ipe where `type Tab = Overview | …`
    // and `type alias Overview = { … }` coexist in the same module.
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn type_alias_name_coinciding_with_adt_ctor_is_not_a_duplicate_value() {
        // `type Tab = Overview | Metrics | Logs` defines ADT constructors.
        // `type alias Overview = { ipeVersion : String }` defines a type alias
        // in a SEPARATE namespace.  Both should coexist without an error.
        //
        // The regression was IPE-N0010 (DuplicateValue) from
        // `synthesize_record_alias_ctors` incorrectly checking `seen_ctors`.
        let src = "module Main exposing (main)\n\n\
                   type Tab = Overview | Metrics | Logs\n\n\
                   type alias Overview =\n    { ipeVersion : String\n    , commit : String\n    }\n\n\
                   main : Int\n\
                   main = 0\n";
        let mut i = Interner::new();
        let parsed = ipe_parse::parse_module(src, &mut i);
        assert!(parsed.is_ok(), "source must parse");
        let Ok(parsed) = parsed else { return };
        let result = canonicalise(&parsed, &mut i);
        assert!(
            result.is_ok(),
            "type alias `Overview` and ADT ctor `Overview` must coexist without N0010; \
             got {result:?}"
        );
    }

    #[test]
    fn adt_ctor_wins_in_expression_position_over_same_named_alias() {
        // When both an ADT ctor `Overview` (from `type Tab = Overview | …`)
        // and a record alias `type alias Overview = { … }` share a name,
        // a bare `Overview` in expression position MUST resolve to the ADT
        // constructor, not to the alias auto-ctor (which is suppressed).
        // Verifies that `resolve_var` correctly returns `VarCtor`, not
        // `VarTopLevel`.
        let src = "module Main exposing (main)\n\n\
                   type Tab = Overview | Metrics\n\n\
                   type alias Overview =\n    { ipeVersion : String\n    }\n\n\
                   main : Tab\n\
                   main = Overview\n";
        let mut i = Interner::new();
        let parsed = ipe_parse::parse_module(src, &mut i);
        assert!(parsed.is_ok(), "source must parse");
        let Ok(parsed) = parsed else { return };
        let m = canonicalise(&parsed, &mut i);
        assert!(m.is_ok(), "must canonicalise cleanly; got {m:?}");
        let Ok(m) = m else { return };
        // `main = Overview` → the body should be a VarCtor, not a VarTopLevel.
        let Some(Def::Typed { body, .. }) = find_def(&m, &i, "main") else {
            assert!(false_marker(), "main is a typed def");
            return;
        };
        assert!(
            matches!(body.value, Expr_::VarCtor { .. }),
            "bare `Overview` in expression position must resolve to the ADT ctor, \
             not to the alias auto-ctor; got {:?}",
            body.value
        );
        let Expr_::VarCtor {
            type_name,
            name,
            index,
            ..
        } = body.value
        else {
            return;
        };
        assert_eq!(i.resolve(type_name), Some("Tab"), "ctor belongs to `Tab`");
        assert_eq!(i.resolve(name), Some("Overview"), "ctor name is `Overview`");
        assert_eq!(index, 0, "`Overview` is the first ctor");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Ipe.Ui.Lazy: module registration regression
    // ─────────────────────────────────────────────────────────────────────────

    /// `import Ipe.Ui.Lazy as Lazy` followed by a bare `Lazy.lazy` call must
    /// resolve without a name error.  If the qualifier "Lazy" were
    /// absent from `STDLIB_MODULE_QUALIFIERS` / `QUALIFIERS`, any reference
    /// to `Lazy.lazy` would fire `NameError::ValueNotFound`.
    #[test]
    fn lazy_module_lazy_resolves_without_name_error() {
        let err = canon_err(
            "module Main exposing (main)\n\
             import Ipe.Ui.Lazy as Lazy\n\
             main = Lazy.lazy identity 0\n",
        );
        assert!(
            err.is_none(),
            "#146 regression: `Lazy.lazy` must resolve cleanly; got {err:?}"
        );
    }

    /// All five arity variants (`lazy`..`lazy5`) must resolve.
    #[test]
    fn lazy_module_all_arities_resolve() {
        // Use integer literals for extra args — bare names like `x` aren't
        // in scope inside a minimal canon fixture and produce ValueNotFound.
        for (name, extra_args) in [
            ("lazy", " 0"),
            ("lazy2", " 0 1"),
            ("lazy3", " 0 1 2"),
            ("lazy4", " 0 1 2 3"),
            ("lazy5", " 0 1 2 3 4"),
        ] {
            let src = format!(
                "module Main exposing (main)\n\
                 import Ipe.Ui.Lazy as Lazy\n\
                 main = Lazy.{name} identity{extra_args}\n"
            );
            let err = canon_err(&src);
            assert!(
                err.is_none(),
                "#146 regression: `Lazy.{name}` must resolve cleanly; got {err:?}"
            );
        }
    }
}
