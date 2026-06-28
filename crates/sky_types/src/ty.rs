//! The two type levels of inference, ported from the M0 subset of
//! `Sky.Type.Type` (derivative of elm/compiler's `Type.Type`, BSD-3-Clause).
//!
//! * [`Ty`] — the *resolved*, immutable, canonical type read back after the
//!   solver settles. This is what [`crate::SolvedTypes`] hands to the lowerer.
//! * [`Content`] / [`FlatType`] — the *solver-level* descriptors stored inside
//!   union-find nodes during inference (mirrors Haskell `Content` / `FlatType`,
//!   narrowed to the M0 lattice: functions, type-constructor applications, and
//!   unit; no records / tuples / aliases / super-types yet).

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_intern::Symbol;

use crate::unionfind::VarId;

/// A resolved type (post-solve, immutable). M0 subset.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ty {
    /// An unresolved type variable that survived solving (e.g. an unused
    /// polymorphic kernel argument). Carries the representative's arena id.
    Var(u32),
    /// A function `arg -> result`.
    Fun(Box<Self>, Box<Self>),
    /// A type-constructor application. `module` is the defining module (empty
    /// for built-ins like `Int` / `String` that have no user home); `name` is
    /// the interned type name; `args` its type arguments.
    Con {
        module: Vec<Symbol>,
        name: Symbol,
        args: Vec<Self>,
    },
    /// The unit type `()`.
    Unit,
    /// An anonymous product (tuple) type `(T1, T2, ...)`. Invariant: arity ≥ 2 —
    /// a 0-tuple is [`Ty::Unit`] and a 1-tuple is just its element.
    Tuple(Vec<Self>),
    /// A closed record type `{ x : Int, y : Bool }`, keyed by field name. The
    /// [`BTreeMap`] fixes iteration order (by [`Symbol`]); consumers that need a
    /// source-like order re-sort by the resolved field name.
    Record(BTreeMap<Symbol, Self>),
}

/// What a union-find variable resolves to during inference.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Content {
    /// An unconstrained (flexible) inference variable.
    Flex,
    /// A *rigid* (skolem) variable minted from a user annotation's type variable
    /// while checking that binding's body. Mirrors Haskell `Content.RigidVar`.
    ///
    /// A rigid variable may unify with a flexible one (the flex adopts it), but
    /// never with a concrete [`FlatType`] nor with a *different* rigid — so a body
    /// cannot force an annotated `a` to a concrete shape (`f : a -> a; f x = x + 1`
    /// is a mismatch, not silently accepted) nor collapse two distinct annotated
    /// variables (`f : a -> b; f x = x` is a mismatch). Distinctness is by
    /// union-find identity: two occurrences of the *same* annotation variable
    /// share one rigid node (via the per-signature instantiation map), so they are
    /// `equivalent`; two different variables are separate nodes that cannot unify.
    Rigid,
    /// A resolved concrete structure.
    Structure(FlatType),
}

/// A concrete type structure whose children are union-find variables (so
/// unification can refine them in place). Mirrors Haskell `FlatType`, M0 subset.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FlatType {
    /// Function `arg -> result`.
    Fun(VarId, VarId),
    /// Type-constructor application over variable arguments.
    Con {
        module: Vec<Symbol>,
        name: Symbol,
        args: Vec<VarId>,
    },
    /// Unit `()`.
    Unit,
    /// Anonymous product (tuple) over variable elements. Invariant: arity ≥ 2.
    Tuple(Vec<VarId>),
    /// Closed record `{ name : var, ... }`, keyed by field name; each value is
    /// the union-find variable of that field's type, refined in place by
    /// unification.
    Record(BTreeMap<Symbol, VarId>),
}

/// Convert a canonical annotation type into a resolved [`Ty`].
///
/// Used to seed `env[update]` directly from its written signature and to
/// instantiate annotated parameter/return types into the solver. A type
/// variable's interned [`Symbol`] is carried through via its raw id so distinct
/// variables stay distinct.
#[must_use]
pub fn from_canon(t: &canon::Type) -> Ty {
    match t {
        canon::Type::Lambda(a, b) => Ty::Fun(Box::new(from_canon(a)), Box::new(from_canon(b))),
        canon::Type::Var(s) => Ty::Var(s.as_raw()),
        canon::Type::Con { home, name, args } => Ty::Con {
            module: home.clone(),
            name: *name,
            args: args.iter().map(from_canon).collect(),
        },
    }
}
