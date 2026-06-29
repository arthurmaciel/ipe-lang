//! Constraint generation, ported from the M0-relevant arms of
//! `Sky.Type.Constrain.Expression` (derivative of elm/compiler's
//! `Type.Constrain.Expression`, BSD-3-Clause).
//!
//! Walks the canonical module, minting a union-find variable for each
//! sub-expression region and emitting equality [`Constraint`]s that the solver
//! discharges. The arms modelled are exactly those the M0 golden program
//! exercises: integer literals, `VarLocal` / `VarTopLevel` / `VarKernel` /
//! `VarCtor` references, function application (`Call`), `case`, and the binary
//! operators `+` / `-`.
//!
//! This module also owns the two bridges between the resolved [`Ty`] level and
//! the solver level: [`Builder::instantiate`] (a [`Ty`] → fresh union-find
//! structure) and [`Builder::zonk`] (a settled union-find variable → [`Ty`]).

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, Span, TypeError};
use sky_intern::{Interner, Symbol};

use crate::doc::canon_type_to_doc;
use crate::solve::{Budget, Constraint};
use crate::ty::{Content, FlatType, Ty, TyBounds, from_canon};
use crate::unionfind::{UnionFind, VarId};

/// `where_` tag for any `CompilerBug` raised during constraint generation.
const STAGE: &str = "sky_types::constrain";

/// Maximum number of nodes [`zonk`] reads back from a single type before
/// declaring it pathologically deep. The occurs check in unification rules out
/// true cycles, so this bound is only ever hit on adversarial input.
///
/// Kept deliberately **well under** the native-stack ceiling (a few thousand,
/// not the previous 100 000): the [`Ty`] this produces is then walked
/// recursively by the renderer ([`crate::doc::ty_to_doc`]), so capping the node
/// count here keeps that downstream recursion provably stack-safe. The
/// read-back itself is iterative (an explicit work stack), so it never grows the
/// native stack regardless of the bound.
const ZONK_NODE_LIMIT: u32 = 4_096;

/// Interned symbols for the built-in type constructors the inferencer needs to
/// name. `Int` / `String` usually already exist (from the source), but `Task`
/// never appears in M0 source, so the builder interns them up front to
/// guarantee a stable, resolvable [`Symbol`] for each.
struct Builtins {
    int: Symbol,
    float: Symbol,
    bool: Symbol,
    string: Symbol,
    char: Symbol,
    task: Symbol,
    maybe: Symbol,
    result: Symbol,
    list: Symbol,
    /// Interned `Just` / `Nothing` / `Ok` / `Err` / `True` / `False` — the
    /// Prelude-exposed built-in constructor names.
    just: Symbol,
    nothing: Symbol,
    ok: Symbol,
    err: Symbol,
    true_: Symbol,
    false_: Symbol,
    /// `Sky.Core.Dict` type constructor symbol.
    dict: Symbol,
    /// `Sky.Core.Set` type constructor symbol.
    set: Symbol,
    /// Two distinct scheme type-variable symbols (`a`, `e`) used to build the
    /// built-in constructor schemes. Their identity links a constructor's
    /// payload to its result type, exactly like a user union's declared vars;
    /// each use site instantiates them fresh through one shared map.
    tv_a: Symbol,
    tv_e: Symbol,
}

impl Builtins {
    fn new(interner: &mut Interner) -> DResult<Self> {
        Ok(Self {
            int: interner.intern("Int")?,
            float: interner.intern("Float")?,
            bool: interner.intern("Bool")?,
            string: interner.intern("String")?,
            char: interner.intern("Char")?,
            task: interner.intern("Task")?,
            maybe: interner.intern("Maybe")?,
            result: interner.intern("Result")?,
            list: interner.intern("List")?,
            dict: interner.intern("Dict")?,
            set: interner.intern("Set")?,
            just: interner.intern("Just")?,
            nothing: interner.intern("Nothing")?,
            ok: interner.intern("Ok")?,
            err: interner.intern("Err")?,
            true_: interner.intern("True")?,
            false_: interner.intern("False")?,
            tv_a: interner.intern("a")?,
            tv_e: interner.intern("e")?,
        })
    }

    /// The Prelude-built-in constructor schemes, keyed by constructor name.
    ///
    /// `Bool` (`True` / `False` : `Bool`), `Maybe a` (`Just : a -> Maybe a`,
    /// `Nothing : Maybe a`), and `Result e a` (`Ok : a -> Result e a`,
    /// `Err : e -> Result e a`). These types have no user `type` declaration, so
    /// their schemes are synthesised here; each is instantiated fresh per use
    /// site exactly like a user constructor's scheme. The built-in `Con`s carry
    /// an empty module path, matching how `from_canon` renders the builtin type
    /// names (`Int` / `Bool` / …) and how the lowerer recognises them by name.
    fn ctor_schemes(&self) -> Vec<(Symbol, CtorScheme)> {
        let bool_ty = Ty::Con {
            module: Vec::new(),
            name: self.bool,
            args: Vec::new(),
        };
        let maybe_ty = Ty::Con {
            module: Vec::new(),
            name: self.maybe,
            args: vec![Ty::Var(self.tv_a.as_raw())],
        };
        let result_ty = Ty::Con {
            module: Vec::new(),
            name: self.result,
            args: vec![Ty::Var(self.tv_e.as_raw()), Ty::Var(self.tv_a.as_raw())],
        };
        vec![
            (
                self.true_,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: bool_ty.clone(),
                },
            ),
            (
                self.false_,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: bool_ty,
                },
            ),
            (
                self.just,
                CtorScheme {
                    arg_tys: vec![Ty::Var(self.tv_a.as_raw())],
                    result: maybe_ty.clone(),
                },
            ),
            (
                self.nothing,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: maybe_ty,
                },
            ),
            (
                self.ok,
                CtorScheme {
                    arg_tys: vec![Ty::Var(self.tv_a.as_raw())],
                    result: result_ty.clone(),
                },
            ),
            (
                self.err,
                CtorScheme {
                    arg_tys: vec![Ty::Var(self.tv_e.as_raw())],
                    result: result_ty,
                },
            ),
        ]
    }
}

/// The type discipline a binary operator imposes. Classified once from the
/// resolved kernel name so the constraint walk doesn't re-borrow the interner.
#[derive(Clone, Copy)]
enum BinopClass {
    /// `//`: integer division `Int -> Int -> Int`.
    IntDiv,
    /// `/`: `Float -> Float -> Float` (matches the Go backend's float division).
    FloatDiv,
    /// `+ - *`: `Number a => a -> a -> a`. The operands and the result share one
    /// numeric variable carrying the named obligation, so the operation stays
    /// generic over `Int` / `Float` until a concrete operand pins it.
    Num(TyBounds),
    /// `< > <= >=`: `Comparable a => a -> a -> Bool` — operands share one
    /// ordered type; the result is `Bool`.
    Order,
    /// `== /=`: `Equatable a => a -> a -> Bool` — operands share one equatable
    /// type (structural equality is total over every non-function type); the
    /// result is `Bool`. The shared variable carries the equality obligation, so
    /// a generalised use emits a Rust `PartialEq` bound.
    Equality,
    /// `&& ||`: `Bool -> Bool -> Bool`.
    Boolean,
    /// `++`: `String -> String -> String`. The general `Appendable` super-type
    /// (which would also cover `List a -> List a -> List a`) is a later batch;
    /// for now both operands and the result are pinned to `String`, so applying
    /// `++` to any other type (a would-be `List`) is a fail-closed type error
    /// rather than a mis-typed pass-through.
    Append,
    /// Any other operator (`::`, …): `a -> a -> a`. The numeric/ordering
    /// super-types do not cover list cons, so it stays a plain pass-through here
    /// and is gated at lowering rather than mis-typed.
    Poly,
}

/// Classify a resolved operator kernel name (`add`, `eq`, `and`, …).
const fn classify_binop(func: &str) -> BinopClass {
    match func.as_bytes() {
        b"add" => BinopClass::Num(TyBounds::add()),
        b"sub" => BinopClass::Num(TyBounds::sub()),
        b"mul" => BinopClass::Num(TyBounds::mul()),
        b"idiv" => BinopClass::IntDiv,
        b"fdiv" => BinopClass::FloatDiv,
        b"lt" | b"gt" | b"le" | b"ge" => BinopClass::Order,
        b"eq" | b"neq" => BinopClass::Equality,
        b"and" | b"or" => BinopClass::Boolean,
        b"append" => BinopClass::Append,
        _ => BinopClass::Poly,
    }
}

/// The Sky `comparable`-key obligation a kernel module's element / key variable
/// carries, or `None` for a module without such a position. `Set`'s element is
/// keyed by `BTreeSet<A>` (`A : Ord`); `Dict`'s key by a determinism-sorted
/// `HashMap<K, V>` (`K : Hash + Eq + Ord`). The obligation is attached to raw
/// scheme-variable 0, which is the element / key in every `Set` / `Dict` kernel.
fn key_obligation(interner: &Interner, module: Symbol) -> Option<TyBounds> {
    match interner.resolve(module) {
        Some("Set") => Some(TyBounds::set_elem()),
        Some("Dict") => Some(TyBounds::dict_key()),
        _ => None,
    }
}

/// The constraint-generation state threaded through the walk.
pub struct Builder<'a> {
    uf: &'a mut UnionFind<Content>,
    interner: &'a Interner,
    builtins: Builtins,
    /// Resolved type per source region (filled with vars, read back post-solve).
    regions: BTreeMap<Span, VarId>,
    /// Equality constraints to be discharged by the solver.
    constraints: Vec<Constraint>,
    /// Annotation-derived types of every top-level binding, for cross-binding
    /// references (`main` mentions `update`).
    top_level: BTreeMap<Symbol, Ty>,
    /// Body region-var of each untyped top-level binding, read back for `env`.
    untyped: BTreeMap<Symbol, VarId>,
    /// Deferred record field-access obligations, resolved after the main solve.
    field_accesses: Vec<FieldAccess>,
    /// Deferred record-update obligations, resolved after the main solve.
    record_updates: Vec<RecordUpdate>,
    /// The type scheme of every data constructor declared in this module, keyed
    /// by constructor name. A constructor is a (possibly generic) function
    /// `field0 -> … -> fieldN -> T vars`; each use site instantiates the scheme
    /// fresh, exactly as a polymorphic top-level binding does.
    ctors: BTreeMap<Symbol, CtorScheme>,
    /// One entry per typed binding: its name and the rigid (skolem) variable each
    /// of its annotation type variables instantiated to while its body was
    /// checked. Read post-solve to recover each variable's super-type obligations
    /// (the bounds the body imposed) for generalisation.
    typed_rigids: Vec<(Symbol, BTreeMap<Symbol, VarId>)>,
    /// One entry per *reference* to a typed top-level binding (each `VarTopLevel`
    /// use site), recording how that use instantiated the binding's scheme. Used
    /// post-solve to check a super-typed binding's obligations against the
    /// concrete type each use pins it to.
    scheme_apps: Vec<SchemeApp>,
    /// Every super-typed flex variable minted by a numeric / ordering / equality
    /// operator, paired with the obligations it was minted with and the operand
    /// span to blame. Read post-solve for two jobs: numeric defaulting (an
    /// unpinned `Number` variable resolves to `Int`, matching the reference
    /// compiler's defaulting of an otherwise-unconstrained `number`) and the
    /// concrete-pin soundness gate (a variable that pinned to a concrete type
    /// during solving must be one the operation truly supports — an equality
    /// obligation rejects a type containing a function, which Rust cannot
    /// compare, with SKY-T0014 rather than emitting code `cargo` rejects).
    super_vars: Vec<(VarId, TyBounds, Span)>,
}

/// A single use site of a typed top-level binding.
///
/// At each reference the binding's scheme is instantiated into fresh variables
/// (the [`Builder::instantiate`] / `CForeign` path). `vars` records, for each of
/// the scheme's type variables (keyed by the annotation variable's raw symbol
/// id), the fresh union-find variable it instantiated to — so once the solver
/// settles, the concrete type this use pinned each variable to can be read back
/// and checked against the binding's super-type obligations.
pub struct SchemeApp {
    /// The referenced binding's name.
    pub name: Symbol,
    /// Scheme type-variable raw id → the fresh variable it instantiated to here.
    pub vars: BTreeMap<u32, VarId>,
    /// The reference's source span, for blame on an unsatisfied bound.
    pub span: Span,
}

/// A data constructor's quantified type scheme.
///
/// `arg_tys` are the declared payload field types (a nullary constructor has an
/// empty list); `result` is the enum type the constructor builds, applied to the
/// union's type variables (`Maybe a` for `Just`). Both sides share the union's
/// type variables as [`Ty::Var`]s, so instantiating them through one shared map
/// alpha-renames a generic constructor consistently per use site.
#[derive(Clone)]
struct CtorScheme {
    arg_tys: Vec<Ty>,
    result: Ty,
}

/// A deferred record field-access obligation `record.field`.
///
/// Closed records carry no row variable, so a field access cannot be discharged
/// by ordinary unification while the constraints are still being built (the
/// record's type may not be settled yet). Each access is recorded here and
/// resolved once after the main solve, when [`crate::resolve_field_accesses`]
/// can read the now-settled record type and link `result` to the field's type.
pub struct FieldAccess {
    /// The variable of the record sub-expression (`record` in `record.field`).
    pub record: VarId,
    /// The accessed field name.
    pub field: Symbol,
    /// The variable the access's result type was bound to (the access's region).
    pub result: VarId,
    /// The access expression's source span, for blame.
    pub span: Span,
}

/// A deferred record-update obligation `{ base | field = value, ... }`.
///
/// Like [`FieldAccess`], a closed record carries no row variable, so the
/// updated fields cannot be checked against the base's type while the
/// constraints are still being built. Each update is recorded here and resolved
/// once after the main solve, when [`crate::resolve_record_updates`] reads the
/// settled base type and unifies each updated value against the corresponding
/// field's type (a field absent from the base is a [`crate::TypeError::NoSuchField`]).
pub struct RecordUpdate {
    /// The variable of the base record being copied (`base` in `{ base | … }`).
    pub record: VarId,
    /// The updated `(field name, value variable)` pairs.
    pub fields: Vec<(Symbol, VarId)>,
    /// The update expression's source span, for blame.
    pub span: Span,
}

/// The output of constraint generation, consumed by the solver + read-back.
pub struct Generated {
    pub regions: BTreeMap<Span, VarId>,
    pub constraints: Vec<Constraint>,
    pub top_level: BTreeMap<Symbol, Ty>,
    pub untyped: BTreeMap<Symbol, VarId>,
    pub field_accesses: Vec<FieldAccess>,
    pub record_updates: Vec<RecordUpdate>,
    pub typed_rigids: Vec<(Symbol, BTreeMap<Symbol, VarId>)>,
    pub scheme_apps: Vec<SchemeApp>,
    pub super_vars: Vec<(VarId, TyBounds, Span)>,
}

impl<'a> Builder<'a> {
    /// Build a constraint set for the whole module.
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] on an internal invariant violation (e.g. an
    /// arity mismatch between a binding's pattern count and its annotation, or
    /// an unbound local — both ruled out by canonicalisation).
    pub fn run(
        uf: &'a mut UnionFind<Content>,
        interner: &'a mut Interner,
        module: &canon::Module,
    ) -> DResult<Generated> {
        let builtins = Builtins::new(interner)?;
        let mut builder = Self {
            uf,
            interner,
            builtins,
            regions: BTreeMap::new(),
            constraints: Vec::new(),
            top_level: BTreeMap::new(),
            untyped: BTreeMap::new(),
            field_accesses: Vec::new(),
            record_updates: Vec::new(),
            ctors: BTreeMap::new(),
            typed_rigids: Vec::new(),
            scheme_apps: Vec::new(),
            super_vars: Vec::new(),
        };

        // Register the Prelude-built-in constructor schemes (`True` / `False` /
        // `Just` / `Nothing` / `Ok` / `Err`) first, so a reference or pattern
        // instantiates `Maybe a` / `Result e a` / `Bool` fresh per use site. A
        // user `type` cannot shadow these names (the canon §3.2 gate rejects it),
        // so the module-union loop below never collides with them.
        for (name, scheme) in builder.builtins.ctor_schemes() {
            builder.ctors.insert(name, scheme);
        }

        // Register every data constructor's scheme up front, so a `VarCtor`
        // reference or a constructor pattern can instantiate it fresh. A
        // constructor `C : field0 -> … -> T vars`; the result type applies the
        // union to its declared type variables (as `Ty::Var`s), and the field
        // types carry those same variables, so one shared instantiation map
        // alpha-renames a generic constructor per use site.
        for union in &module.unions {
            let result = Ty::Con {
                module: module.name.clone(),
                name: union.name,
                args: union.vars.iter().map(|v| Ty::Var(v.as_raw())).collect(),
            };
            for ctor in &union.ctors {
                let arg_tys = ctor.args.iter().map(from_canon).collect();
                builder.ctors.insert(
                    ctor.name,
                    CtorScheme {
                        arg_tys,
                        result: result.clone(),
                    },
                );
            }
        }

        // First pass: register every binding so any binding can reference any
        // other (forward references resolve).
        //
        // * Typed bindings record their annotation type — the binding's *scheme*,
        //   instantiated fresh (flex) at each reference (`VarTopLevel`).
        // * Untyped bindings mint one shared monomorphic variable up front. Every
        //   reference resolves to that *same* variable, so a reference is checked
        //   against the binding's inferred type instead of being left
        //   unconstrained. The variable's settled type is read back into `env`.
        //   (Generalising an *un*annotated binding so it can be used at several
        //   concrete types in one module needs rank-based let-generalisation,
        //   which the M2a solver does not yet model — so an untyped polymorphic
        //   binding is monomorphic at its use sites. Sound, not yet complete;
        //   write an annotation to get full polymorphism.)
        for def in &module.defs {
            match def {
                canon::Def::Typed { name, ty, .. } => {
                    builder.top_level.insert(name.value, from_canon(ty));
                }
                canon::Def::Untyped { name, .. } => {
                    let v = builder.flex()?;
                    builder.untyped.insert(name.value, v);
                }
            }
        }

        // Second pass: constrain each binding's body.
        for def in &module.defs {
            builder.constrain_def(def)?;
        }

        Ok(Generated {
            regions: builder.regions,
            constraints: builder.constraints,
            top_level: builder.top_level,
            untyped: builder.untyped,
            field_accesses: builder.field_accesses,
            record_updates: builder.record_updates,
            typed_rigids: builder.typed_rigids,
            scheme_apps: builder.scheme_apps,
            super_vars: builder.super_vars,
        })
    }

    // ── solver-var construction helpers ────────────────────────────────────

    fn flex(&mut self) -> DResult<VarId> {
        self.uf.fresh(Content::Flex)
    }

    fn rigid(&mut self) -> DResult<VarId> {
        self.uf.fresh(Content::Rigid)
    }

    fn structure(&mut self, f: FlatType) -> DResult<VarId> {
        self.uf.fresh(Content::Structure(f))
    }

    fn int_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.int;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn bool_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.bool;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn float_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.float;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn string_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.string;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn char_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.char;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    /// Mint a fresh super-typed flexible variable carrying `bounds` — a value
    /// the body has constrained to a Sky super-type (numeric / ordered /
    /// equatable) but not yet to a concrete type. It pins to any matching type,
    /// or — when it meets an annotation skolem — lifts that skolem's obligations
    /// so the generic parameter is emitted with the matching trait bound.
    /// `span` is the operand span blamed if the variable later pins to a
    /// concrete type that does not actually support the operation.
    fn super_var(&mut self, bounds: TyBounds, span: Span) -> DResult<VarId> {
        let v = self.uf.fresh(Content::Super {
            rigid: false,
            bounds,
        })?;
        self.super_vars.push((v, bounds, span));
        Ok(v)
    }

    /// Constrain a binary operation by the type discipline of its operator. The
    /// returned [`VarId`] is the result type's variable. Mirrors the M1-core
    /// subset of `Sky.Type.Constrain.Expression.binopTypes`.
    fn constrain_binop(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        func: Symbol,
        lhs: &canon::Expr,
        rhs: &canon::Expr,
    ) -> DResult<VarId> {
        let class = classify_binop(self.interner.resolve(func).unwrap_or(""));
        let lv = self.constrain_expr(local, lhs)?;
        let rv = self.constrain_expr(local, rhs)?;
        match class {
            BinopClass::Num(bounds) => {
                // `+ - *` are Number-polymorphic: operands and result share one
                // numeric variable. A concrete operand (`x + 1`) pins it to that
                // type; an all-variable use (`x + x`) leaves it generic, carrying
                // the operator's obligation so generalisation emits the bound.
                let s = self.super_var(bounds, lhs.span)?;
                self.eq(lhs.span, lv, s);
                self.eq(rhs.span, rv, s);
                Ok(s)
            }
            BinopClass::IntDiv => {
                let li = self.int_var()?;
                self.eq(lhs.span, lv, li);
                let ri = self.int_var()?;
                self.eq(rhs.span, rv, ri);
                self.int_var()
            }
            BinopClass::FloatDiv => {
                let lf = self.float_var()?;
                self.eq(lhs.span, lv, lf);
                let rf = self.float_var()?;
                self.eq(rhs.span, rv, rf);
                self.float_var()
            }
            BinopClass::Order => {
                // `< > <= >=` are Comparable-polymorphic: operands share one
                // ordered type (carrying the ordering obligation), result Bool.
                let s = self.super_var(TyBounds::ord(), lhs.span)?;
                self.eq(lhs.span, lv, s);
                self.eq(rhs.span, rv, s);
                self.bool_var()
            }
            BinopClass::Equality => {
                // `== /=` are Equatable-polymorphic: operands share one equatable
                // type (carrying the equality obligation), result Bool. A
                // concrete operand pins it (`n == 1` → `Int`); an all-variable
                // use (`p == q`) leaves it generic, so generalisation emits a
                // `PartialEq` bound rather than an unbounded `T{n}` the backend
                // could not compare. A function operand fails the pin and a
                // function instantiation fails the post-solve gate (SKY-T0014).
                let s = self.super_var(TyBounds::eq(), lhs.span)?;
                self.eq(lhs.span, lv, s);
                self.eq(rhs.span, rv, s);
                self.bool_var()
            }
            BinopClass::Boolean => {
                let lb = self.bool_var()?;
                self.eq(lhs.span, lv, lb);
                let rb = self.bool_var()?;
                self.eq(rhs.span, rv, rb);
                self.bool_var()
            }
            BinopClass::Append => {
                // `++` is `String -> String -> String`: both operands and the
                // result are pinned to `String`. A non-String operand (a
                // would-be `List`) fails to unify with `String` and surfaces as
                // a type error rather than reaching the backend.
                let ls = self.string_var()?;
                self.eq(lhs.span, lv, ls);
                let rs = self.string_var()?;
                self.eq(rhs.span, rv, rs);
                self.string_var()
            }
            BinopClass::Poly => {
                // `a -> a -> a`: operands and result share one type.
                self.eq(rhs.span, lv, rv);
                Ok(lv)
            }
        }
    }

    fn con_var(&mut self, module: Vec<Symbol>, name: Symbol, args: Vec<VarId>) -> DResult<VarId> {
        self.structure(FlatType::Con { module, name, args })
    }

    /// A `List elem` type variable over the element variable `elem`. The built-in
    /// `List` carries an empty module path, matching the other builtins.
    fn list_var(&mut self, elem: VarId) -> DResult<VarId> {
        let name = self.builtins.list;
        self.con_var(Vec::new(), name, vec![elem])
    }

    /// Constrain a list literal `[]` / `[a, b, c]`: every element shares one
    /// element variable, and the whole expression is the `List` over it. An empty
    /// list leaves the element variable flexible (inferred from context, else
    /// numeric-defaulted like any unpinned variable). Returns the result variable.
    fn constrain_list(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        elems: &[canon::Expr],
    ) -> DResult<VarId> {
        let elem = self.flex()?;
        for e in elems {
            let ev = self.constrain_expr(local, e)?;
            self.eq(e.span, ev, elem);
        }
        self.list_var(elem)
    }

    /// Constrain a cons `head :: tail`: `head : elem`, `tail : List elem`, result
    /// `List elem`. Imposing the `a -> List a -> List a` discipline directly makes
    /// a non-list tail or a mismatched element a type error, not a backend crash.
    fn constrain_cons(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        head: &canon::Expr,
        tail: &canon::Expr,
    ) -> DResult<VarId> {
        let elem = self.constrain_expr(local, head)?;
        let list = self.list_var(elem)?;
        let tail_var = self.constrain_expr(local, tail)?;
        self.eq(tail.span, tail_var, list);
        Ok(list)
    }

    fn eq(&mut self, span: Span, lhs: VarId, rhs: VarId) {
        self.constraints.push(Constraint { span, lhs, rhs });
    }

    // ── Ty ⇄ solver bridges ────────────────────────────────────────────────

    /// Instantiate a resolved [`Ty`] into fresh union-find structure, with every
    /// type variable replaced by a fresh **flexible** variable.
    ///
    /// This is the per-call-site instantiation (the Haskell `CForeign` path):
    /// each reference to a polymorphic top-level binding alpha-renames the
    /// binding's scheme into fresh flex variables, so the call unifies against the
    /// concrete argument types at *this* site without pinning the binding's other
    /// uses. Type variables alpha-rename consistently *within this call* via a
    /// fresh `vars` map (`a -> a` becomes `f -> f`, one shared flex), so calling
    /// `identity` at `Int` and at `Bool` in the same module yields two
    /// independent, separately-satisfiable instantiations.
    fn instantiate(&mut self, ty: &Ty) -> DResult<VarId> {
        let (var, _vars) = self.instantiate_tracked(ty)?;
        Ok(var)
    }

    /// [`Self::instantiate`], additionally returning the alpha-renaming map
    /// (scheme type-variable raw id → fresh variable). The map lets a use site be
    /// checked post-solve against the binding's super-type obligations: each
    /// obligated scheme variable's fresh variable reveals the concrete type this
    /// use pinned it to.
    fn instantiate_tracked(&mut self, ty: &Ty) -> DResult<(VarId, BTreeMap<u32, VarId>)> {
        let mut vars = BTreeMap::new();
        let var = self.instantiate_in(ty, &mut vars, /* rigid */ false)?;
        Ok((var, vars))
    }

    /// Instantiate a constructor scheme through one shared variable map, returning
    /// the fresh variables of its payload fields and of its result enum type.
    /// Sharing the map keeps a generic constructor's field and result variables
    /// linked at this use site (`Just : a -> Maybe a` instantiated at `a = Int`
    /// ties the payload to the result), exactly like [`Self::instantiate`] over the
    /// equivalent arrow — but decomposed, so a pattern can bind each field and a
    /// value reference can rebuild the arrow.
    fn instantiate_ctor(&mut self, scheme: &CtorScheme) -> DResult<(Vec<VarId>, VarId)> {
        let mut vars = BTreeMap::new();
        let mut arg_vars = Vec::with_capacity(scheme.arg_tys.len());
        for t in &scheme.arg_tys {
            arg_vars.push(self.instantiate_in(t, &mut vars, /* rigid */ false)?);
        }
        let result_var = self.instantiate_in(&scheme.result, &mut vars, /* rigid */ false)?;
        Ok((arg_vars, result_var))
    }

    /// Instantiate a resolved [`Ty`] with every type variable replaced by a fresh
    /// **rigid** (skolem) variable, sharing `vars` across the call so repeated
    /// occurrences of one annotation variable map to one rigid node.
    ///
    /// Used to seed a typed binding's parameters + return when checking its body:
    /// the whole signature is instantiated through *one* `vars` map so `a` is the
    /// same rigid everywhere it appears, and distinct annotation variables become
    /// distinct rigids that the body cannot conflate ([`Content::Rigid`]).
    fn instantiate_rigid(&mut self, ty: &Ty, vars: &mut BTreeMap<u32, VarId>) -> DResult<VarId> {
        self.instantiate_in(ty, vars, /* rigid */ true)
    }

    fn instantiate_in(
        &mut self,
        ty: &Ty,
        vars: &mut BTreeMap<u32, VarId>,
        rigid: bool,
    ) -> DResult<VarId> {
        match ty {
            Ty::Unit => self.structure(FlatType::Unit),
            Ty::Tuple(elems) => {
                let mut elem_vars = Vec::with_capacity(elems.len());
                for e in elems {
                    elem_vars.push(self.instantiate_in(e, vars, rigid)?);
                }
                self.structure(FlatType::Tuple(elem_vars))
            }
            Ty::Record(fields) => {
                let mut field_vars = BTreeMap::new();
                for (name, field_ty) in fields {
                    let v = self.instantiate_in(field_ty, vars, rigid)?;
                    field_vars.insert(*name, v);
                }
                self.structure(FlatType::Record(field_vars))
            }
            Ty::Var(id) => {
                if let Some(v) = vars.get(id).copied() {
                    return Ok(v);
                }
                let v = if rigid { self.rigid()? } else { self.flex()? };
                vars.insert(*id, v);
                Ok(v)
            }
            Ty::Fun(a, b) => {
                let av = self.instantiate_in(a, vars, rigid)?;
                let bv = self.instantiate_in(b, vars, rigid)?;
                self.structure(FlatType::Fun(av, bv))
            }
            Ty::Con { module, name, args } => {
                let mut arg_vars = Vec::with_capacity(args.len());
                for a in args {
                    arg_vars.push(self.instantiate_in(a, vars, rigid)?);
                }
                self.structure(FlatType::Con {
                    module: module.clone(),
                    name: *name,
                    args: arg_vars,
                })
            }
        }
    }

    // ── the walk ────────────────────────────────────────────────────────────

    fn constrain_def(&mut self, def: &canon::Def) -> DResult<()> {
        match def {
            canon::Def::Typed {
                name,
                patterns,
                body,
                ty,
                free_vars,
            } => {
                // Instantiate the WHOLE signature through one shared map so every
                // occurrence of an annotation variable (`a` in `a -> a`) becomes
                // the *same* rigid (skolem) node, and distinct variables become
                // distinct rigids. Checking the body against rigids is what makes
                // the annotation a genuine contract: `f : a -> a; f x = x + 1`
                // (body pins `a` to `Int`) and `f : a -> b; f x = x` (body
                // conflates `a` and `b`) are both mismatches rather than silently
                // accepted. Per-call-site uses instead instantiate the binding's
                // type as fresh *flex* variables (see [`Self::instantiate`]).
                let mut rigid_vars = BTreeMap::new();
                let mut local = BTreeMap::new();
                let mut cursor = ty;
                for pat in patterns {
                    let (arg_ty, rest) = match cursor {
                        canon::Type::Lambda(a, b) => (a.as_ref(), b.as_ref()),
                        // The binding writes more parameter patterns than its
                        // annotation has arrows (`f a b = …` with `f : Int`).
                        // Parse-don't-validate: surface a user-facing
                        // SKY-T0004 with the binding span + the written
                        // signature, not a CompilerBug.
                        _ => return Err(self.too_many_parameters(name, ty)),
                    };
                    let arg = from_canon(arg_ty);
                    let arg_var = self.instantiate_rigid(&arg, &mut rigid_vars)?;
                    self.constrain_pattern(&mut local, pat, arg_var)?;
                    cursor = rest;
                }
                let ret_ty = from_canon(cursor);
                let ret_var = self.instantiate_rigid(&ret_ty, &mut rigid_vars)?;
                let body_var = self.constrain_expr(&local, body)?;
                self.eq(body.span, body_var, ret_var);
                // Record the skolem each annotation variable instantiated to, so
                // its body-imposed super-type obligations can be read back for
                // generalisation. Keyed by the variable's symbol (the lowerer's
                // `free_vars` are these same symbols).
                let mut var_rigids = BTreeMap::new();
                for fv in free_vars {
                    if let Some(rigid) = rigid_vars.get(&fv.as_raw()) {
                        var_rigids.insert(*fv, *rigid);
                    }
                }
                self.typed_rigids.push((name.value, var_rigids));
                Ok(())
            }
            canon::Def::Untyped {
                name,
                patterns,
                body,
            } => {
                let mut local = BTreeMap::new();
                let mut param_vars = Vec::with_capacity(patterns.len());
                for pat in patterns {
                    let v = self.flex()?;
                    self.constrain_pattern(&mut local, pat, v)?;
                    param_vars.push(v);
                }
                let body_var = self.constrain_expr(&local, body)?;
                // Reconstruct the binding's full type as the right-nested arrow
                // `p0 -> p1 -> … -> body`, so `env[f]` for `f a b = a` is
                // `a -> b -> a`, not just the body's type. A binding with no
                // parameters is just its body's type.
                let mut arrow = body_var;
                for pv in param_vars.into_iter().rev() {
                    arrow = self.structure(FlatType::Fun(pv, arrow))?;
                }
                // Tie the reconstructed type to the shared variable minted in the
                // registration pass, which every reference resolves to.
                let Some(shared) = self.untyped.get(&name.value).copied() else {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: format!(
                            "untyped binding `{}` was not registered",
                            self.interner.resolve(name.value).unwrap_or("<unknown>")
                        ),
                    });
                };
                self.eq(name.span, arrow, shared);
                Ok(())
            }
        }
    }

    /// Build the SKY-T0004 diagnostic for a binding with more parameter
    /// patterns than its annotation has arrows. Resolving the name / rendering
    /// the signature can itself only fail on a forged symbol, in which case
    /// that internal bug is surfaced instead.
    fn too_many_parameters(
        &self,
        name: &sky_diagnostics::Located<Symbol>,
        ty: &canon::Type,
    ) -> Diagnostic {
        let binding = match self.interner.resolve(name.value) {
            Some(s) => Box::from(s),
            None => {
                return Diagnostic::CompilerBug {
                    where_: "intern.resolve",
                    detail: format!("no backing string for symbol {}", name.value.as_raw()),
                };
            }
        };
        match canon_type_to_doc(ty, self.interner) {
            Ok(signature) => Diagnostic::Type {
                span: name.span,
                msg: TypeError::TooManyParameters {
                    binding,
                    signature: Box::new(signature),
                },
            },
            Err(bug) => bug,
        }
    }

    /// Constrain a reference to a top-level binding. A typed binding is
    /// instantiated fresh (flex) at this use site so it unifies against its own
    /// concrete arguments without pinning the binding's other call sites, and the
    /// alpha-renaming map is recorded for the post-solve super-type obligation
    /// check. An untyped binding resolves to its shared monomorphic variable; a
    /// name that is not a binding of this module stays fully flexible.
    fn constrain_var_top_level(&mut self, name: Symbol, span: Span) -> DResult<VarId> {
        if let Some(ty) = self.top_level.get(&name).cloned() {
            let (var, vars) = self.instantiate_tracked(&ty)?;
            self.scheme_apps.push(SchemeApp { name, vars, span });
            Ok(var)
        } else if let Some(v) = self.untyped.get(&name).copied() {
            Ok(v)
        } else {
            self.flex()
        }
    }

    /// The type of a kernel reference (`Math.min`, `Set.insert`, …).
    ///
    /// Most kernels take the declarative scheme from [`Self::kernel_ty`] verbatim
    /// via `instantiate`. Two families instead mint super-typed obligations so a
    /// generic use lifts the matching Rust trait bound onto its annotation
    /// skolem and a non-comparable argument fails closed at type-check:
    ///
    /// * `Math.min` / `Math.max` — `Comparable a => a -> a -> a`: the shared
    ///   variable carries the ORDERING obligation, exactly as the `< > <= >=`
    ///   operators and the user-fn `maxOf` do, so a generic use emits Rust
    ///   `T: PartialOrd` and a function / record argument is rejected rather than
    ///   emitting an unbounded `math_min<T>(…)` that `cargo` rejects.
    /// * `Set` / `Dict` kernels — the element / key (raw scheme-variable 0 in
    ///   every Set / Dict kernel) carries the Sky `comparable`-key obligation
    ///   ([`key_obligation`]). The scheme is instantiated, then variable 0 is
    ///   tied to a fresh super-typed variable carrying that obligation, so a
    ///   non-comparable element / key (record, ADT, function) fails closed
    ///   instead of emitting an unbounded `set_insert::<T>` / `dict_insert::<T>`
    ///   call `cargo` rejects, and a generic `a -> Set a` lifts `Ord` (Set) /
    ///   `Hash + Eq + Ord` (Dict) onto its annotation skolem (see `bounds_for`).
    ///   This is also more conservative than Sky's runtime, which keys a Set /
    ///   Dict on a stringified value.
    fn constrain_var_kernel(&mut self, module: Symbol, name: Symbol, span: Span) -> DResult<VarId> {
        if matches!(self.interner.resolve(module), Some("Math"))
            && matches!(self.interner.resolve(name), Some("min" | "max"))
        {
            let s = self.super_var(TyBounds::ord(), span)?;
            let inner = self.structure(FlatType::Fun(s, s))?;
            return self.structure(FlatType::Fun(s, inner));
        }
        if let Some(bound) = key_obligation(self.interner, module) {
            let ty = self.kernel_ty(module, name);
            let (var, vars) = self.instantiate_tracked(&ty)?;
            if let Some(&key_var) = vars.get(&0) {
                let s = self.super_var(bound, span)?;
                self.eq(span, key_var, s);
            }
            return Ok(var);
        }
        let ty = self.kernel_ty(module, name);
        self.instantiate(&ty)
    }

    fn constrain_expr(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        e: &canon::Expr,
    ) -> DResult<VarId> {
        let span = e.span;
        let var = match &e.value {
            canon::Expr_::Int(_) => self.int_var()?,
            canon::Expr_::Float(_) => self.float_var()?,
            canon::Expr_::Str(_) => self.string_var()?,
            canon::Expr_::Char(_) => self.char_var()?,
            canon::Expr_::Unit => self.structure(FlatType::Unit)?,
            canon::Expr_::VarLocal(s) => match local.get(s) {
                Some(v) => *v,
                None => {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: format!(
                            "unbound local `{}`",
                            self.interner.resolve(*s).unwrap_or("<unknown symbol>")
                        ),
                    });
                }
            },
            canon::Expr_::VarTopLevel { name, .. } => self.constrain_var_top_level(*name, span)?,
            canon::Expr_::VarKernel { module, name } => {
                self.constrain_var_kernel(*module, *name, span)?
            }
            canon::Expr_::VarCtor {
                home,
                type_name,
                name,
                ..
            } => self.constrain_var_ctor(home, *type_name, *name)?,
            canon::Expr_::Call(callee, args) => {
                let callee_var = self.constrain_expr(local, callee)?;
                let mut arg_vars = Vec::with_capacity(args.len());
                for a in args {
                    arg_vars.push(self.constrain_expr(local, a)?);
                }
                let ret = self.flex()?;
                // Fold a right-associative arrow: a0 -> a1 -> … -> ret.
                let mut expected = ret;
                for av in arg_vars.into_iter().rev() {
                    expected = self.structure(FlatType::Fun(av, expected))?;
                }
                self.eq(callee.span, callee_var, expected);
                ret
            }
            canon::Expr_::Case(scrut, branches) => self.constrain_case(local, scrut, branches)?,
            canon::Expr_::Lambda(params, body) => self.constrain_lambda(local, params, body)?,
            canon::Expr_::Binop { func, lhs, rhs, .. } => {
                self.constrain_binop(local, *func, lhs, rhs)?
            }
            canon::Expr_::Let(bindings, body) => {
                // Sequential, monomorphic `let`: each binding's value is
                // constrained against the scope built so far, and its name binds
                // to that value's variable for the bindings that follow and the
                // `in` body. The whole `let`'s type is the body's type. (M1 does
                // not generalise let-bound names — no let-polymorphism.)
                let mut let_local = local.clone();
                for b in bindings {
                    let bv = self.constrain_expr(&let_local, &b.body)?;
                    // The binder may be a plain name or an irrefutable destructure
                    // (tuple / record); `constrain_pattern` ties the binder's
                    // shape to the value's type and binds every leaf variable.
                    self.constrain_pattern(&mut let_local, &b.pat, bv)?;
                }
                self.constrain_expr(&let_local, body)?
            }
            canon::Expr_::If(branches, else_expr) => {
                // Every condition is `Bool`; every branch and the final `else`
                // unify to one shared result type, which is the whole `if`'s
                // type. Mirrors `Sky.Type.Constrain.Expression.constrainIf`.
                let result = self.flex()?;
                for (cond, body) in branches {
                    let cond_var = self.constrain_expr(local, cond)?;
                    let want_bool = self.bool_var()?;
                    self.eq(cond.span, cond_var, want_bool);
                    let body_var = self.constrain_expr(local, body)?;
                    self.eq(body.span, body_var, result);
                }
                let else_var = self.constrain_expr(local, else_expr)?;
                self.eq(else_expr.span, else_var, result);
                result
            }
            canon::Expr_::Tuple(elems) => {
                // A tuple's type is the product of its elements' types, each
                // constrained independently. Mirrors
                // `Sky.Type.Constrain.Expression`'s tuple arm.
                let mut elem_vars = Vec::with_capacity(elems.len());
                for elem in elems {
                    elem_vars.push(self.constrain_expr(local, elem)?);
                }
                self.structure(FlatType::Tuple(elem_vars))?
            }
            canon::Expr_::List(elems) => self.constrain_list(local, elems)?,
            canon::Expr_::Cons(head, tail) => self.constrain_cons(local, head, tail)?,
            canon::Expr_::Record(fields) => self.constrain_record(local, fields)?,
            canon::Expr_::Access(record, field) => {
                self.constrain_access(local, record, *field, span)?
            }
            canon::Expr_::Update(base, fields) => {
                self.constrain_update(local, base, fields, span)?
            }
        };
        self.regions.insert(span, var);
        Ok(var)
    }

    /// Constrain a lambda `\p0 p1 ... -> body`. Each parameter gets a fresh
    /// flexible variable bound in the body's scope; the body is constrained
    /// there. The lambda's type is the right-nested arrow `p0 -> p1 -> … -> body`,
    /// so a surrounding `Call` unifies its callee against exactly this shape.
    /// Mirrors `Sky.Type.Constrain.Expression`'s lambda arm.
    fn constrain_lambda(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        params: &[canon::Pattern],
        body: &canon::Expr,
    ) -> DResult<VarId> {
        let mut lam_local = local.clone();
        let mut param_vars = Vec::with_capacity(params.len());
        for p in params {
            let v = self.flex()?;
            self.constrain_pattern(&mut lam_local, p, v)?;
            param_vars.push(v);
        }
        let mut arrow = self.constrain_expr(&lam_local, body)?;
        for pv in param_vars.into_iter().rev() {
            arrow = self.structure(FlatType::Fun(pv, arrow))?;
        }
        Ok(arrow)
    }

    /// Constrain a record literal `{ name = value, ... }`. Its type is the
    /// closed record `{ name : <field type>, ... }`, each field value
    /// constrained independently. Canonicalisation has already rejected a
    /// duplicate field name, so the resulting field map is exact.
    fn constrain_record(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        fields: &[(Symbol, canon::Expr)],
    ) -> DResult<VarId> {
        let mut field_vars = BTreeMap::new();
        for (name, value) in fields {
            let v = self.constrain_expr(local, value)?;
            field_vars.insert(*name, v);
        }
        self.structure(FlatType::Record(field_vars))
    }

    /// Constrain a record field access `record.field`. With closed records (no
    /// row variable), the field cannot be resolved until the record's type
    /// settles, so the access is deferred: a fresh result variable is its region
    /// type now, and [`crate::resolve_field_accesses`] links it to the field's
    /// type after the main solve.
    fn constrain_access(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        record: &canon::Expr,
        field: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        let record_var = self.constrain_expr(local, record)?;
        let result = self.flex()?;
        self.field_accesses.push(FieldAccess {
            record: record_var,
            field,
            result,
            span,
        });
        Ok(result)
    }

    /// Constrain a record update `{ base | field = value, ... }`. The result
    /// type is the base record's type (an update copies-and-replaces, changing
    /// no field's type), so the update's region variable *is* the base's. The
    /// field-existence + per-field type checks are deferred — closed records
    /// carry no row variable, so the base's type may not be settled yet —
    /// recorded here and discharged by [`crate::resolve_record_updates`] after
    /// the main solve.
    fn constrain_update(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        base: &canon::Expr,
        fields: &[(Symbol, canon::Expr)],
        span: Span,
    ) -> DResult<VarId> {
        let record_var = self.constrain_expr(local, base)?;
        let mut field_vars = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            let v = self.constrain_expr(local, value)?;
            field_vars.push((*name, v));
        }
        self.record_updates.push(RecordUpdate {
            record: record_var,
            fields: field_vars,
            span,
        });
        Ok(record_var)
    }

    /// Constrain a `case scrut of …`: the scrutinee shares one type, every arm
    /// pattern is checked against it, and every arm body unifies to one shared
    /// result — the whole `case`'s type.
    fn constrain_case(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        scrut: &canon::Expr,
        branches: &[canon::CaseBranch],
    ) -> DResult<VarId> {
        let scrut_var = self.constrain_expr(local, scrut)?;
        let result = self.flex()?;
        for br in branches {
            let mut br_local = local.clone();
            self.constrain_pattern(&mut br_local, &br.pat, scrut_var)?;
            let body_var = self.constrain_expr(&br_local, &br.body)?;
            self.eq(br.body.span, body_var, result);
        }
        Ok(result)
    }

    /// Constrain a constructor referenced as a value: its scheme instantiated
    /// fresh. A nullary constructor's value type is the enum itself; a payload
    /// constructor's is the curried arrow `field0 -> … -> T vars`. Each reference
    /// instantiates independently, so the same generic constructor used at `Int`
    /// and at `Bool` in one module yields two separately-satisfiable types. A
    /// constructor with no registered scheme (imported, outside the single-module
    /// subset) falls back to the bare enum type, sound for the nullary case.
    fn constrain_var_ctor(
        &mut self,
        home: &[Symbol],
        type_name: Symbol,
        name: Symbol,
    ) -> DResult<VarId> {
        if let Some(scheme) = self.ctors.get(&name).cloned() {
            let (arg_vars, result_var) = self.instantiate_ctor(&scheme)?;
            let mut t = result_var;
            for av in arg_vars.into_iter().rev() {
                t = self.structure(FlatType::Fun(av, t))?;
            }
            Ok(t)
        } else {
            self.con_var(home.to_vec(), type_name, Vec::new())
        }
    }

    /// Constrain a `case` arm pattern against the scrutinee's variable, binding
    /// any pattern variables into `local`.
    fn constrain_pattern(
        &mut self,
        local: &mut BTreeMap<Symbol, VarId>,
        pat: &canon::Pattern,
        scrut_var: VarId,
    ) -> DResult<()> {
        match &pat.value {
            canon::Pattern_::PAnything => Ok(()),
            canon::Pattern_::PVar(s) => {
                local.insert(*s, scrut_var);
                Ok(())
            }
            canon::Pattern_::PCtor {
                home,
                type_name,
                name,
                args,
                ..
            } => {
                if let Some(scheme) = self.ctors.get(name).cloned() {
                    // A constructor pattern binds exactly its declared fields. A
                    // mismatch (`Just` with no payload, `Node l r` for a three-field
                    // `Node`) is a user error, surfaced as SKY-T0013 rather than
                    // silently constraining a prefix.
                    if args.len() != scheme.arg_tys.len() {
                        return Err(self.ctor_pattern_arity(
                            pat.span,
                            *name,
                            scheme.arg_tys.len(),
                            args.len(),
                        ));
                    }
                    // Instantiate the scheme fresh, tie the result to the
                    // scrutinee, and constrain each payload sub-pattern against its
                    // field's (now use-site) type. Recursing handles a nested
                    // sub-pattern's typing too; the lowerer is what restricts M3a
                    // payloads to variables / wildcards.
                    let (arg_vars, result_var) = self.instantiate_ctor(&scheme)?;
                    self.eq(pat.span, result_var, scrut_var);
                    for (sub, av) in args.iter().zip(arg_vars) {
                        self.constrain_pattern(local, sub, av)?;
                    }
                } else {
                    // A constructor with no registered scheme (imported, outside the
                    // single-module subset): fall back to the bare enum type, sound
                    // for the nullary case.
                    let ctor = self.con_var(home.clone(), *type_name, Vec::new())?;
                    self.eq(pat.span, ctor, scrut_var);
                }
                Ok(())
            }
            canon::Pattern_::PTuple(elems) => {
                // A tuple pattern matches a Tuple type element-wise: mint one
                // fresh variable per element, tie the scrutinee to the product
                // over them, and constrain each sub-pattern against its element's
                // variable. Nested sub-patterns recurse; the lowerer restricts
                // which element shapes it can actually emit.
                let mut elem_vars = Vec::with_capacity(elems.len());
                for _ in elems {
                    elem_vars.push(self.flex()?);
                }
                let tuple = self.structure(FlatType::Tuple(elem_vars.clone()))?;
                self.eq(pat.span, tuple, scrut_var);
                for (sub, ev) in elems.iter().zip(elem_vars) {
                    self.constrain_pattern(local, sub, ev)?;
                }
                Ok(())
            }
            canon::Pattern_::PRecord(fields) => {
                // A field-pun record pattern `{ x, y }` binds each named field of
                // the scrutinee record. Closed records carry no row variable, so
                // the scrutinee's full field set may not be settled here; instead
                // of forcing an exact-shape unification (which would reject the
                // legal subset pattern `{ x }` on a `{ x, y }` record), each
                // field is pulled out with the SAME deferred field-access channel
                // a `record.field` expression uses. After the main solve,
                // `resolve_field_accesses` links each binder to the field's type.
                for f in fields {
                    let result = self.flex()?;
                    self.field_accesses.push(FieldAccess {
                        record: scrut_var,
                        field: f.value,
                        result,
                        span: f.span,
                    });
                    local.insert(f.value, result);
                }
                Ok(())
            }
            // A literal pattern pins the scrutinee to the literal's type. It
            // binds no names. A mismatch (`case n of "x" -> …` with `n : Int`)
            // surfaces as the ordinary SKY-T0001 type mismatch.
            canon::Pattern_::PInt(_) => {
                let lit = self.int_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PBool(_) => {
                let lit = self.bool_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PChar(_) => {
                let lit = self.char_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PStr(_) => {
                let lit = self.string_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            // An alias `inner as name` binds `name` to the whole scrutinee and
            // additionally constrains the inner pattern against it.
            canon::Pattern_::PAlias(inner, name) => {
                local.insert(name.value, scrut_var);
                self.constrain_pattern(local, inner, scrut_var)
            }
            // A list pattern `[a, b]` matches a `List elem`: each element
            // sub-pattern is constrained against one shared element variable, and
            // the scrutinee is tied to the list over it.
            canon::Pattern_::PList(elems) => {
                let elem = self.flex()?;
                let list = self.list_var(elem)?;
                self.eq(pat.span, list, scrut_var);
                for sub in elems {
                    self.constrain_pattern(local, sub, elem)?;
                }
                Ok(())
            }
            // A cons pattern `head :: tail` matches a `List elem`: `head : elem`,
            // `tail : List elem` (the scrutinee's own type), scrutinee `List elem`.
            canon::Pattern_::PCons(head, tail) => {
                let elem = self.flex()?;
                let list = self.list_var(elem)?;
                self.eq(pat.span, list, scrut_var);
                self.constrain_pattern(local, head, elem)?;
                self.constrain_pattern(local, tail, list)
            }
        }
    }

    /// Build the SKY-T0013 diagnostic for a constructor pattern that binds the
    /// wrong number of payload fields. A forged constructor symbol surfaces the
    /// underlying intern bug instead.
    fn ctor_pattern_arity(
        &self,
        span: Span,
        ctor: Symbol,
        expected: usize,
        found: usize,
    ) -> Diagnostic {
        self.interner.resolve(ctor).map_or_else(
            || Diagnostic::CompilerBug {
                where_: "intern.resolve",
                detail: format!("no backing string for constructor symbol {}", ctor.as_raw()),
            },
            |s| Diagnostic::Type {
                span,
                msg: TypeError::CtorPatternArity {
                    ctor: Box::from(s),
                    expected,
                    found,
                },
            },
        )
    }

    /// The type of a kernel function. The wired set is `String.fromInt :
    /// Int -> String`, `String.fromFloat : Float -> String`, and `Log.println :
    /// String -> Task ()`; any other kernel is treated as fully polymorphic so
    /// it never spuriously fails inference for the supported subset.
    #[allow(clippy::too_many_lines)] // declarative kernel-type table — extracting helpers would obscure the data
    fn kernel_ty(&self, module: Symbol, name: Symbol) -> Ty {
        let int = Ty::Con {
            module: Vec::new(),
            name: self.builtins.int,
            args: Vec::new(),
        };
        let float = Ty::Con {
            module: Vec::new(),
            name: self.builtins.float,
            args: Vec::new(),
        };
        let string = Ty::Con {
            module: Vec::new(),
            name: self.builtins.string,
            args: Vec::new(),
        };
        let task_unit = Ty::Con {
            module: Vec::new(),
            name: self.builtins.task,
            args: vec![Ty::Unit],
        };
        let bool_ty = Ty::Con {
            module: Vec::new(),
            name: self.builtins.bool,
            args: Vec::new(),
        };
        // The polymorphic kernel schemes below use `Ty::Var(n)` for their type
        // variables. `instantiate` mints one fresh flexible variable per distinct
        // raw id, sharing it across every occurrence within ONE scheme — so the
        // ids only need to be distinct within a single arm (they are local to that
        // arm's instantiation), exactly like a constructor scheme's variables.
        let var = Ty::Var;
        let fun = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b));
        let list = |t: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.list,
            args: vec![t],
        };
        let maybe = |t: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.maybe,
            args: vec![t],
        };
        let result = |e: Ty, a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.result,
            args: vec![e, a],
        };
        let dict = |k: Ty, v: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.dict,
            args: vec![k, v],
        };
        let set = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.set,
            args: vec![a],
        };
        let tuple2 = |a: Ty, b: Ty| Ty::Tuple(vec![a, b]);
        match (self.interner.resolve(module), self.interner.resolve(name)) {
            (Some("String"), Some("fromInt")) => Ty::Fun(Box::new(int), Box::new(string)),
            (Some("String"), Some("fromFloat")) => Ty::Fun(Box::new(float), Box::new(string)),
            (Some("Log"), Some("println")) => Ty::Fun(Box::new(string), Box::new(task_unit)),

            // ── Sky.Core.List (kernel-anchored combinators) ──
            // map : (a -> b) -> List a -> List b
            (Some("List"), Some("map")) => {
                fun(fun(var(0), var(1)), fun(list(var(0)), list(var(1))))
            }
            // filter : (a -> Bool) -> List a -> List a
            (Some("List"), Some("filter")) => {
                fun(fun(var(0), bool_ty), fun(list(var(0)), list(var(0))))
            }
            // foldl / foldr : (a -> b -> b) -> b -> List a -> b
            (Some("List"), Some("foldl" | "foldr")) => fun(
                fun(var(0), fun(var(1), var(1))),
                fun(var(1), fun(list(var(0)), var(1))),
            ),
            // length : List a -> Int
            (Some("List"), Some("length")) => fun(list(var(0)), int),
            // head : List a -> Maybe a
            (Some("List"), Some("head")) => fun(list(var(0)), maybe(var(0))),
            // tail : List a -> Maybe (List a)
            (Some("List"), Some("tail")) => fun(list(var(0)), maybe(list(var(0)))),
            // member : a -> List a -> Bool
            (Some("List"), Some("member")) => fun(var(0), fun(list(var(0)), bool_ty)),
            // range : Int -> Int -> List Int
            (Some("List"), Some("range")) => fun(int.clone(), fun(int.clone(), list(int))),
            // reverse : List a -> List a
            (Some("List"), Some("reverse")) => fun(list(var(0)), list(var(0))),

            // ── Sky.Core.Maybe ──
            // withDefault : a -> Maybe a -> a
            (Some("Maybe"), Some("withDefault")) => fun(var(0), fun(maybe(var(0)), var(0))),
            // map : (a -> b) -> Maybe a -> Maybe b
            (Some("Maybe"), Some("map")) => {
                fun(fun(var(0), var(1)), fun(maybe(var(0)), maybe(var(1))))
            }
            // andThen : (a -> Maybe b) -> Maybe a -> Maybe b
            (Some("Maybe"), Some("andThen")) => fun(
                fun(var(0), maybe(var(1))),
                fun(maybe(var(0)), maybe(var(1))),
            ),

            // ── Sky.Core.Result ── (e = the error type variable)
            // withDefault : a -> Result e a -> a
            (Some("Result"), Some("withDefault")) => {
                fun(var(0), fun(result(var(1), var(0)), var(0)))
            }
            // map : (a -> b) -> Result e a -> Result e b
            (Some("Result"), Some("map")) => fun(
                fun(var(0), var(1)),
                fun(result(var(2), var(0)), result(var(2), var(1))),
            ),

            // ── Sky.Core.Math ──
            // NOTE: `Math.min` / `Math.max` do NOT use this arm — they are handled
            // on a dedicated path in the `VarKernel` walk that mints the shared
            // variable with the ORDERING obligation (`Comparable a => a -> a -> a`,
            // Elm Basics-conformant). This bare `var(0)` table entry would emit an
            // UNBOUNDED variable, which lowers to a `math_min<T>(…)` call that
            // `cargo` rejects (the runtime helper requires `T: PartialOrd`); the
            // bounded path fails closed at type-check on non-comparable arguments
            // instead. Kept only as a safety net should the dedicated path ever be
            // bypassed; it is unreachable in normal lowering. The no-truncation /
            // type-preserving behaviour (Divergence from Sky, PR #136 — Sky
            // routes through AsInt; we follow Elm's polymorphic comparable;
            // rationale: Elm-conformance) is a property of the runtime compare
            // the bounded variable lowers to.
            (Some("Math"), Some("min" | "max")) => fun(var(0), fun(var(0), var(0))),
            // Constants — bare Float values (arity 0).
            (Some("Math"), Some("pi" | "e" | "phi" | "sqrt2" | "inf" | "nan")) => float,
            // abs : Int -> Int.
            (Some("Math"), Some("abs")) => fun(int.clone(), int),
            // Arity-1 Float -> Float.
            (
                Some("Math"),
                Some(
                    "sqrt" | "cbrt" | "exp" | "exp2" | "log" | "log2" | "log10" | "sin" | "cos"
                    | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "asinh"
                    | "acosh" | "atanh",
                ),
            ) => fun(float.clone(), float),
            // Arity-1 Float -> Int (rounding functions).
            (Some("Math"), Some("floor" | "ceil" | "round" | "trunc")) => fun(float, int),
            // Arity-2 Float -> Float -> Float.
            (Some("Math"), Some("pow" | "hypot" | "atan2" | "mod" | "remainder")) => {
                fun(float.clone(), fun(float.clone(), float))
            }

            // ── Sky.Core.Dict (M4d) ──
            // NOTE: the key variable `var(0)` in every Dict arm below is written
            // bare here, but the `VarKernel` walk does NOT take this scheme as-is
            // for `Dict` kernels: it instantiates the scheme and then ties raw
            // scheme-variable 0 (the key) to a fresh super-typed variable
            // carrying the Sky `comparable`-key obligation (`TyBounds::dict_key`,
            // → Rust `Hash + Eq + Ord`). So a non-comparable key fails closed at
            // type-check, and a generic key lifts the bound onto the annotation
            // skolem rather than emitting an unbounded `dict_*::<T>` call `cargo`
            // rejects. The bare scheme is the SHAPE; the obligation is attached
            // on the dedicated path (see `key_obligation` + the `VarKernel` arm).
            // empty : Dict k v  — arity-0 polymorphic value.
            (Some("Dict"), Some("empty")) => dict(var(0), var(1)),
            // isEmpty : Dict k v -> Bool
            (Some("Dict"), Some("isEmpty")) => fun(dict(var(0), var(1)), bool_ty),
            // size : Dict k v -> Int
            (Some("Dict"), Some("size")) => fun(dict(var(0), var(1)), int),
            // insert : k -> v -> Dict k v -> Dict k v
            (Some("Dict"), Some("insert")) => fun(
                var(0),
                fun(var(1), fun(dict(var(0), var(1)), dict(var(0), var(1)))),
            ),
            // get : k -> Dict k v -> Maybe v
            (Some("Dict"), Some("get")) => fun(var(0), fun(dict(var(0), var(1)), maybe(var(1)))),
            // remove : k -> Dict k v -> Dict k v
            (Some("Dict"), Some("remove")) => {
                fun(var(0), fun(dict(var(0), var(1)), dict(var(0), var(1))))
            }
            // member : k -> Dict k v -> Bool
            (Some("Dict"), Some("member")) => fun(var(0), fun(dict(var(0), var(1)), bool_ty)),
            // keys : Dict k v -> List k
            (Some("Dict"), Some("keys")) => fun(dict(var(0), var(1)), list(var(0))),
            // values : Dict k v -> List v
            (Some("Dict"), Some("values")) => fun(dict(var(0), var(1)), list(var(1))),
            // toList : Dict k v -> List (k, v)
            (Some("Dict"), Some("toList")) => {
                fun(dict(var(0), var(1)), list(tuple2(var(0), var(1))))
            }
            // fromList : List (k, v) -> Dict k v
            (Some("Dict"), Some("fromList")) => {
                fun(list(tuple2(var(0), var(1))), dict(var(0), var(1)))
            }
            // map : (k -> a -> b) -> Dict k a -> Dict k b
            (Some("Dict"), Some("map")) => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dict(var(0), var(1)), dict(var(0), var(2))),
            ),
            // foldl : (k -> v -> b -> b) -> b -> Dict k v -> b
            (Some("Dict"), Some("foldl")) => fun(
                fun(var(0), fun(var(1), fun(var(2), var(2)))),
                fun(var(2), fun(dict(var(0), var(1)), var(2))),
            ),
            // union : Dict k v -> Dict k v -> Dict k v  (left-biased)
            (Some("Dict"), Some("union")) => fun(
                dict(var(0), var(1)),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),

            // ── Sky.Core.Set (M4d) ──
            // NOTE: the element variable `var(0)` in every Set arm is written
            // bare here; the `VarKernel` walk ties raw scheme-variable 0 (the
            // element) to a fresh super-typed variable carrying the Sky
            // `comparable`-key obligation (`TyBounds::set_elem`, → Rust `Ord`),
            // exactly as the Dict arms above. See `key_obligation`.
            // empty : Set a  — arity-0 polymorphic value.
            (Some("Set"), Some("empty")) => set(var(0)),
            // size : Set a -> Int
            (Some("Set"), Some("size")) => fun(set(var(0)), int),
            // insert : a -> Set a -> Set a
            // remove : a -> Set a -> Set a
            (Some("Set"), Some("insert" | "remove")) => fun(var(0), fun(set(var(0)), set(var(0)))),
            // member : a -> Set a -> Bool
            (Some("Set"), Some("member")) => fun(var(0), fun(set(var(0)), bool_ty)),
            // toList : Set a -> List a
            (Some("Set"), Some("toList")) => fun(set(var(0)), list(var(0))),
            // fromList : List a -> Set a
            (Some("Set"), Some("fromList")) => fun(list(var(0)), set(var(0))),
            // union : Set a -> Set a -> Set a
            // intersect : Set a -> Set a -> Set a
            // diff : Set a -> Set a -> Set a
            (Some("Set"), Some("union" | "intersect" | "diff")) => {
                fun(set(var(0)), fun(set(var(0)), set(var(0))))
            }

            // Unknown kernel: a single flexible variable. The raw id is chosen
            // to be distinct from any real interned symbol's typical range; it
            // only needs to differ between the two `Ty::Var` arms of one
            // instantiate call, which a constant id trivially satisfies.
            _ => Ty::Var(u32::MAX),
        }
    }
}

/// A single step of the iterative [`zonk`] work stack.
///
/// `Visit` reads one union-find node and pushes either a leaf result or the
/// `Build*` task plus its children's `Visit`s; the `Build*` tasks reassemble a
/// parent [`Ty`] once its children's results sit on the result stack.
enum ZonkTask {
    /// Resolve and read back one variable.
    Visit(VarId),
    /// Pop two results (`arg`, then `result`) and push a `Fun`.
    BuildFun,
    /// Pop `arity` results and push a `Con` over them.
    BuildCon {
        module: Vec<Symbol>,
        name: Symbol,
        arity: usize,
    },
    /// Pop `arity` results and push a `Tuple` over them.
    BuildTuple { arity: usize },
    /// Pop one result per field name (in `names` order) and push a `Record`. The
    /// `names` are visited in their `BTreeMap` order, so popping in reverse pairs
    /// each result with its field name.
    BuildRecord { names: Vec<Symbol> },
}

/// Read a settled union-find variable back into a resolved [`Ty`].
///
/// Called after [`crate::solve::solve`] has discharged every constraint. The
/// occurs check in unification guarantees the structure is acyclic, so the node
/// bound is only ever hit on adversarial input.
///
/// **Iterative.** The walk runs over an explicit heap-allocated work stack
/// (mirroring the iterative `find` in `unionfind.rs`), so it never grows the
/// native call stack regardless of how deep the type is. Each node visited
/// ticks the shared [`Budget`] (a DOS bound) and consumes one of
/// [`ZONK_NODE_LIMIT`] per-call nodes (a stack-safety bound on the renderer that
/// later walks the result).
///
/// # Errors
/// [`Diagnostic::CompilerBug`] on a union-find invariant violation or if the
/// structure has more than [`ZONK_NODE_LIMIT`] nodes; [`TypeError::StepBudgetExceeded`]
/// if the shared budget is exhausted.
pub fn zonk(uf: &mut UnionFind<Content>, budget: &mut Budget, var: VarId) -> DResult<Ty> {
    let mut work: Vec<ZonkTask> = vec![ZonkTask::Visit(var)];
    let mut results: Vec<Ty> = Vec::new();
    let mut nodes_left = ZONK_NODE_LIMIT;

    while let Some(task) = work.pop() {
        match task {
            ZonkTask::Visit(v) => {
                budget.tick()?;
                nodes_left = nodes_left
                    .checked_sub(1)
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: "type exceeded read-back node limit".to_owned(),
                    })?;
                let root = uf.find(v)?;
                match uf.content(root)? {
                    // A flexible, rigid, or super-typed variable that survives
                    // solving reads back as a type variable named by its
                    // representative's id. (A super-typed variable is still a
                    // variable; its obligations are read separately when
                    // generalising — see [`crate::SolvedTypes::bounds`].)
                    Content::Flex | Content::Rigid | Content::Super { .. } => {
                        results.push(Ty::Var(root));
                    }
                    Content::Structure(FlatType::Unit) => results.push(Ty::Unit),
                    Content::Structure(FlatType::Fun(a, b)) => {
                        // Push the rebuild first, then the children so that `a`
                        // is visited before `b` and lands lower on `results`.
                        work.push(ZonkTask::BuildFun);
                        work.push(ZonkTask::Visit(b));
                        work.push(ZonkTask::Visit(a));
                    }
                    Content::Structure(FlatType::Con { module, name, args }) => {
                        let arity = args.len();
                        work.push(ZonkTask::BuildCon {
                            module,
                            name,
                            arity,
                        });
                        // Reverse so args land on `results` in source order.
                        for a in args.into_iter().rev() {
                            work.push(ZonkTask::Visit(a));
                        }
                    }
                    Content::Structure(FlatType::Tuple(elems)) => {
                        let arity = elems.len();
                        work.push(ZonkTask::BuildTuple { arity });
                        // Reverse so elements land on `results` in source order.
                        for e in elems.into_iter().rev() {
                            work.push(ZonkTask::Visit(e));
                        }
                    }
                    Content::Structure(FlatType::Record(fields)) => {
                        // Capture the field names (BTreeMap order) for the
                        // rebuild, and visit each field var in reverse so the
                        // results land in the same order the names are popped.
                        let names: Vec<Symbol> = fields.keys().copied().collect();
                        work.push(ZonkTask::BuildRecord { names });
                        for v in fields.values().copied().rev() {
                            work.push(ZonkTask::Visit(v));
                        }
                    }
                }
            }
            ZonkTask::BuildFun => {
                let (Some(b), Some(a)) = (results.pop(), results.pop()) else {
                    return Err(zonk_underflow());
                };
                results.push(Ty::Fun(Box::new(a), Box::new(b)));
            }
            ZonkTask::BuildCon {
                module,
                name,
                arity,
            } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(zonk_underflow)?;
                let args = results.split_off(split);
                results.push(Ty::Con { module, name, args });
            }
            ZonkTask::BuildTuple { arity } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(zonk_underflow)?;
                let elems = results.split_off(split);
                results.push(Ty::Tuple(elems));
            }
            ZonkTask::BuildRecord { names } => {
                let split = results
                    .len()
                    .checked_sub(names.len())
                    .ok_or_else(zonk_underflow)?;
                let tys = results.split_off(split);
                // `tys` is in the same order as `names` (field var visits were
                // reversed, so the results stack restores `BTreeMap` order).
                let fields: BTreeMap<Symbol, Ty> = names.into_iter().zip(tys).collect();
                results.push(Ty::Record(fields));
            }
        }
    }

    match results.pop() {
        Some(ty) if results.is_empty() => Ok(ty),
        _ => Err(zonk_underflow()),
    }
}

/// The work-stack invariant was violated (only reachable via a compiler bug in
/// `zonk` itself, never from input).
fn zonk_underflow() -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: STAGE,
        detail: "zonk result stack underflow".to_owned(),
    }
}
