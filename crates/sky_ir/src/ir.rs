//! The typed IR node definitions (M0 subset). Widened in later milestones; for
//! M0 the surface is deliberately narrow so that every constructible value is a
//! well-formed program fragment.

use std::collections::{BTreeMap, BTreeSet};

use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Symbol;

/// A dotted module path, e.g. `Main` or `Sky.Core.Io`, as interned segments in
/// source order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModPath(pub Vec<Symbol>);

/// A function identifier, unique within a [`Program`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FuncId(pub u32);

impl FuncId {
    #[must_use]
    pub const fn from_raw(n: u32) -> Self {
        Self(n)
    }

    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

/// A whole compiled program: an ordered list of modules.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Program {
    pub modules: Vec<Module>,
}

/// A single module: its declared types and functions, plus an optional entry
/// point (the `main` function, when this module carries it).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Module {
    pub name: ModPath,
    pub types: Vec<TypeDef>,
    pub funcs: Vec<Func>,
    pub entry: Option<FuncId>,
    /// Every CLOSED record shape the module's expressions construct or read,
    /// each an [`IrType::Record`]. The lowerer surfaces these (it alone has the
    /// solved types) so the backend can synthesise one Rust struct per shape —
    /// record literals live inside function bodies, where the type does not
    /// otherwise appear in a signature. Non-record entries are ignored by the
    /// backend, so the field stays robust to a stray shape.
    pub records: Vec<IrType>,
}

/// A user-declared type. The IR models user types as enums (Sky's `type`
/// declarations); a nullary-only enum is the M0 case, a payload-carrying and/or
/// generic enum the M3a case.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeDef {
    Enum(EnumDef),
}

/// An enum (algebraic data type) declaration.
///
/// A variant may carry payload fields (M3a) and the type may be generic over a
/// list of type parameters (`type Maybe a = Just a | Nothing`). A nullary-only,
/// non-generic enum (`type Msg = Increment | Decrement`) has every variant's
/// `fields` empty and an empty `type_params` — that path stays byte-identical to
/// the M0 backend output.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnumDef {
    pub name: Symbol,
    /// The type variables this enum quantifies, in declaration order. Each is a
    /// Sky type-variable [`Symbol`] that appears as an [`IrType::Generic`] in a
    /// variant's field types. A non-generic enum has an empty list.
    ///
    /// The order is load-bearing: the backend derives each parameter's Rust
    /// generic name (`T1`, `T2`, …) from its *position* here — exactly as for
    /// [`Func::type_params`] — so the emitted `enum Name<T1, T2>` agrees with
    /// every field type and use-site instantiation regardless of source naming.
    pub type_params: Vec<Symbol>,
    pub variants: Vec<Variant>,
}

/// One constructor of an [`EnumDef`]: its name and its ordered payload field
/// types.
///
/// A nullary constructor (`Increment`, `Nothing`) has an empty `fields`. A
/// payload constructor (`Just a`, `Rect Float Float`, `Node Tree Int Tree`)
/// lists one [`IrType`] per positional field, in source order. A field whose
/// type is the enum being declared (direct self-recursion) is rendered boxed by
/// the backend so the Rust enum stays finite-sized; the IR carries the bare
/// recursive type and leaves the boxing to emission.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Variant {
    pub name: Symbol,
    pub fields: Vec<IrType>,
}

/// A function: the type variables it quantifies, typed parameters, a return
/// type, and a body expression.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Func {
    pub id: FuncId,
    pub name: Symbol,
    /// The type variables this function quantifies, in quantification order
    /// (M2a). Each is a Sky type-variable [`Symbol`] that appears as an
    /// [`IrType::Generic`] in the parameters / return / body. A monomorphic
    /// function has an empty list, so existing M0 / M1 functions are unchanged.
    ///
    /// The order is load-bearing: the backend derives each variable's Rust
    /// generic name (`T1`, `T2`, …) from its *position* here, so a function
    /// quantifying `[a, b]` emits `fn name<T1, T2>(..)` with `a` → `T1` and
    /// `b` → `T2` regardless of the source variable spellings.
    pub type_params: Vec<Symbol>,
    pub params: Vec<(Symbol, IrType)>,
    pub ret: IrType,
    pub body: Expr,
}

/// The M0 type lattice. Widened in later milestones.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IrType {
    Int,
    Float,
    Bool,
    Str,
    Unit,
    TaskUnit,
    /// A user-declared enum type, applied to its type arguments.
    ///
    /// `name` is the enum's bare type [`Symbol`]; `args` are the concrete type
    /// arguments at a use site (`Maybe Int` → `args = [Int]`, rendered
    /// `MainMaybe<i64>`). A non-generic enum (`Msg`) carries an empty `args`
    /// list, so it renders as the bare Rust type name — byte-identical to the M0
    /// backend. An `arg` may itself be an [`IrType::Generic`] when a generic
    /// enum is passed through a generic function (`Maybe a` inside a parametric
    /// signature → `MainMaybe<T1>`).
    Enum {
        name: Symbol,
        args: Vec<Self>,
    },
    /// An anonymous product type `(T1, T2, ...)`.
    ///
    /// Invariant: the element list has arity ≥ 2. A 0-tuple is [`IrType::Unit`]
    /// and a 1-tuple is just its element type — neither is a `Tuple`. The
    /// lowerer is the sole producer and upholds this; the backend stays total
    /// over any vector it receives (it never panics on a degenerate arity).
    Tuple(Vec<Self>),
    /// A CLOSED record type `{ x : Int, y : Bool, ... }` — an exact, known field
    /// set keyed by field name.
    ///
    /// The field map is a [`BTreeMap`], so its iteration order is fixed (by
    /// [`Symbol`]). The backend re-canonicalises by *field name* before it
    /// derives a struct name or emits the struct body, so the synthesised Rust
    /// struct is deterministic regardless of interning order.
    ///
    /// Open / row-polymorphic records (`{ r | x : Int }`) are intentionally NOT
    /// representable here — they are deferred to M2 and rejected at lowering, so
    /// every `Record` the backend sees is closed.
    Record(BTreeMap<Symbol, Self>),
    /// A function type `T0 -> T1 -> ... -> R`, carried as its parameter list and
    /// return type (`params -> ret`).
    ///
    /// This is the type of a first-class function value — a lambda, a
    /// function-typed parameter or binding, or a top-level function used as a
    /// value. The backend renders it as a boxed trait object
    /// `Box<dyn Fn(T0, ...) -> R>`.
    ///
    /// Invariant: a zero-parameter function type (`params` empty) is a genuine
    /// nullary `Fn() -> R`, distinct from `ret` alone. The lowerer is the sole
    /// producer; the backend stays total over any parameter vector it receives.
    Fun(Vec<Self>, Box<Self>),
    /// A generic type parameter — a Sky type variable used STRUCTURALLY
    /// (pass-through, no operation applied to it) in a fully-parametric
    /// top-level function (M2a). The carried [`Symbol`] is the source type
    /// variable's name (e.g. interned `"a"`).
    ///
    /// The backend renders this as the function's corresponding Rust generic
    /// (`T1`, `T2`, …), resolved by the variable's position in the enclosing
    /// [`Func::type_params`] — not by the symbol's spelling — so emission is
    /// deterministic regardless of source naming.
    ///
    /// A `Generic` is only ever in scope inside a function that quantifies it;
    /// it never appears in a program-level position (enum / record-struct
    /// declaration). Constrained type variables (those needing a Rust trait
    /// bound — `Number` / `Comparable` / `Appendable`) and the wildcard `any`
    /// are NOT representable here: they are rejected at lowering (M2c) so every
    /// `Generic` the backend sees is a true parametric pass-through.
    Generic(Symbol),
}

/// An expression in the typed IR.
///
/// Note: the [`Match`] variant wraps the opaque [`Match`] type rather than
/// inlining `scrutinee` / `arms` fields. That is deliberate — it keeps the
/// exhaustiveness invariant unbreakable, because the only constructor for a
/// [`Match`] is [`Match::new`], which validates the arm set. An inline
/// struct-variant with public fields could be built directly, bypassing the
/// check, and would make illegal IR representable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Expr {
    Int(i64),
    /// The unit value `()` — the sole inhabitant of [`IrType::Unit`].
    ///
    /// Sky's `()` literal lowers here; the backend emits the Rust unit
    /// expression `()`. Distinct from a zero-element [`Expr::Tuple`], which the
    /// tuple invariant forbids (arity ≥ 2): the empty product is this `Unit`.
    Unit,
    Var(Symbol),
    /// A constructor application `Variant arg0 arg1 …` (a nullary constructor
    /// `Variant` has an empty `args`).
    ///
    /// `ty` is the constructor's enum type [`Symbol`]; `variant` the constructor
    /// name. `args` are the payload expressions, one per declared field, in
    /// source order. The backend resolves the variant's declared field types
    /// from the enum declaration to wrap any direct-self-recursive field in
    /// `Box::new` at construction (matching the boxed enum field).
    Ctor {
        ty: Symbol,
        variant: Symbol,
        args: Vec<Self>,
    },
    BinOp {
        op: BinOp,
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    /// A non-recursive single-binding `let name = value in body`. Multi-binding
    /// `let` lowers to nested `Let`s; `name` is bound only within `body`, not in
    /// `value`.
    Let {
        name: Symbol,
        value: Box<Self>,
        body: Box<Self>,
    },
    /// An irrefutable destructuring binding `let <binder> = value in body`.
    ///
    /// The sibling of [`Self::Let`] for the pattern-binder case: where `Let`
    /// binds a single [`Symbol`] (the audited common fast path), `Destructure`
    /// binds an IRREFUTABLE [`Pat`] — a [`Pat::Tuple`] of variables / wildcards
    /// (recursively), or a bare [`Pat::Var`] / [`Pat::Wildcard`]. It is the IR
    /// shape M3b-1 lowers a tuple-destructuring `case` arm and a tuple function
    /// parameter to (`fst (a, b) = a` → a synthetic param plus
    /// `Destructure { (a, b) = arg } a`). The binder must be irrefutable — the
    /// lowerer is the sole producer and rejects a refutable element
    /// (a constructor / literal) fail-closed (SKY-L0115) — so the backend's
    /// `let <binder> = <value>;` is a sound, exhaustive Rust binding. `binder`
    /// is bound only within `body`, not in `value`.
    Destructure {
        binder: Pat,
        value: Box<Self>,
        body: Box<Self>,
    },
    /// A conditional `if cond then then_ else else_`. The `else` arm is
    /// mandatory — every Sky `if` is an expression with both branches.
    If {
        cond: Box<Self>,
        then_: Box<Self>,
        else_: Box<Self>,
    },
    Match(Match),
    Call {
        callee: Callee,
        args: Vec<Self>,
    },
    /// A tuple constructor `(e1, e2, ...)`.
    ///
    /// Invariant: the element list has arity ≥ 2 — a 0-tuple is the unit value
    /// and a 1-tuple is just its element, so neither is a `Tuple`. The lowerer
    /// upholds this; the backend remains total over any vector (it never panics
    /// on a degenerate arity).
    Tuple(Vec<Self>),
    /// A record literal `{ x = e1, y = e2, ... }`.
    ///
    /// The fields are carried as `(field name, value)` pairs sorted by field
    /// name, so the construction is deterministic. The backend resolves the
    /// literal's synthesised Rust struct from its field-name set; Rust names its
    /// struct-literal fields, so the emitted construction is order-independent.
    Record(Vec<(Symbol, Self)>),
    /// A record field access `record.field`.
    Access {
        record: Box<Self>,
        field: Symbol,
    },
    /// A record update `{ record | x = e1, ... }`: a copy of `record` with the
    /// listed fields replaced. `fields` lists only the changed fields, as
    /// `(field name, new value)` pairs.
    Update {
        record: Box<Self>,
        fields: Vec<(Symbol, Self)>,
    },
    /// An anonymous function `\p0 p1 ... -> body`: typed parameters, a return
    /// type, and a body expression.
    ///
    /// Distinct from [`Func`] (a named top-level declaration): a `Lambda` is an
    /// expression value. The backend emits it as a boxed closure
    /// `Box::new(move |p0: T0, ...| -> R { body })`, move-capturing any free
    /// locals. A zero-parameter lambda is a genuine nullary closure.
    Lambda {
        params: Vec<(Symbol, IrType)>,
        ret: IrType,
        body: Box<Self>,
    },
    /// Application of an arbitrary expression value to arguments, `func(args)`.
    ///
    /// Distinct from [`Expr::Call`], which targets a known [`Callee`] (a direct
    /// top-level function or a kernel) and keeps the efficient direct-call path.
    /// `Apply` calls a first-class function *value* — a lambda, a
    /// function-typed parameter/binding, or a top-level function passed as a
    /// value — and renders as `(func)(args)` (a boxed `dyn Fn` auto-derefs).
    Apply {
        func: Box<Self>,
        args: Vec<Self>,
    },
    /// A top-level function or kernel named as a first-class *value* — passed as
    /// an argument, returned, or let-bound — rather than directly called.
    ///
    /// Distinct from [`Expr::Call`] (which applies a known [`Callee`] to
    /// arguments on the spot): `FuncValue` reifies the callee into a boxed
    /// closure value so it fills a `Box<dyn Fn(..) -> R>` slot uniformly. The
    /// backend emits `{ let f: <ty> = Box::new(<callee>); f }`, the explicit
    /// binding type pinning the unsized coercion of the top-level `fn` item (a
    /// zero-sized `Fn` implementor) to the boxed trait object. `ty` is the
    /// value's flattened [`IrType::Fun`], recorded by the lowerer from the
    /// reference's solved region type. A direct call keeps the efficient
    /// [`Expr::Call`] path; only a bare value reference becomes a `FuncValue`.
    FuncValue {
        callee: Callee,
        ty: IrType,
    },
}

/// The target of a [`Expr::Call`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Callee {
    Func(FuncId),
    Kernel(KernelFn),
}

/// Built-in kernel functions reachable from M0 programs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelFn {
    StringFromInt,
    LogPrintln,
}

/// Binary operators.
///
/// M0 shipped `Add`/`Sub`; M1 core widens the set with the remaining
/// arithmetic, comparison, and boolean operators. List/string operators
/// (`++`, `::`) are deferred until those types land.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

/// One arm of a [`Match`]: a constructor pattern and the body it guards.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Arm {
    pub pat: Pat,
    pub body: Expr,
}

/// A pattern.
///
/// M3a supports a constructor pattern whose payload sub-patterns bind to a
/// variable ([`Pat::Var`]) or are ignored ([`Pat::Wildcard`]). Nullary
/// constructor patterns (M0) are [`Pat::Ctor`] with an empty `args`. M3b-1 adds
/// the tuple pattern [`Pat::Tuple`], whose elements reuse the existing pattern
/// variants (var / wildcard / nested ctor / nested tuple).
///
/// [`Pat::Var`] / [`Pat::Wildcard`] / [`Pat::Tuple`] appear as the payload
/// sub-patterns of a [`Pat::Ctor`] and, for [`Pat::Tuple`], also as a tuple-
/// destructuring binder (a single irrefutable case arm or a function parameter).
/// Literal / record / cons / alias patterns remain M3b+ and are rejected
/// upstream at lowering, so every [`Pat`] the backend sees here is
/// var / wildcard / ctor / tuple.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pat {
    /// A variable binder — binds the matched value (a constructor payload field)
    /// to a name.
    Var(Symbol),
    /// A wildcard `_` — matches any value and binds nothing.
    Wildcard,
    /// A constructor pattern `Variant sub0 sub1 …` (a nullary pattern `Variant`
    /// has an empty `args`).
    Ctor {
        ty: Symbol,
        variant: Symbol,
        args: Vec<Self>,
    },
    /// A tuple pattern `(p0, p1, …)`, destructuring an [`IrType::Tuple`] value
    /// element-by-element.
    ///
    /// The element sub-patterns reuse the existing [`Pat`] variants. The tuple-
    /// value invariant (arity ≥ 2) applies to well-formed IR — the lowerer is
    /// the sole producer and upholds it — but the backend stays total over any
    /// element vector it receives and never panics on a degenerate arity.
    Tuple(Vec<Self>),
}

/// An exhaustive case analysis over an enum scrutinee.
///
/// Fields are private: the sole way to obtain a `Match` is [`Match::new`],
/// which proves exhaustiveness at construction time. This makes a
/// non-exhaustive `Match` unrepresentable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Match {
    scrutinee: Box<Expr>,
    arms: Vec<Arm>,
}

impl Match {
    /// Build an exhaustive `Match`.
    ///
    /// `variants` is the complete set of constructors of the scrutinee's enum.
    /// The arm set is accepted only when it covers exactly that set, with no
    /// duplicate, unknown, or missing variant.
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] when the arms do not form an
    /// exhaustive, non-redundant cover of `variants` — an internal invariant
    /// violation the lowerer must never produce.
    pub fn new(scrutinee: Expr, arms: Vec<Arm>, variants: &[Symbol]) -> DResult<Self> {
        let expected: BTreeSet<Symbol> = variants.iter().copied().collect();

        let mut covered: BTreeSet<Symbol> = BTreeSet::new();
        for arm in &arms {
            // The case-arm head is always a constructor pattern (payload binders
            // are sub-patterns). A bare variable / wildcard whole-scrutinee arm
            // is not an M3a shape, so the lowerer never produces one here — a
            // non-ctor arm head is an internal invariant violation, surfaced as
            // a `CompilerBug` rather than silently skewing the coverage count.
            let Pat::Ctor { variant, .. } = &arm.pat else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_ir::Match::new",
                    detail: "match arm head is not a constructor pattern".to_owned(),
                });
            };
            let variant = *variant;
            if !expected.contains(&variant) {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_ir::Match::new",
                    detail: format!(
                        "match arm covers variant {} not in the scrutinee's enum",
                        variant.as_raw()
                    ),
                });
            }
            if !covered.insert(variant) {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_ir::Match::new",
                    detail: format!("duplicate match arm for variant {}", variant.as_raw()),
                });
            }
        }

        if covered != expected {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_ir::Match::new",
                detail: format!(
                    "non-exhaustive match: covered {} of {} variants",
                    covered.len(),
                    expected.len()
                ),
            });
        }

        Ok(Self {
            scrutinee: Box::new(scrutinee),
            arms,
        })
    }

    #[must_use]
    pub fn scrutinee(&self) -> &Expr {
        &self.scrutinee
    }

    #[must_use]
    pub fn arms(&self) -> &[Arm] {
        &self.arms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_diagnostics::DResult;
    use sky_intern::Interner;

    fn msg_enum(i: &mut Interner) -> DResult<(Symbol, Symbol, Symbol)> {
        let ty = i.intern("Msg")?;
        let inc = i.intern("Increment")?;
        let dec = i.intern("Decrement")?;
        Ok((ty, inc, dec))
    }

    #[test]
    fn match_new_accepts_exhaustive_and_round_trips_debug() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let count = i.intern("count")?;

        // case msg of Increment -> count + 1 ; Decrement -> count - 1
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
            },
            Arm {
                pat: Pat::Ctor {
                    ty,
                    variant: dec,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
            },
        ];
        let res = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);

        assert_eq!(res.as_ref().map(|m| m.arms().len()), Ok(2));
        assert!(matches!(
            res.as_ref().map(Match::scrutinee),
            Ok(Expr::Var(_))
        ));
        // Debug round-trips (no panic, stable shape).
        let rendered = format!("{res:?}");
        assert!(rendered.contains("Match"));
        Ok(())
    }

    #[test]
    fn match_new_rejects_non_exhaustive() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let count = i.intern("count")?;

        // Only the Increment arm — Decrement uncovered.
        let arms = vec![Arm {
            pat: Pat::Ctor {
                ty,
                variant: inc,
                args: vec![],
            },
            body: Expr::Var(count),
        }];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    #[test]
    fn match_new_rejects_duplicate_arm() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;

        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
            },
            Arm {
                pat: Pat::Ctor {
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(1),
            },
        ];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    #[test]
    fn match_new_rejects_unknown_variant() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let bogus = i.intern("Reset")?;

        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
            },
            Arm {
                pat: Pat::Ctor {
                    ty,
                    variant: bogus,
                    args: vec![],
                },
                body: Expr::Int(1),
            },
        ];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    #[test]
    fn tuple_expr_and_type_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;

        // ( x + 1, 2, "three"-as-Var ) — a 3-tuple expression.
        let expr = Expr::Tuple(vec![
            Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(x)),
                rhs: Box::new(Expr::Int(1)),
            },
            Expr::Int(2),
            Expr::Var(x),
        ]);
        assert_eq!(expr, expr.clone());
        let rendered = format!("{expr:?}");
        assert!(rendered.contains("Tuple"));

        // (Int, Bool) — a 2-tuple type.
        let ty = IrType::Tuple(vec![IrType::Int, IrType::Bool]);
        assert_eq!(ty, ty.clone());
        assert!(format!("{ty:?}").contains("Tuple"));

        // Nested tuple type: (Int, (Bool, String)).
        let nested = IrType::Tuple(vec![
            IrType::Int,
            IrType::Tuple(vec![IrType::Bool, IrType::Str]),
        ]);
        assert_eq!(nested, nested.clone());
        Ok(())
    }

    #[test]
    fn record_expr_access_update_and_type_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;
        let y = i.intern("y")?;
        let p = i.intern("p")?;

        // { x = 1, y = 2 } — fields sorted by name (x before y).
        let lit = Expr::Record(vec![(x, Expr::Int(1)), (y, Expr::Int(2))]);
        assert_eq!(lit, lit.clone());
        assert!(format!("{lit:?}").contains("Record"));

        // p.x — a field access.
        let access = Expr::Access {
            record: Box::new(Expr::Var(p)),
            field: x,
        };
        assert_eq!(access, access.clone());
        assert!(format!("{access:?}").contains("Access"));

        // { p | x = 5 } — a single-field update.
        let update = Expr::Update {
            record: Box::new(Expr::Var(p)),
            fields: vec![(x, Expr::Int(5))],
        };
        assert_eq!(update, update.clone());
        assert!(format!("{update:?}").contains("Update"));

        // { x : Int, y : Bool } — a closed record TYPE.
        let mut fields = BTreeMap::new();
        fields.insert(x, IrType::Int);
        fields.insert(y, IrType::Bool);
        let ty = IrType::Record(fields);
        assert_eq!(ty, ty.clone());
        assert!(format!("{ty:?}").contains("Record"));

        // Nested record type: { x : Int, y : { x : Int, y : Bool } }.
        let mut outer = BTreeMap::new();
        outer.insert(x, IrType::Int);
        outer.insert(y, ty);
        let nested = IrType::Record(outer);
        assert_eq!(nested, nested.clone());
        Ok(())
    }

    #[test]
    fn lambda_apply_expr_and_fun_type_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;
        let f = i.intern("f")?;

        // \x -> x + 1 — a single-param lambda returning Int.
        let lambda = Expr::Lambda {
            params: vec![(x, IrType::Int)],
            ret: IrType::Int,
            body: Box::new(Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(x)),
                rhs: Box::new(Expr::Int(1)),
            }),
        };
        assert_eq!(lambda, lambda.clone());
        assert!(format!("{lambda:?}").contains("Lambda"));

        // f 2 — apply the function-typed local `f` to one argument.
        let apply = Expr::Apply {
            func: Box::new(Expr::Var(f)),
            args: vec![Expr::Int(2)],
        };
        assert_eq!(apply, apply.clone());
        assert!(format!("{apply:?}").contains("Apply"));

        // Int -> Int — a one-param function type.
        let fun_ty = IrType::Fun(vec![IrType::Int], Box::new(IrType::Int));
        assert_eq!(fun_ty, fun_ty.clone());
        assert!(format!("{fun_ty:?}").contains("Fun"));

        // () -> Bool — a nullary function type (distinct from Bool alone).
        let nullary = IrType::Fun(vec![], Box::new(IrType::Bool));
        assert_eq!(nullary, nullary.clone());
        assert_ne!(nullary, IrType::Bool);

        // (Int, Bool) -> Int — a multi-param function type, nested under Fun.
        let multi = IrType::Fun(
            vec![IrType::Int, IrType::Bool],
            Box::new(IrType::Fun(vec![IrType::Str], Box::new(IrType::Unit))),
        );
        assert_eq!(multi, multi.clone());

        // A top-level function named as a first-class value: callee `fn#0`,
        // reified at its boxed `Int -> Int` value type.
        let func_value = Expr::FuncValue {
            callee: Callee::Func(FuncId::from_raw(0)),
            ty: IrType::Fun(vec![IrType::Int], Box::new(IrType::Int)),
        };
        assert_eq!(func_value, func_value.clone());
        assert!(format!("{func_value:?}").contains("FuncValue"));
        Ok(())
    }

    #[test]
    fn generic_type_and_quantified_func_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let a = i.intern("a")?;
        let b = i.intern("b")?;
        let x = i.intern("x")?;
        let id = i.intern("id")?;

        // A fully-parametric `id : a -> a` quantifying [a].
        let generic_a = IrType::Generic(a);
        assert_eq!(generic_a, generic_a.clone());
        assert!(format!("{generic_a:?}").contains("Generic"));

        let func = Func {
            id: FuncId::from_raw(0),
            name: id,
            type_params: vec![a],
            params: vec![(x, IrType::Generic(a))],
            ret: IrType::Generic(a),
            body: Expr::Var(x),
        };
        assert_eq!(func, func.clone());
        assert_eq!(func.type_params, vec![a]);

        // Distinct generic vars compare unequal; quantification order is carried
        // verbatim (no dedup / sort), so [a, b] stays [a, b].
        assert_ne!(IrType::Generic(a), IrType::Generic(b));
        let two = Func {
            id: FuncId::from_raw(1),
            name: id,
            type_params: vec![a, b],
            params: vec![(x, IrType::Generic(a))],
            ret: IrType::Generic(b),
            body: Expr::Var(x),
        };
        assert_eq!(two.type_params, vec![a, b]);
        Ok(())
    }

    #[test]
    fn program_round_trips_debug() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let main_sym = i.intern("main")?;
        let main_mod = i.intern("Main")?;

        let func = Func {
            id: FuncId::from_raw(0),
            name: main_sym,
            type_params: vec![],
            params: vec![],
            ret: IrType::TaskUnit,
            body: Expr::Call {
                callee: Callee::Kernel(KernelFn::LogPrintln),
                args: vec![Expr::Call {
                    callee: Callee::Kernel(KernelFn::StringFromInt),
                    args: vec![Expr::Int(1)],
                }],
            },
        };
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: ty,
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: inc,
                            fields: vec![],
                        },
                        Variant {
                            name: dec,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![func],
                entry: Some(FuncId::from_raw(0)),
                records: vec![],
            }],
        };
        let clone = program.clone();
        assert_eq!(program, clone);
        assert!(format!("{program:?}").contains("Program"));
        Ok(())
    }

    #[test]
    fn let_if_and_extended_binops_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;

        // let x = 6 / 2 in if (x == 3) && (x > 0) then x * 10 else x - 1
        let expr = Expr::Let {
            name: x,
            value: Box::new(Expr::BinOp {
                op: BinOp::Div,
                lhs: Box::new(Expr::Int(6)),
                rhs: Box::new(Expr::Int(2)),
            }),
            body: Box::new(Expr::If {
                cond: Box::new(Expr::BinOp {
                    op: BinOp::And,
                    lhs: Box::new(Expr::BinOp {
                        op: BinOp::Eq,
                        lhs: Box::new(Expr::Var(x)),
                        rhs: Box::new(Expr::Int(3)),
                    }),
                    rhs: Box::new(Expr::BinOp {
                        op: BinOp::Gt,
                        lhs: Box::new(Expr::Var(x)),
                        rhs: Box::new(Expr::Int(0)),
                    }),
                }),
                then_: Box::new(Expr::BinOp {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(10)),
                }),
                else_: Box::new(Expr::BinOp {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(1)),
                }),
            }),
        };

        // Clone + structural equality + Debug all hold for the new variants.
        assert_eq!(expr, expr.clone());
        let rendered = format!("{expr:?}");
        assert!(rendered.contains("Let"));
        assert!(rendered.contains("If"));

        // Every extended BinOp is a distinct, Copy, comparable value: the full
        // set has no duplicates and the Copy bound holds (the array is consumed
        // by value below without moving out of `all`).
        let all = [
            BinOp::Add,
            BinOp::Sub,
            BinOp::Mul,
            BinOp::Div,
            BinOp::Eq,
            BinOp::Neq,
            BinOp::Lt,
            BinOp::Gt,
            BinOp::Le,
            BinOp::Ge,
            BinOp::And,
            BinOp::Or,
        ];
        let distinct: BTreeSet<_> = all.iter().map(|op| format!("{op:?}")).collect();
        assert_eq!(distinct.len(), all.len());
        let copied = all;
        assert_eq!(copied.len(), all.len());
        Ok(())
    }

    #[test]
    fn payload_and_generic_enum_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let a = i.intern("a")?;
        let maybe = i.intern("Maybe")?;
        let just = i.intern("Just")?;
        let nothing = i.intern("Nothing")?;

        // type Maybe a = Just a | Nothing — one generic param, one payload
        // variant (carrying the type variable), one nullary variant.
        let def = EnumDef {
            name: maybe,
            type_params: vec![a],
            variants: vec![
                Variant {
                    name: just,
                    fields: vec![IrType::Generic(a)],
                },
                Variant {
                    name: nothing,
                    fields: vec![],
                },
            ],
        };
        assert_eq!(def, def.clone());
        assert_eq!(def.type_params, vec![a]);
        assert_eq!(def.variants.len(), 2);
        assert!(def.variants.first().is_some_and(|v| !v.fields.is_empty()));
        assert!(def.variants.get(1).is_some_and(|v| v.fields.is_empty()));

        // A use-site type `Maybe Int` carries its concrete type argument.
        let use_ty = IrType::Enum {
            name: maybe,
            args: vec![IrType::Int],
        };
        assert_eq!(use_ty, use_ty.clone());
        // A non-generic enum use carries no args and is distinct from the applied
        // form.
        let bare = IrType::Enum {
            name: maybe,
            args: vec![],
        };
        assert_ne!(use_ty, bare);

        // Construction `Just 5` carries its payload argument.
        let ctor = Expr::Ctor {
            ty: maybe,
            variant: just,
            args: vec![Expr::Int(5)],
        };
        assert_eq!(ctor, ctor.clone());
        assert!(format!("{ctor:?}").contains("Ctor"));
        Ok(())
    }

    #[test]
    fn ctor_pattern_with_var_and_wildcard_payloads_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let maybe = i.intern("Maybe")?;
        let just = i.intern("Just")?;
        let nothing = i.intern("Nothing")?;
        let x = i.intern("x")?;
        let m = i.intern("m")?;

        // case m of Just x -> x ; Nothing -> 0  — a var-binding payload pattern
        // and a nullary pattern. Match::new accepts it (coverage over the variant
        // NAME set; payload binding does not affect coverage).
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    ty: maybe,
                    variant: just,
                    args: vec![Pat::Var(x)],
                },
                body: Expr::Var(x),
            },
            Arm {
                pat: Pat::Ctor {
                    ty: maybe,
                    variant: nothing,
                    args: vec![],
                },
                body: Expr::Int(0),
            },
        ];
        let m1 = Match::new(Expr::Var(m), arms, &[just, nothing])?;
        assert_eq!(m1.arms().len(), 2);

        // The wildcard payload sub-pattern is also representable.
        let wild = Pat::Ctor {
            ty: maybe,
            variant: just,
            args: vec![Pat::Wildcard],
        };
        assert_eq!(wild, wild.clone());
        assert!(format!("{wild:?}").contains("Wildcard"));
        Ok(())
    }

    #[test]
    fn match_new_rejects_non_ctor_arm_head() -> DResult<()> {
        let mut i = Interner::new();
        let (_ty, inc, dec) = msg_enum(&mut i)?;

        // A bare variable whole-scrutinee arm is not an M3a shape — the arm head
        // must be a constructor pattern, so Match::new fails closed.
        let arms = vec![Arm {
            pat: Pat::Var(i.intern("anything")?),
            body: Expr::Int(0),
        }];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    #[test]
    fn recursive_enum_def_round_trips() -> DResult<()> {
        let mut i = Interner::new();
        let tree = i.intern("Tree")?;
        let leaf = i.intern("Leaf")?;
        let node = i.intern("Node")?;

        // type Tree = Leaf | Node Tree Int Tree — the Node payload carries two
        // direct self-edges (the enum's own type) around an Int.
        let self_ty = IrType::Enum {
            name: tree,
            args: vec![],
        };
        let def = EnumDef {
            name: tree,
            type_params: vec![],
            variants: vec![
                Variant {
                    name: leaf,
                    fields: vec![],
                },
                Variant {
                    name: node,
                    fields: vec![self_ty.clone(), IrType::Int, self_ty],
                },
            ],
        };
        assert_eq!(def, def.clone());
        assert!(def.variants.get(1).is_some_and(|v| v.fields.len() == 3));
        Ok(())
    }
}
