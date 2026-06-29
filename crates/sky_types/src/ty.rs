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

/// The super-type obligations a type variable carries: the operations a body
/// performs on it that only *some* types support.
///
/// A bare type variable is structurally parametric — the body passes it through
/// untouched, so any type works. The moment a body adds it (`x + x`), orders
/// it (`a > b`), or compares it for equality (`a == b`), the variable is no
/// longer "any type": it must be a type that supports that operation. Sky's
/// relevant super-types are **Number** (the numeric operators `+ - *`, satisfied
/// by `Int` / `Float`), **Comparable** (the ordering comparisons `< > <= >=`,
/// satisfied by the scalar primitives), and **Equatable** (the equality
/// comparisons `== /=`, satisfied by every non-function type — Rust's
/// `PartialEq` is derivable for primitives, tuples, records, and enums but never
/// for a function).
///
/// Each obligation is one bit, so two variables that unify merge their
/// obligations with a bitwise OR ([`Self::union`]) — the merged variable owes
/// everything either side owed. The set is read back when generalising: a
/// variable that owes `Add` becomes a generic parameter bounded by Rust's
/// `Add` trait, and so on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TyBounds(u8);

impl TyBounds {
    const ADD: u8 = 1 << 0;
    const SUB: u8 = 1 << 1;
    const MUL: u8 = 1 << 2;
    const ORD: u8 = 1 << 3;
    const EQ: u8 = 1 << 4;
    const SET_ELEM: u8 = 1 << 5;
    const DICT_KEY: u8 = 1 << 6;

    /// No obligation — a structurally-parametric variable.
    pub const EMPTY: Self = Self(0);

    /// The single-obligation sets, one per operator family.
    #[must_use]
    pub const fn add() -> Self {
        Self(Self::ADD)
    }
    #[must_use]
    pub const fn sub() -> Self {
        Self(Self::SUB)
    }
    #[must_use]
    pub const fn mul() -> Self {
        Self(Self::MUL)
    }
    /// The ordering obligation (`< > <= >=` → Rust `PartialOrd`).
    #[must_use]
    pub const fn ord() -> Self {
        Self(Self::ORD)
    }
    /// The equality obligation (`== /=` → Rust `PartialEq`).
    #[must_use]
    pub const fn eq() -> Self {
        Self(Self::EQ)
    }
    /// The Set-element obligation: this variable is used as a `Set` element, so
    /// it must be a Sky `comparable` whose Rust backing — `BTreeSet<A>` —
    /// requires `A : Ord`. Distinct from [`Self::ord`] (which renders Rust
    /// `PartialOrd`, insufficient for `BTreeSet`'s key requirement): a generic
    /// `a -> Set a` must lift `Ord` onto its emitted type parameter.
    #[must_use]
    pub const fn set_elem() -> Self {
        Self(Self::SET_ELEM)
    }
    /// The Dict-key obligation: this variable is used as a `Dict` key, so it
    /// must be a Sky `comparable` whose Rust backing — `HashMap<K, V>` with
    /// determinism-sorted key iteration — requires `K : Hash + Eq + Ord`.
    /// Distinct from [`Self::ord`] / [`Self::eq`] (which render `PartialOrd` /
    /// `PartialEq`, neither of which satisfies `HashMap`'s `Hash + Eq` nor the
    /// sorted-iteration `Ord`): a generic `a -> Dict a v` must lift the full
    /// trait set onto its emitted type parameter.
    #[must_use]
    pub const fn dict_key() -> Self {
        Self(Self::DICT_KEY)
    }

    /// Whether this set carries no obligation at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
    #[must_use]
    pub const fn has_add(self) -> bool {
        self.0 & Self::ADD != 0
    }
    #[must_use]
    pub const fn has_sub(self) -> bool {
        self.0 & Self::SUB != 0
    }
    #[must_use]
    pub const fn has_mul(self) -> bool {
        self.0 & Self::MUL != 0
    }
    #[must_use]
    pub const fn has_ord(self) -> bool {
        self.0 & Self::ORD != 0
    }
    #[must_use]
    pub const fn has_eq(self) -> bool {
        self.0 & Self::EQ != 0
    }
    /// Whether the Set-element obligation is set.
    #[must_use]
    pub const fn has_set_elem(self) -> bool {
        self.0 & Self::SET_ELEM != 0
    }
    /// Whether the Dict-key obligation is set.
    #[must_use]
    pub const fn has_dict_key(self) -> bool {
        self.0 & Self::DICT_KEY != 0
    }
    /// Whether this variable carries a Sky `comparable`-key obligation — used as
    /// a `Set` element or a `Dict` key. Both are satisfied by exactly the Sky
    /// `comparable` scalar primitives at type-check; the per-container Rust
    /// trait differences (`Ord` vs `Hash + Eq + Ord`) surface only at emission.
    #[must_use]
    pub const fn has_comparable_key(self) -> bool {
        self.0 & (Self::SET_ELEM | Self::DICT_KEY) != 0
    }

    /// Whether any numeric operator (`+ - *`) constrains this variable — i.e. it
    /// must be a `Number` (`Int` / `Float`).
    #[must_use]
    pub const fn has_number(self) -> bool {
        self.0 & (Self::ADD | Self::SUB | Self::MUL) != 0
    }

    /// The union of two obligation sets — what a merged variable owes.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
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
    /// A variable constrained to a Sky super-type ([`TyBounds`]) but not yet
    /// pinned to a concrete structure. `rigid` distinguishes a super-typed
    /// *annotation skolem* (it must stay generic, surfacing its obligations as
    /// trait bounds on the emitted type parameter) from a super-typed *flex* (an
    /// inferred variable that may still pin to a matching concrete type).
    ///
    /// A super-typed flex pins to any structure that satisfies its obligations
    /// (`Int` / `Float` for Number; the scalar primitives for Comparable; any
    /// non-function type for Equatable); a
    /// super-typed rigid against a concrete structure is a mismatch, exactly as a
    /// plain rigid is — the annotation promised a generic the body is now trying
    /// to pin down.
    Super { rigid: bool, bounds: TyBounds },
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
        canon::Type::Unit => Ty::Unit,
        canon::Type::Tuple(elems) => Ty::Tuple(elems.iter().map(from_canon).collect()),
        // A closed record annotation `{ field : T, ... }`. Keyed by field name in
        // the [`BTreeMap`] (fixing iteration order); a field type variable carries
        // through via its raw id exactly as a top-level [`canon::Type::Var`] does,
        // so an annotated record field var becomes a rigid skolem when the
        // signature is instantiated to check the body.
        canon::Type::Record(fields) => Ty::Record(
            fields
                .iter()
                .map(|(name, fty)| (*name, from_canon(fty)))
                .collect(),
        ),
    }
}
