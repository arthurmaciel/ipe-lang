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
use sky_diagnostics::{DResult, Diagnostic, Span};
use sky_intern::{Interner, Symbol};

use crate::solve::Constraint;
use crate::ty::{Content, FlatType, Ty, from_canon};
use crate::unionfind::{UnionFind, VarId};

/// `where_` tag for any `CompilerBug` raised during constraint generation.
const STAGE: &str = "sky_types::constrain";

/// Maximum structural depth [`Builder::zonk`] walks before declaring a type
/// pathologically deep. The occurs check in unification rules out true cycles,
/// so this bound is only ever hit on adversarial input.
const ZONK_DEPTH_LIMIT: u32 = 100_000;

/// Interned symbols for the built-in type constructors the inferencer needs to
/// name. `Int` / `String` usually already exist (from the source), but `Task`
/// never appears in M0 source, so the builder interns them up front to
/// guarantee a stable, resolvable [`Symbol`] for each.
struct Builtins {
    int: Symbol,
    string: Symbol,
    task: Symbol,
}

impl Builtins {
    fn new(interner: &mut Interner) -> DResult<Self> {
        Ok(Self {
            int: interner.intern("Int")?,
            string: interner.intern("String")?,
            task: interner.intern("Task")?,
        })
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
}

/// The output of constraint generation, consumed by the solver + read-back.
pub struct Generated {
    pub regions: BTreeMap<Span, VarId>,
    pub constraints: Vec<Constraint>,
    pub top_level: BTreeMap<Symbol, Ty>,
    pub untyped: BTreeMap<Symbol, VarId>,
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
        };

        // First pass: record annotation types so any binding can reference any
        // other (forward references resolve).
        for def in &module.defs {
            if let canon::Def::Typed { name, ty, .. } = def {
                builder.top_level.insert(name.value, from_canon(ty));
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
        })
    }

    // ── solver-var construction helpers ────────────────────────────────────

    fn flex(&mut self) -> DResult<VarId> {
        self.uf.fresh(Content::Flex)
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

    fn con_var(&mut self, module: Vec<Symbol>, name: Symbol, args: Vec<VarId>) -> DResult<VarId> {
        self.structure(FlatType::Con { module, name, args })
    }

    fn eq(&mut self, span: Span, lhs: VarId, rhs: VarId) {
        self.constraints.push(Constraint { span, lhs, rhs });
    }

    // ── Ty ⇄ solver bridges ────────────────────────────────────────────────

    /// Instantiate a resolved [`Ty`] into fresh union-find structure. Type
    /// variables alpha-rename consistently *within this call* via `vars`.
    fn instantiate(&mut self, ty: &Ty) -> DResult<VarId> {
        let mut vars = BTreeMap::new();
        self.instantiate_in(ty, &mut vars)
    }

    fn instantiate_in(&mut self, ty: &Ty, vars: &mut BTreeMap<u32, VarId>) -> DResult<VarId> {
        match ty {
            Ty::Unit => self.structure(FlatType::Unit),
            Ty::Var(id) => {
                if let Some(v) = vars.get(id).copied() {
                    return Ok(v);
                }
                let v = self.flex()?;
                vars.insert(*id, v);
                Ok(v)
            }
            Ty::Fun(a, b) => {
                let av = self.instantiate_in(a, vars)?;
                let bv = self.instantiate_in(b, vars)?;
                self.structure(FlatType::Fun(av, bv))
            }
            Ty::Con { module, name, args } => {
                let mut arg_vars = Vec::with_capacity(args.len());
                for a in args {
                    arg_vars.push(self.instantiate_in(a, vars)?);
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
                patterns, body, ty, ..
            } => {
                let mut local = BTreeMap::new();
                let mut cursor = ty;
                for pat in patterns {
                    let (arg_ty, rest) = peel_arrow(cursor)?;
                    let arg = from_canon(arg_ty);
                    let arg_var = self.instantiate(&arg)?;
                    Self::bind_param(&mut local, pat, arg_var);
                    cursor = rest;
                }
                let ret_ty = from_canon(cursor);
                let ret_var = self.instantiate(&ret_ty)?;
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
                for pat in patterns {
                    let v = self.flex()?;
                    Self::bind_param(&mut local, pat, v);
                }
                let body_var = self.constrain_expr(&local, body)?;
                self.untyped.insert(name.value, body_var);
                Ok(())
            }
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
                let ty = self.top_level.get(name).cloned();
                match ty {
                    Some(ty) => self.instantiate(&ty)?,
                    // Untyped top-level reference: leave fully flexible.
                    None => self.flex()?,
                }
            }
            canon::Expr_::VarKernel { module, name } => {
                let ty = self.kernel_ty(*module, *name);
                self.instantiate(&ty)?
            }
            canon::Expr_::VarCtor {
                home, type_name, ..
            } => {
                // M0 constructors are nullary, so the value's type is the enum.
                self.con_var(home.clone(), *type_name, Vec::new())?
            }
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
            canon::Expr_::Case(scrut, branches) => {
                let scrut_var = self.constrain_expr(local, scrut)?;
                let result = self.flex()?;
                for br in branches {
                    let mut br_local = local.clone();
                    self.constrain_pattern(&mut br_local, &br.pat, scrut_var)?;
                    let body_var = self.constrain_expr(&br_local, &br.body)?;
                    self.eq(br.body.span, body_var, result);
                }
                result
            }
            canon::Expr_::Binop { lhs, rhs, .. } => {
                // M0 exposes only `+` / `-`, both `Int -> Int -> Int`.
                let lv = self.constrain_expr(local, lhs)?;
                let rv = self.constrain_expr(local, rhs)?;
                let li = self.int_var()?;
                self.eq(lhs.span, lv, li);
                let ri = self.int_var()?;
                self.eq(rhs.span, rv, ri);
                self.int_var()?
            }
        };
        self.regions.insert(span, var);
        Ok(var)
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
                home, type_name, ..
            } => {
                // M0 constructor patterns are nullary; the pattern's type is the
                // enum, which must match the scrutinee.
                let ctor = self.con_var(home.clone(), *type_name, Vec::new())?;
                self.eq(pat.span, ctor, scrut_var);
                Ok(())
            }
        }
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

/// Read a settled union-find variable back into a resolved [`Ty`].
///
/// Called after [`crate::solve::solve`] has discharged every constraint. The
/// occurs check in unification guarantees the structure is acyclic, so the
/// depth bound is only ever hit on adversarial input.
///
/// # Errors
/// [`Diagnostic::CompilerBug`] on a union-find invariant violation or if the
/// structure is deeper than [`ZONK_DEPTH_LIMIT`].
pub fn zonk(uf: &mut UnionFind<Content>, var: VarId) -> DResult<Ty> {
    zonk_depth(uf, var, ZONK_DEPTH_LIMIT)
}

fn zonk_depth(uf: &mut UnionFind<Content>, var: VarId, depth: u32) -> DResult<Ty> {
    if depth == 0 {
        return Err(Diagnostic::CompilerBug {
            where_: STAGE,
            detail: "type exceeded read-back depth limit".to_owned(),
        });
    }
    let root = uf.find(var)?;
    match uf.content(root)? {
        Content::Flex => Ok(Ty::Var(root)),
        Content::Structure(FlatType::Unit) => Ok(Ty::Unit),
        Content::Structure(FlatType::Fun(a, b)) => {
            let at = zonk_depth(uf, a, depth - 1)?;
            let bt = zonk_depth(uf, b, depth - 1)?;
            Ok(Ty::Fun(Box::new(at), Box::new(bt)))
        }
        Content::Structure(FlatType::Con { module, name, args }) => {
            let mut targs = Vec::with_capacity(args.len());
            for a in args {
                targs.push(zonk_depth(uf, a, depth - 1)?);
            }
            Ok(Ty::Con {
                module,
                name,
                args: targs,
            })
        }
    }
}

/// Split an arrow type into (argument, remainder).
fn peel_arrow(t: &canon::Type) -> DResult<(&canon::Type, &canon::Type)> {
    match t {
        canon::Type::Lambda(a, b) => Ok((a, b)),
        _ => Err(Diagnostic::CompilerBug {
            where_: STAGE,
            detail: "binding has more parameters than its annotation's arrows".to_owned(),
        }),
    }
}
