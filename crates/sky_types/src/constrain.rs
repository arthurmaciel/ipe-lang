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
use crate::ty::{Content, FlatType, Ty, from_canon};
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
    task: Symbol,
}

impl Builtins {
    fn new(interner: &mut Interner) -> DResult<Self> {
        Ok(Self {
            int: interner.intern("Int")?,
            float: interner.intern("Float")?,
            bool: interner.intern("Bool")?,
            string: interner.intern("String")?,
            task: interner.intern("Task")?,
        })
    }
}

/// The type discipline a binary operator imposes. Classified once from the
/// resolved kernel name so the constraint walk doesn't re-borrow the interner.
#[derive(Clone, Copy)]
enum BinopClass {
    /// `+ - * //`: `Int -> Int -> Int`.
    IntArith,
    /// `/`: `Float -> Float -> Float` (matches the Go backend's float division).
    FloatDiv,
    /// `== /= < > <= >=`: `a -> a -> Bool` (operands share one type).
    Compare,
    /// `&& ||`: `Bool -> Bool -> Bool`.
    Boolean,
    /// Any other operator (`++`, `::`, …): `a -> a -> a`. Unreachable from the
    /// M1 lexer's operator set, but kept sound rather than panicking.
    Poly,
}

/// Classify a resolved operator kernel name (`add`, `eq`, `and`, …).
const fn classify_binop(func: &str) -> BinopClass {
    match func.as_bytes() {
        b"add" | b"sub" | b"mul" | b"idiv" => BinopClass::IntArith,
        b"fdiv" => BinopClass::FloatDiv,
        b"eq" | b"neq" | b"lt" | b"gt" | b"le" | b"ge" => BinopClass::Compare,
        b"and" | b"or" => BinopClass::Boolean,
        _ => BinopClass::Poly,
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
        };

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
            BinopClass::IntArith => {
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
            BinopClass::Compare => {
                // Operands unify to one shared type; the result is Bool.
                self.eq(rhs.span, lv, rv);
                self.bool_var()
            }
            BinopClass::Boolean => {
                let lb = self.bool_var()?;
                self.eq(lhs.span, lv, lb);
                let rb = self.bool_var()?;
                self.eq(rhs.span, rv, rb);
                self.bool_var()
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
        let mut vars = BTreeMap::new();
        self.instantiate_in(ty, &mut vars, /* rigid */ false)
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
                ..
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
                    Self::bind_param(&mut local, pat, arg_var);
                    cursor = rest;
                }
                let ret_ty = from_canon(cursor);
                let ret_var = self.instantiate_rigid(&ret_ty, &mut rigid_vars)?;
                let body_var = self.constrain_expr(&local, body)?;
                self.eq(body.span, body_var, ret_var);
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
                    Self::bind_param(&mut local, pat, v);
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

    /// Bind a function parameter pattern's names to `var` in `local`.
    fn bind_param(local: &mut BTreeMap<Symbol, VarId>, pat: &canon::Pattern, var: VarId) {
        match &pat.value {
            canon::Pattern_::PVar(s) => {
                local.insert(*s, var);
            }
            // `_` binds nothing; nullary ctor params don't occur in M0.
            canon::Pattern_::PAnything | canon::Pattern_::PCtor { .. } => {}
        }
    }

    fn constrain_expr(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        e: &canon::Expr,
    ) -> DResult<VarId> {
        let span = e.span;
        let var = match &e.value {
            canon::Expr_::Int(_) => self.int_var()?,
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
            canon::Expr_::VarTopLevel { name, .. } => {
                if let Some(ty) = self.top_level.get(name).cloned() {
                    // Typed binding: instantiate its scheme fresh (flex) here, so
                    // this use unifies against its own concrete arguments without
                    // pinning the binding's other call sites.
                    self.instantiate(&ty)?
                } else if let Some(v) = self.untyped.get(name).copied() {
                    // Untyped binding: resolve to its shared monomorphic variable.
                    v
                } else {
                    // Not a binding of this module (e.g. a re-export the
                    // canonicaliser accepted): leave fully flexible.
                    self.flex()?
                }
            }
            canon::Expr_::VarKernel { module, name } => {
                let ty = self.kernel_ty(*module, *name);
                self.instantiate(&ty)?
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
                    let_local.insert(b.name.value, bv);
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
            Self::bind_param(&mut lam_local, p, v);
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

    /// The type of a kernel function. M0 only exercises `String.fromInt` and
    /// `Log.println`; any other kernel is treated as fully polymorphic so it
    /// never spuriously fails inference for the M0 subset.
    fn kernel_ty(&self, module: Symbol, name: Symbol) -> Ty {
        let int = Ty::Con {
            module: Vec::new(),
            name: self.builtins.int,
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
        match (self.interner.resolve(module), self.interner.resolve(name)) {
            (Some("String"), Some("fromInt")) => Ty::Fun(Box::new(int), Box::new(string)),
            (Some("Log"), Some("println")) => Ty::Fun(Box::new(string), Box::new(task_unit)),
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
                    // A flexible or rigid variable that survives solving reads
                    // back as a type variable named by its representative's id.
                    Content::Flex | Content::Rigid => results.push(Ty::Var(root)),
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
