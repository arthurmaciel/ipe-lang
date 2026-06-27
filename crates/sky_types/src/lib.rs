#![forbid(unsafe_code)]
//! `sky_types` — Hindley-Milner type inference for the Milestone-0 subset of
//! Sky.
//!
//! Entry point: [`infer`]. It consumes a name-resolved [`sky_canon::ast::Module`]
//! and produces a [`SolvedTypes`] carrying (a) the inferred type of every
//! top-level binding (`env`) and (b) the inferred type of every sub-expression
//! source region (`regions`) — the latter being exactly what the type-directed
//! lowerer reads to fill its `IrType` slots.
//!
//! The implementation is a faithful but narrowed port of the Haskell compiler's
//! `Sky.Type.{Type,UnionFind,Unify,Solve}` + `Constrain.Expression`:
//!
//! * [`unionfind`] — `Vec`-backed weighted union-find (port of `UnionFind`).
//! * [`constrain`] — constraint generation over the canonical AST (M0 arms of
//!   `Constrain.Expression`).
//! * [`unify`] — in-place unification with an occurs check (port of `Unify`).
//! * [`solve`] — budget-bounded constraint discharge (port of `Solve`).
//!
//! ## Interner mutability
//! [`infer`] takes `&mut Interner`. The type checker must *name* built-in type
//! constructors that never appear in user source — notably `Task` (the result
//! of `println`). Minting their [`Symbol`]s requires interning, exactly as the
//! sibling pipeline stages (`parse_module`, `canonicalise`) already take
//! `&mut Interner`. The freshly-interned names flow downstream so the lowerer
//! (which keeps `&Interner`) can resolve them.

mod constrain;
mod solve;
mod ty;
mod unify;
mod unionfind;

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Span};
use sky_intern::{Interner, Symbol};

pub use solve::{BUDGET_ENV, Budget, DEFAULT_SOLVER_BUDGET};
pub use ty::Ty;

use constrain::{Builder, zonk};
use solve::solve;
use unionfind::UnionFind;

/// The result of inference: resolved types for bindings and for every region.
///
/// Mirrors the Haskell `SolvedTypes` record's `_stEnv` + `_stRegions`. Both
/// maps are `BTreeMap`s so iteration is deterministic.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SolvedTypes {
    /// Type of each top-level binding, keyed by its name symbol.
    pub env: BTreeMap<Symbol, Ty>,
    /// Type of each sub-expression source region, keyed by its [`Span`]. Drives
    /// type-directed lowering.
    pub regions: BTreeMap<Span, Ty>,
}

/// Infer the types of a canonical module.
///
/// # Errors
/// * [`sky_diagnostics::Diagnostic::Type`] with [`sky_diagnostics::TypeError::Mismatch`]
///   when two types fail to unify, or [`sky_diagnostics::TypeError::BudgetExceeded`]
///   when the solver step budget is exhausted.
/// * [`sky_diagnostics::Diagnostic::CompilerBug`] on a violated internal
///   invariant (dangling union-find id, unbound local, arity mismatch — all
///   unreachable for well-canonicalised input).
pub fn infer(m: &canon::Module, interner: &mut Interner) -> DResult<SolvedTypes> {
    let mut budget = Budget::from_env();
    infer_with_budget(m, interner, &mut budget)
}

/// Inference with an explicit solver budget. Exposed for tests that need to
/// drive the [`sky_diagnostics::TypeError::BudgetExceeded`] path deterministically
/// without mutating process-global environment state.
fn infer_with_budget(
    m: &canon::Module,
    interner: &mut Interner,
    budget: &mut Budget,
) -> DResult<SolvedTypes> {
    let mut uf = UnionFind::new();
    let generated = Builder::run(&mut uf, interner, m)?;

    solve(&mut uf, budget, &generated.constraints)?;

    // Read back every region's resolved type.
    let mut regions = BTreeMap::new();
    for (span, var) in generated.regions {
        regions.insert(span, zonk(&mut uf, var)?);
    }

    // `env` = annotation types of typed bindings (exact) + read-back of every
    // untyped binding's inferred body type.
    let mut env = generated.top_level;
    for (name, var) in generated.untyped {
        env.insert(name, zonk(&mut uf, var)?);
    }

    Ok(SolvedTypes { env, regions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_diagnostics::{Diagnostic, TypeError};

    const GOLDEN: &str = include_str!("../../../tests/golden/m0/Main.sky");

    /// Parse + canonicalise the golden module, returning it plus the interner.
    fn canon_golden() -> Option<(canon::Module, Interner)> {
        let mut i = Interner::new();
        let src = sky_parse::parse_module(GOLDEN, &mut i).ok()?;
        let m = sky_canon::canonicalise(&src, &mut i).ok()?;
        Some((m, i))
    }

    fn sym(i: &Interner, m: &canon::Module, name: &str) -> Option<Symbol> {
        // Resolve a name to its symbol by scanning the def names / unions.
        for d in &m.defs {
            if i.resolve(d.name().value) == name {
                return Some(d.name().value);
            }
        }
        for u in &m.unions {
            if i.resolve(u.name) == name {
                return Some(u.name);
            }
        }
        None
    }

    /// Drill into a `Call` node.
    fn as_call(e: &canon::Expr) -> Option<(&canon::Expr, &[canon::Expr])> {
        match &e.value {
            canon::Expr_::Call(callee, args) => Some((callee, args)),
            _ => None,
        }
    }

    fn ty_con_name(ty: &Ty, i: &Interner) -> Option<String> {
        match ty {
            Ty::Con { name, .. } => Some(i.resolve(*name).to_owned()),
            _ => None,
        }
    }

    #[test]
    fn env_update_is_msg_to_int_to_int() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden must parse + canonicalise");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };

        let Some(update) = sym(&i, &m, "update") else {
            return;
        };
        let Some(ty) = solved.env.get(&update) else {
            return;
        };

        // Msg -> (Int -> Int)
        assert!(matches!(ty, Ty::Fun(..)), "update is an arrow");
        let Ty::Fun(msg_arg, tail) = ty else { return };
        assert_eq!(ty_con_name(msg_arg, &i).as_deref(), Some("Msg"));
        assert!(matches!(tail.as_ref(), Ty::Fun(..)), "tail is an arrow");
        let Ty::Fun(int_arg, ret) = tail.as_ref() else {
            return;
        };
        assert_eq!(ty_con_name(int_arg, &i).as_deref(), Some("Int"));
        assert_eq!(ty_con_name(ret, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn regions_carry_call_and_kernel_types() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };

        // main = println (String.fromInt (update Increment 0))
        let main_def = m.defs.iter().find(|d| i.resolve(d.name().value) == "main");
        assert!(
            matches!(main_def, Some(canon::Def::Untyped { .. })),
            "main is untyped"
        );
        let Some(canon::Def::Untyped { body, .. }) = main_def else {
            return;
        };

        // Outer call: println … : Task ()
        let outer = as_call(body);
        assert!(outer.is_some(), "main body is a call");
        let Some((_println, outer_args)) = outer else {
            return;
        };
        let println_region = solved.regions.get(&body.span);
        assert!(
            matches!(
                println_region,
                Some(Ty::Con { name, args, .. })
                    if i.resolve(*name) == "Task" && args.as_slice() == [Ty::Unit]
            ),
            "println region must be Task (): {println_region:?}"
        );

        // String.fromInt … : String
        let Some(from_int_call) = outer_args.first() else {
            return;
        };
        let mid = as_call(from_int_call);
        assert!(mid.is_some(), "fromInt call");
        let Some((_from_int, mid_args)) = mid else {
            return;
        };
        assert_eq!(
            solved
                .regions
                .get(&from_int_call.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("String")
        );

        // update Increment 0 : Int
        let Some(update_call) = mid_args.first() else {
            return;
        };
        assert!(as_call(update_call).is_some(), "update call");
        assert_eq!(
            solved
                .regions
                .get(&update_call.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Int")
        );
    }

    #[test]
    fn regions_carry_scrutinee_and_binop_types() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };

        let update_def = m
            .defs
            .iter()
            .find(|d| i.resolve(d.name().value) == "update");
        assert!(
            matches!(update_def, Some(canon::Def::Typed { .. })),
            "update is typed"
        );
        let Some(canon::Def::Typed { body, .. }) = update_def else {
            return;
        };
        assert!(
            matches!(&body.value, canon::Expr_::Case(..)),
            "update body is case"
        );
        let canon::Expr_::Case(scrut, branches) = &body.value else {
            return;
        };

        // Scrutinee `msg` : Msg
        assert_eq!(
            solved
                .regions
                .get(&scrut.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Msg")
        );

        // First arm body `count + 1` : Int
        let Some(first) = branches.first() else {
            return;
        };
        assert!(
            matches!(first.body.value, canon::Expr_::Binop { .. }),
            "arm body is binop"
        );
        assert_eq!(
            solved
                .regions
                .get(&first.body.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Int")
        );
    }

    #[test]
    fn env_main_is_task_unit() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };
        let Some(main) = sym(&i, &m, "main") else {
            return;
        };
        let main_ty = solved.env.get(&main);
        assert!(
            matches!(
                main_ty,
                Some(Ty::Con { name, args, .. })
                    if i.resolve(*name) == "Task" && args.as_slice() == [Ty::Unit]
            ),
            "env[main] must be Task (): {main_ty:?}"
        );
    }

    #[test]
    fn exhausted_budget_yields_budget_exceeded() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        // A budget of one step cannot discharge the golden program's
        // constraints; the very first unify trips the bound.
        let mut budget = Budget::new(1);
        let r = infer_with_budget(&m, &mut i, &mut budget);
        assert!(matches!(
            r,
            Err(Diagnostic::Type {
                msg: TypeError::BudgetExceeded,
                ..
            })
        ));
    }

    #[test]
    fn disabled_budget_still_succeeds() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let mut budget = Budget::unbounded();
        assert!(infer_with_budget(&m, &mut i, &mut budget).is_ok());
    }
}
