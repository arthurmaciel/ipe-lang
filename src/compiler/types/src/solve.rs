//! The constraint solver, ported from the relevant core of
//! `Ipe.Type.Solve` (derivative of elm/compiler's `Type.Solve`,
//! BSD-3-Clause).
//!
//! The reference solver threads ranks / marks / generalisation for full
//! let-polymorphism. The supported subset has no nested `let`-generalisation
//! to model, so solving reduces to: run every generated equality
//! [`Constraint`] through
//! the unifier, in order, sharing one [`Budget`]. The defensive solver-step
//! bound (`IPE_SOLVER_BUDGET`) carries over verbatim in spirit.

use ipe_diagnostics::{DResult, Diagnostic, Span, TypeError};
use ipe_intern::{Interner, Symbol};

use crate::ty::Content;
use crate::unify::unify;
use crate::unionfind::{UnionFind, VarId};

/// Default cap on solver steps before bailing — matches the reference compiler
/// `defaultSolverBudget`. Ordinary programs consume well under a thousand
/// steps; the cap exists purely to bound adversarial blow-up.
pub const DEFAULT_SOLVER_BUDGET: u64 = 5_000_000;

/// Environment variable that overrides [`DEFAULT_SOLVER_BUDGET`].
pub const BUDGET_ENV: &str = "IPE_SOLVER_BUDGET";

/// A single constraint: the types of two solver variables must unify. The
/// [`Span`] is the source region blamed in any resulting [`TypeError`].
/// `home` is the module path that owns the expression emitting this constraint
/// — populated by [`constrain::Builder`] from the currently-processed def's
/// `home` field.  It is used by the compiler driver's error-attribution path to
/// select the correct source file for a cross-module type error without relying
/// on the byte-offset heuristic that can fail when two modules share the same
/// numeric span range.
#[derive(Clone, Debug)]
pub struct Constraint {
    pub span: Span,
    pub lhs: VarId,
    pub rhs: VarId,
    /// The module that owns the constrained expression.  `Vec::new()` for
    /// compiler-synthesised constraints (e.g. built-in operator types) that
    /// have no meaningful source location.
    pub home: Vec<Symbol>,
}

/// A decrementing solver-step budget.
///
/// Each unify/occurs/zonk step ticks it; reaching zero raises
/// [`TypeError::StepBudgetExceeded`] carrying the original `limit` (so the help
/// line can name the value to raise). A budget of `None` is disabled (the
/// `IPE_SOLVER_BUDGET=0` escape hatch).
pub struct Budget {
    remaining: Option<u64>,
    /// The step cap this budget was created with, reported in the diagnostic.
    /// `0` for an unbounded budget (never surfaced — it cannot be exhausted).
    limit: u64,
}

impl Budget {
    /// A budget with an explicit step cap.
    #[must_use]
    pub const fn new(steps: u64) -> Self {
        Self {
            remaining: Some(steps),
            limit: steps,
        }
    }

    /// A disabled (unbounded) budget.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self {
            remaining: None,
            limit: 0,
        }
    }

    /// Resolve the budget from the environment, mirroring the reference compiler
    /// three-mode resolution (unset → default; `0` → disabled; `N` → absolute).
    /// A malformed value falls back to the default rather than failing the
    /// build — the budget is a guard rail, not a feature.
    #[must_use]
    pub fn from_env() -> Self {
        std::env::var(BUDGET_ENV).map_or_else(
            |_| Self::new(DEFAULT_SOLVER_BUDGET),
            |raw| match raw.trim().parse::<u64>() {
                Ok(0) => Self::unbounded(),
                Ok(n) => Self::new(n),
                Err(_) => Self::new(DEFAULT_SOLVER_BUDGET),
            },
        )
    }

    /// Consume one step.
    ///
    /// # Errors
    /// [`TypeError::StepBudgetExceeded`] when the budget is exhausted, carrying
    /// the configured `limit`.
    pub const fn tick(&mut self) -> DResult<()> {
        if let Some(remaining) = self.remaining.as_mut() {
            match remaining.checked_sub(1) {
                Some(next) => *remaining = next,
                None => {
                    return Err(Diagnostic::Type {
                        span: Span::DUMMY,
                        msg: TypeError::StepBudgetExceeded { budget: self.limit },
                    });
                }
            }
        }
        Ok(())
    }
}

/// Solve every constraint in order, unifying in place.
///
/// This is the plain, non-attributed variant. Prefer [`solve_attributed`] when
/// the caller needs to know which module owns the failing constraint (e.g. for
/// cross-module error attribution).
///
/// # Errors
/// Propagates any [`Diagnostic`] from unification (mismatch, budget, or a
/// union-find invariant violation).
#[allow(dead_code)] // used via solve_attributed; kept as a convenience entry point
pub fn solve(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    constraints: &[Constraint],
) -> DResult<()> {
    solve_attributed(uf, budget, interner, constraints).map_err(|(diag, _home)| diag)
}

/// Like [`solve`] but on failure also returns the `home` module path carried by
/// the failing constraint.  The home path lets the compiler driver's
/// error-attribution path select the correct source file without relying on the
/// byte-offset heuristic (`source_for_span`).
///
/// Returns `Ok(())` on success, `Err((diag, home))` on the first failing
/// constraint.
///
/// # Errors
/// Same error conditions as [`solve`].
pub fn solve_attributed(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    constraints: &[Constraint],
) -> Result<(), (Diagnostic, Vec<Symbol>)> {
    for c in constraints {
        unify(uf, budget, interner, c.span, c.lhs, c.rhs).map_err(|diag| (diag, c.home.clone()))?;
    }
    Ok(())
}
