//! The lowering core: a name-resolved [`canon::Module`] plus its
//! [`SolvedTypes`] become a backend-agnostic [`sky_ir::Program`].
//!
//! This is the narrowed M0 port of the Haskell compiler's `Sky.Build.Compile`
//! lowering walk and `Sky.Build.LowerCtx`. Every step is total, and failures
//! split into two channels — never a panic, never a guess:
//!
//! * an input shape that is *valid Sky the M0 subset does not model yet*
//!   (polymorphism, higher-order values, extra kernels, …) becomes a
//!   [`sky_diagnostics::Diagnostic::Lower`] carrying the offending node's span
//!   and the matching `SKY-L01##` feature — the "not supported yet" channel;
//! * a *genuinely-unreachable* state (a foreign symbol, a missing `FuncId`, a
//!   type slot the solver did not record, an unresolved scrutinee enum) becomes
//!   a [`sky_diagnostics::Diagnostic::CompilerBug`] — the "compiler is broken"
//!   channel, reachable only for ill-canonicalised or ill-typed input.

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, Feature, LowerError, Span};
use sky_intern::{Interner, Symbol};
use sky_ir::{
    Arm, BinOp, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath, Module, Pat,
    Program, TypeDef,
};
use sky_types::{SolvedTypes, Ty};

/// Build a [`Diagnostic::CompilerBug`] for a violated lowering invariant.
///
/// Reserved **strictly** for genuinely-unreachable states: a symbol foreign to
/// the interner, a missing `FuncId`, a missing inferred region type, an
/// unresolved scrutinee enum — things a well-canonicalised, well-typed module
/// can never produce. A shape the M0 subset simply does not model yet is *not*
/// a bug: it goes through [`Self::unsupported`] instead.
fn bug(where_: &'static str, detail: impl Into<String>) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_,
        detail: detail.into(),
    }
}

/// Build a [`Diagnostic::Lower`] for a feature the M0 lowerer does not model
/// yet, carrying the offending node's source `span`. This is the
/// "not supported yet" channel (`SKY-L01##`), distinct from [`bug`] ("the
/// compiler is broken"): the input is valid Sky the M0 subset has not reached.
const fn unsupported(span: Span, feature: Feature) -> Diagnostic {
    Diagnostic::Lower {
        span,
        msg: LowerError::Unsupported(feature),
    }
}

/// The lowering pass over a single canonical module.
pub struct Lowerer<'a> {
    m: &'a canon::Module,
    types: &'a SolvedTypes,
    interner: &'a Interner,
    /// Each top-level binding's [`FuncId`], assigned in declaration order so a
    /// `VarTopLevel` reference can resolve to a [`Callee::Func`].
    func_ids: BTreeMap<Symbol, FuncId>,
    /// Each union's complete, in-declaration-order constructor set — the *true*
    /// variant set handed to [`Match::new`] (the IR layer cannot self-confirm
    /// this; the lowerer carries the obligation).
    enum_variants: BTreeMap<Symbol, Vec<Symbol>>,
    /// Pre-minted, collision-free parameter names for eta-expanding a partial
    /// application into a boxed closure. Sized in [`crate::lower`] to the widest
    /// function arity in the module — an eta-lambda introduces at most that many
    /// params — so position `i` of the pool names the i-th synthesised parameter.
    /// Each eta-lambda is its own closure scope, so the same pool entry is reused
    /// across sites without shadowing; [`Interner::fresh_symbols`] guarantees no
    /// entry aliases a user identifier.
    eta_params: Vec<Symbol>,
}

/// The widest parameter-pattern count across the module's top-level bindings —
/// the most parameters any single eta-expanded partial application can need.
/// Drives the eta-parameter pool sizing in [`crate::lower`].
pub fn max_def_arity(m: &canon::Module) -> usize {
    m.defs
        .iter()
        .map(|d| match d {
            canon::Def::Typed { patterns, .. } | canon::Def::Untyped { patterns, .. } => {
                patterns.len()
            }
        })
        .max()
        .unwrap_or(0)
}

impl<'a> Lowerer<'a> {
    pub fn new(
        m: &'a canon::Module,
        types: &'a SolvedTypes,
        interner: &'a Interner,
        eta_params: Vec<Symbol>,
    ) -> Self {
        let mut func_ids = BTreeMap::new();
        for (idx, def) in m.defs.iter().enumerate() {
            let id = FuncId::from_raw(u32::try_from(idx).unwrap_or(u32::MAX));
            func_ids.insert(def.name().value, id);
        }

        let mut enum_variants = BTreeMap::new();
        for union in &m.unions {
            enum_variants.insert(union.name, union.ctors.iter().map(|c| c.name).collect());
        }

        Self {
            m,
            types,
            interner,
            func_ids,
            enum_variants,
            eta_params,
        }
    }

    /// Resolve a symbol the IR guarantees was interned by `interner`. A `None`
    /// means the canonical AST carried a foreign symbol — an internal invariant
    /// violation, surfaced as a [`Diagnostic::CompilerBug`] rather than a silent
    /// empty name.
    fn resolve(&self, sym: Symbol) -> DResult<&'a str> {
        self.interner.resolve(sym).ok_or_else(|| {
            bug(
                "sky_lower::resolve",
                format!("symbol {} not present in interner", sym.as_raw()),
            )
        })
    }

    /// Run the pass, producing the single-module program.
    pub fn run(self) -> DResult<Program> {
        let types_ir: Vec<TypeDef> = self
            .m
            .unions
            .iter()
            .map(|u| {
                TypeDef::Enum(EnumDef {
                    name: u.name,
                    variants: u.ctors.iter().map(|c| c.name).collect(),
                })
            })
            .collect();

        let mut funcs = Vec::with_capacity(self.m.defs.len());
        let mut entry = None;
        for def in &self.m.defs {
            let func = self.lower_def(def)?;
            if self.interner.resolve(func.name) == Some("main") {
                entry = Some(func.id);
            }
            funcs.push(func);
        }

        let records = self.collect_record_types()?;

        let module = Module {
            name: ModPath(self.m.name.clone()),
            types: types_ir,
            funcs,
            entry,
            records,
        };
        Ok(Program {
            modules: vec![module],
        })
    }

    /// Collect every distinct CLOSED record shape the module's expressions
    /// construct or read, as [`IrType::Record`]s for the backend to synthesise a
    /// struct from. A record literal lives inside a function body, where its
    /// type appears in no signature — so the type-directed lowerer surfaces it
    /// here from the solver's per-region (and per-binding) types, which is the
    /// only place the solved record shape is known.
    ///
    /// Determinism: both maps walked are `BTreeMap`s, and duplicates are dropped
    /// by full structural equality, so the output order is fixed.
    fn collect_record_types(&self) -> DResult<Vec<IrType>> {
        let mut out: Vec<IrType> = Vec::new();
        for ty in self.types.regions.values().chain(self.types.env.values()) {
            self.collect_records_in_ty(ty, &mut out)?;
        }
        Ok(out)
    }

    /// Walk a solved [`Ty`], pushing every distinct record shape it contains
    /// (nested records first) into `out`. Non-record shapes recurse into their
    /// children; leaves contribute nothing.
    fn collect_records_in_ty(&self, ty: &Ty, out: &mut Vec<IrType>) -> DResult<()> {
        match ty {
            Ty::Record(fields) => {
                for field_ty in fields.values() {
                    self.collect_records_in_ty(field_ty, out)?;
                }
                let ir = self.ir_type_from_ty(ty, Span::DUMMY)?;
                if !out.contains(&ir) {
                    out.push(ir);
                }
            }
            Ty::Tuple(elems) => {
                for e in elems {
                    self.collect_records_in_ty(e, out)?;
                }
            }
            Ty::Fun(a, b) => {
                self.collect_records_in_ty(a, out)?;
                self.collect_records_in_ty(b, out)?;
            }
            Ty::Con { args, .. } => {
                for a in args {
                    self.collect_records_in_ty(a, out)?;
                }
            }
            Ty::Var(_) | Ty::Unit => {}
        }
        Ok(())
    }

    fn lower_def(&self, def: &canon::Def) -> DResult<Func> {
        let name = def.name().value;
        let id = *self
            .func_ids
            .get(&name)
            .ok_or_else(|| bug("sky_lower::lower_def", "missing func id"))?;

        let sig_span = def.name().span;
        match def {
            canon::Def::Typed {
                patterns, body, ty, ..
            } => {
                let (params, ret) = self.split_typed_sig(ty, patterns, sig_span)?;
                Ok(Func {
                    id,
                    name,
                    params,
                    ret,
                    body: self.lower_expr(body)?,
                })
            }
            canon::Def::Untyped { patterns, body, .. } => {
                if !patterns.is_empty() {
                    // An unannotated top-level binding with parameters: the M0
                    // lowerer needs the annotation's arrows to type its params.
                    // [SKY-L0106, feature: untyped-functions]
                    return Err(unsupported(sig_span, Feature::UntypedFunctions));
                }
                let ret_ty =
                    self.types.env.get(&name).ok_or_else(|| {
                        bug("sky_lower::lower_def", "no inferred type for binding")
                    })?;
                let ret = self.ir_type_from_ty(ret_ty, sig_span)?;
                Ok(Func {
                    id,
                    name,
                    params: Vec::new(),
                    ret,
                    body: self.lower_expr(body)?,
                })
            }
        }
    }

    /// Split a typed binding's arrow annotation into one [`IrType`] per
    /// parameter pattern plus the trailing return type.
    fn split_typed_sig(
        &self,
        ty: &canon::Type,
        patterns: &[canon::Pattern],
        sig_span: Span,
    ) -> DResult<(Vec<(Symbol, IrType)>, IrType)> {
        let mut cur = ty;
        let mut params = Vec::with_capacity(patterns.len());
        for pat in patterns {
            let canon::Type::Lambda(arg, rest) = cur else {
                // More parameter patterns than the annotation has arrows. The
                // type checker rejects this first (the body's inferred arity
                // cannot unify with the shorter annotation → SKY-T0001), so
                // reaching it here is a genuine invariant violation, not a
                // missing M0 feature. (Slated to become a dedicated SKY-T0004
                // at the type-checking boundary.)
                return Err(bug(
                    "sky_lower::split_typed_sig",
                    "annotation has fewer arrows than parameters",
                ));
            };
            // The argument type describes this parameter; blame its pattern.
            params.push((
                Self::pattern_var(pat)?,
                self.ir_type_from_canon(arg, pat.span)?,
            ));
            cur = rest.as_ref();
        }
        // The trailing type is the return type; blame the binding signature.
        Ok((params, self.ir_type_from_canon(cur, sig_span)?))
    }

    const fn pattern_var(pat: &canon::Pattern) -> DResult<Symbol> {
        match &pat.value {
            canon::Pattern_::PVar(s) => Ok(*s),
            // A non-variable parameter pattern (`_`, `Just x`, a literal). M0
            // function parameters must be plain names.
            // [SKY-L0105, feature: param-patterns]
            _ => Err(unsupported(pat.span, Feature::ParamPatterns)),
        }
    }

    /// Convert a canonical annotation type (no `Task`/unit appears in M0
    /// annotations) into an [`IrType`].
    fn ir_type_from_canon(&self, t: &canon::Type, span: Span) -> DResult<IrType> {
        match t {
            canon::Type::Con { name, .. } => self.con_name_to_ir(*name),
            // A function type in argument/return position of a value annotation
            // (`apply : (Int -> Int) -> Int`). Flatten the curried arrow chain
            // into one boxed `Fn` value type `Fun([T0, …], R)`.
            canon::Type::Lambda(_, _) => {
                let mut params = Vec::new();
                let mut cur = t;
                while let canon::Type::Lambda(arg, rest) = cur {
                    params.push(self.ir_type_from_canon(arg, span)?);
                    cur = rest.as_ref();
                }
                let ret = self.ir_type_from_canon(cur, span)?;
                Ok(IrType::Fun(params, Box::new(ret)))
            }
            // A type variable in an annotation (`id : a -> a`); M0 is monomorphic.
            // [SKY-L0102, feature: polymorphism]
            canon::Type::Var(_) => Err(unsupported(span, Feature::Polymorphism)),
        }
    }

    /// Lower an anonymous function `\p0 p1 ... -> body` into [`Expr::Lambda`].
    ///
    /// The lambda's solved region type is a curried arrow `T0 -> T1 -> … -> R`.
    /// A directly-nested lambda body (`\b -> \c -> e`) is *flattened* into this
    /// same multi-parameter [`Expr::Lambda`]: one arrow is peeled from the region
    /// type per parameter, across every nested level, until the body is no longer
    /// a lambda. This mirrors how [`Self::ir_type_from_ty`] /
    /// [`Self::ir_type_from_canon`] fully flatten a curried arrow chain into a
    /// single `Fun([T0, …], R)`, so the emitted closure's arity always equals its
    /// declared `Box<dyn Fn(..)>` type — at *every* nesting depth, not just one.
    /// (Without the flatten, `f a = \b -> \c -> …` declared `Int -> Int -> Int ->
    /// Int` emits a curried `Fn(i64) -> Fn(i64) -> i64` body into a flattened
    /// `Fn(i64, i64) -> i64` return slot, which cargo rejects with no Sky
    /// diagnostic.) Parameter patterns must be plain names (M1 has no parameter
    /// destructuring).
    fn lower_lambda(
        &self,
        params: &[canon::Pattern],
        body: &canon::Expr,
        span: Span,
    ) -> DResult<Expr> {
        // The region type the solver recorded for this lambda is its arrow.
        let ty = self.types.regions.get(&span).ok_or_else(|| {
            bug(
                "sky_lower::lower_lambda",
                "no inferred type for lambda expression",
            )
        })?;
        let mut cur = ty;
        let mut ir_params = Vec::with_capacity(params.len());
        // The frontier of the flatten: start at this lambda's own params/body,
        // then descend into each directly-nested lambda while the arrow type can
        // still supply a parameter type.
        let mut cur_params: &[canon::Pattern] = params;
        let mut cur_body: &canon::Expr = body;
        loop {
            for pat in cur_params {
                let Ty::Fun(arg, rest) = cur else {
                    // The lambda's inferred type has fewer arrows than it has
                    // parameters — ruled out by inference (the lambda arm builds
                    // one arrow per parameter), so reaching here is an invariant
                    // violation, not a missing feature.
                    return Err(bug(
                        "sky_lower::lower_lambda",
                        "lambda type has fewer arrows than parameters",
                    ));
                };
                ir_params.push((
                    Self::pattern_var(pat)?,
                    self.ir_type_from_ty(arg, pat.span)?,
                ));
                cur = rest.as_ref();
            }
            // Collapse a directly-nested lambda body into this same closure: a
            // remaining `Fun` arrow proves the type still curries, so the nested
            // params extend `ir_params` rather than becoming a separate boxed
            // closure. The `matches!` guard is belt-and-braces — a well-typed
            // lambda body always carries a function type, so when `cur_body` is a
            // lambda `cur` is always `Fun` — but keeping it means any unexpected
            // shape degrades to the single-level lowering rather than panicking.
            match &cur_body.value {
                canon::Expr_::Lambda(inner_params, inner_body) if matches!(cur, Ty::Fun(_, _)) => {
                    cur_params = inner_params;
                    cur_body = inner_body;
                }
                _ => break,
            }
        }
        let ret = self.ir_type_from_ty(cur, span)?;
        let body = self.lower_expr(cur_body)?;
        Ok(Expr::Lambda {
            params: ir_params,
            ret,
            body: Box::new(body),
        })
    }

    /// Convert a solved [`Ty`] (used for the return type of untyped bindings,
    /// e.g. `main : Task ()`) into an [`IrType`]. `span` blames the binding when
    /// the inferred type is a shape M0 does not model yet.
    fn ir_type_from_ty(&self, t: &Ty, span: Span) -> DResult<IrType> {
        match t {
            Ty::Unit => Ok(IrType::Unit),
            // Builtin names are matched first: `sky_canon`'s §3.2 gate rejects
            // any user type/ctor that shadows a builtin name, so this precedence
            // is sound (it can never silently override a user `type Int = …`),
            // not a deliberate override.
            Ty::Con { name, args, .. } => match self.resolve(*name)? {
                "Int" => Ok(IrType::Int),
                "Float" => Ok(IrType::Float),
                "Bool" => Ok(IrType::Bool),
                "String" => Ok(IrType::Str),
                "Task" if args.len() == 1 && matches!(args.first(), Some(Ty::Unit)) => {
                    Ok(IrType::TaskUnit)
                }
                // A `Task` carrying a non-unit result (`Task Int`); M0 models
                // only `Task ()`. [SKY-L0104, feature: task-results]
                "Task" => Err(unsupported(span, Feature::TaskResults)),
                _ if self.enum_variants.contains_key(name) => Ok(IrType::Enum(*name)),
                // Name resolution guarantees every type constructor resolves to
                // a builtin or a declared union, so an unknown one here is an
                // invariant violation, not user error.
                other => Err(bug(
                    "sky_lower::ir_type_from_ty",
                    format!("unknown type constructor `{other}`"),
                )),
            },
            // A tuple in value position (e.g. a binding whose body is a tuple
            // literal): lower element-wise to the IR tuple type.
            Ty::Tuple(elems) => {
                let lowered = elems
                    .iter()
                    .map(|e| self.ir_type_from_ty(e, span))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(IrType::Tuple(lowered))
            }
            // A closed record type: lower each field type, keyed by field name.
            Ty::Record(fields) => {
                let mut lowered = BTreeMap::new();
                for (name, field_ty) in fields {
                    lowered.insert(*name, self.ir_type_from_ty(field_ty, span)?);
                }
                Ok(IrType::Record(lowered))
            }
            // An inferred function type in value position (a lambda, or a
            // function-typed parameter/binding). Flatten the curried arrow chain
            // into one boxed `Fn` value type `Fun([T0, …], R)`, matching the
            // backend's `Box<dyn Fn(T0, …) -> R>` rendering.
            Ty::Fun(_, _) => {
                let mut params = Vec::new();
                let mut cur = t;
                while let Ty::Fun(arg, rest) = cur {
                    params.push(self.ir_type_from_ty(arg, span)?);
                    cur = rest.as_ref();
                }
                let ret = self.ir_type_from_ty(cur, span)?;
                Ok(IrType::Fun(params, Box::new(ret)))
            }
            // The solver left a flexible variable unresolved for a concrete
            // binding: it should have been fully zonked. An invariant violation.
            Ty::Var(_) => Err(bug(
                "sky_lower::ir_type_from_ty",
                "unresolved type variable in value position",
            )),
        }
    }

    /// Map a built-in or user type constructor *name* to an [`IrType`].
    fn con_name_to_ir(&self, name: Symbol) -> DResult<IrType> {
        // Builtin names are matched first: `sky_canon`'s §3.2 gate rejects any
        // user type/ctor that shadows a builtin name, so matching `Int`/`Float`/
        // `Bool`/`String` ahead of the user-enum lookup can never silently
        // override a user declaration — the precedence is pinned to that gate,
        // not a deliberate shadow.
        match self.resolve(name)? {
            "Int" => Ok(IrType::Int),
            "Float" => Ok(IrType::Float),
            "Bool" => Ok(IrType::Bool),
            "String" => Ok(IrType::Str),
            _ if self.enum_variants.contains_key(&name) => Ok(IrType::Enum(name)),
            // Name resolution + the type checker guarantee every type
            // constructor in an annotation resolves to a builtin or a declared
            // union before lowering, so an unknown one here is a violated
            // invariant, not user error.
            other => Err(bug(
                "sky_lower::con_name_to_ir",
                format!("unknown type constructor `{other}`"),
            )),
        }
    }

    /// Reject a record field whose value is function-typed.
    ///
    /// A function value lowers to a `Box<dyn Fn(..) -> R>`, but a synthesised
    /// record struct derives `Clone`/`Debug`/`PartialEq` — none of which a boxed
    /// `dyn Fn` satisfies — so a function-in-record field would emit Rust that
    /// does not compile. Storing a function in a `let` works (no derive is
    /// involved); storing one in a record is the documented first-class gap
    /// until the record struct can carry a non-deriving function field.
    /// [SKY-L0107, feature: first-class-functions]
    fn reject_function_valued_field(&self, value: &canon::Expr) -> DResult<()> {
        if let Some(Ty::Fun(_, _)) = self.types.regions.get(&value.span) {
            return Err(unsupported(value.span, Feature::FirstClassFunctions));
        }
        Ok(())
    }

    fn lower_expr(&self, e: &canon::Expr) -> DResult<Expr> {
        match &e.value {
            canon::Expr_::Int(n) => Ok(Expr::Int(*n)),
            canon::Expr_::VarLocal(s) => Ok(Expr::Var(*s)),
            canon::Expr_::VarCtor {
                type_name, name, ..
            } => Ok(Expr::Ctor {
                ty: *type_name,
                variant: *name,
            }),
            canon::Expr_::Binop { func, lhs, rhs, .. } => Ok(Expr::BinOp {
                op: self.binop(*func, e.span)?,
                lhs: Box::new(self.lower_expr(lhs)?),
                rhs: Box::new(self.lower_expr(rhs)?),
            }),
            canon::Expr_::Call(callee, args) => self.lower_call(callee, args, e.span),
            canon::Expr_::Lambda(params, body) => self.lower_lambda(params, body, e.span),
            canon::Expr_::Let(bindings, body) => {
                // Multi-binding `let` lowers to right-nested single-binding IR
                // `Let`s: `let a = …; b = … in body` becomes
                // `Let a (Let b body)`. The IR `Let` is non-recursive — `name`
                // is bound only within `body` — which matches the sequential
                // (`let*`) scoping canonicalisation and inference established.
                let mut acc = self.lower_expr(body)?;
                for b in bindings.iter().rev() {
                    let value = self.lower_expr(&b.body)?;
                    acc = Expr::Let {
                        name: b.name.value,
                        value: Box::new(value),
                        body: Box::new(acc),
                    };
                }
                Ok(acc)
            }
            canon::Expr_::If(branches, else_expr) => {
                // A multi-way `if` (with `else if` branches) lowers to right-
                // nested binary `If`s: `if c1 then a else if c2 then b else c`
                // becomes `If c1 a (If c2 b c)`. Folding from the right keeps
                // the source order of the conditions.
                let mut acc = self.lower_expr(else_expr)?;
                for (cond, body) in branches.iter().rev() {
                    let cond = self.lower_expr(cond)?;
                    let then_ = self.lower_expr(body)?;
                    acc = Expr::If {
                        cond: Box::new(cond),
                        then_: Box::new(then_),
                        else_: Box::new(acc),
                    };
                }
                Ok(acc)
            }
            canon::Expr_::Tuple(elems) => {
                // A tuple value lowers element-wise to the IR tuple constructor.
                // The parser guarantees arity ≥ 2, which is the IR invariant.
                let elems = elems
                    .iter()
                    .map(|e| self.lower_expr(e))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Expr::Tuple(elems))
            }
            canon::Expr_::Record(fields) => {
                // A record literal lowers field-wise. The IR carries fields in
                // field-NAME order (the backend names struct-literal fields, so
                // write order is free), making the lowering deterministic
                // regardless of source order or interning order.
                let mut lowered: Vec<(Symbol, Expr)> = Vec::with_capacity(fields.len());
                for (name, value) in fields {
                    self.reject_function_valued_field(value)?;
                    lowered.push((*name, self.lower_expr(value)?));
                }
                lowered.sort_by(|a, b| {
                    self.resolve(a.0)
                        .unwrap_or("")
                        .cmp(self.resolve(b.0).unwrap_or(""))
                });
                Ok(Expr::Record(lowered))
            }
            canon::Expr_::Access(record, field) => Ok(Expr::Access {
                record: Box::new(self.lower_expr(record)?),
                field: *field,
            }),
            canon::Expr_::Update(base, fields) => {
                // A record update lowers to a copy of `base` with the listed
                // fields replaced. Only the changed fields are carried, sorted by
                // field name so the lowering is deterministic; the backend names
                // each reassignment, so write order is free. The result's record
                // struct is the base's, already surfaced via `Module.records`
                // from the base region's solved type.
                let record = Box::new(self.lower_expr(base)?);
                let mut lowered: Vec<(Symbol, Expr)> = Vec::with_capacity(fields.len());
                for (name, value) in fields {
                    self.reject_function_valued_field(value)?;
                    lowered.push((*name, self.lower_expr(value)?));
                }
                lowered.sort_by(|a, b| {
                    self.resolve(a.0)
                        .unwrap_or("")
                        .cmp(self.resolve(b.0).unwrap_or(""))
                });
                Ok(Expr::Update {
                    record,
                    fields: lowered,
                })
            }
            canon::Expr_::Case(scrut, branches) => self.lower_case(scrut, branches),
            // A top-level binding or kernel named as a bare *value* (passed,
            // returned, or let-bound) rather than directly applied. The
            // reference's solved region type fixes its shape: a function type
            // reifies into an [`Expr::FuncValue`] (a boxed closure the backend
            // pins to a `Box<dyn Fn(..) -> R>` slot); a non-function top-level
            // value reference (a nullary constant binding named as a value) is
            // its zero-argument call.
            canon::Expr_::VarTopLevel { .. } | canon::Expr_::VarKernel { .. } => {
                let callee = self.lower_callee(e)?;
                let ty = self.types.regions.get(&e.span).ok_or_else(|| {
                    bug(
                        "sky_lower::lower_expr",
                        "no inferred type for a function/value reference",
                    )
                })?;
                match self.ir_type_from_ty(ty, e.span)? {
                    fun @ IrType::Fun(_, _) => Ok(Expr::FuncValue { callee, ty: fun }),
                    // A nullary top-level constant referenced as a value is its
                    // own zero-argument call (`x` → `x()`); a kernel is always
                    // function-typed, so this branch is the constant case.
                    _ => Ok(Expr::Call {
                        callee,
                        args: Vec::new(),
                    }),
                }
            }
        }
    }

    /// Lower a function application. A kernel or top-level callee keeps the
    /// efficient direct [`Callee`] path (`Expr::Call`); any other callee is a
    /// first-class function *value* — a local (function-typed) binding, a
    /// lambda, or another expression's result — applied via [`Expr::Apply`]
    /// (a boxed `dyn Fn` auto-derefs at the call site).
    ///
    /// A direct [`Expr::Call`] is *saturated*: it passes exactly as many
    /// arguments as the callee declares. A top-level `fn` / kernel has a fixed
    /// Rust signature, so a call whose argument count differs from the callee's
    /// arity cannot be one direct `Call` — it is reshaped to preserve currying:
    ///
    /// * **exact** (`args == arity`) — the direct [`Expr::Call`] (the fast path);
    /// * **partial** (`args < arity`) — eta-expanded into an [`Expr::Lambda`]
    ///   that captures the supplied args and takes the missing ones as fresh
    ///   parameters, its body the now-saturated [`Expr::Call`]
    ///   (see [`Self::eta_expand_partial`]);
    /// * **over** (`args > arity`) — saturated: the first `arity` args form a
    ///   direct [`Expr::Call`], and the surplus apply to its (function-typed)
    ///   result through an [`Expr::Apply`] (see [`Self::saturate_over`]) — but
    ///   only when the surplus exactly saturates the returned closure; a surplus
    ///   that leaves it partially applied fails closed (see [`Self::saturate_over`]).
    ///
    /// A non-named callee — a local (function-typed) binding, a lambda, or
    /// another expression's result — is a first-class function *value* applied
    /// via [`Expr::Apply`] (a boxed `dyn Fn` auto-derefs at the call site).
    fn lower_call(
        &self,
        callee: &canon::Expr,
        args: &[canon::Expr],
        call_span: Span,
    ) -> DResult<Expr> {
        let lowered_args = args
            .iter()
            .map(|a| self.lower_expr(a))
            .collect::<DResult<Vec<_>>>()?;
        match &callee.value {
            canon::Expr_::VarKernel { .. } | canon::Expr_::VarTopLevel { .. } => {
                let resolved = self.lower_callee(callee)?;
                let arity = self.callee_arity(&resolved)?;
                match args.len().cmp(&arity) {
                    std::cmp::Ordering::Equal => Ok(Expr::Call {
                        callee: resolved,
                        args: lowered_args,
                    }),
                    std::cmp::Ordering::Less => {
                        self.eta_expand_partial(callee, resolved, lowered_args, arity, call_span)
                    }
                    std::cmp::Ordering::Greater => {
                        self.saturate_over(callee, resolved, lowered_args, arity, call_span)
                    }
                }
            }
            _ => {
                // A first-class function *value* applied via [`Expr::Apply`]
                // (a local function-typed binding, a lambda, or another
                // expression's result). The named-callee path above reshapes an
                // arity mismatch (eta-expand / saturate); the value path cannot
                // — eta-expanding a value would have to capture the closure
                // value itself, a distinct mechanism M1 does not yet provide.
                //
                // So when the callee's solved type is a known curried arrow whose
                // arity exceeds the supplied argument count, this is *partial*
                // application of a first-class value: fail closed with a Sky
                // diagnostic rather than emit an under-applied `(g)(a)` that cargo
                // rejects with no Sky-level error. (Over-application of a value is
                // ruled out earlier by type-checking — applying past the arity
                // makes the result a non-function — so a mismatch here is always
                // partial.) A missing or non-arrow region type falls through to
                // the direct apply, preserving the exact-application fast path.
                if let Some(ty) = self.types.regions.get(&callee.span) {
                    let arity = Self::ty_arrow_arity(ty);
                    if arity != 0 && args.len() != arity {
                        return Err(unsupported(call_span, Feature::PartialOverApplication));
                    }
                }
                Ok(Expr::Apply {
                    func: Box::new(self.lower_expr(callee)?),
                    args: lowered_args,
                })
            }
        }
    }

    /// The number of leading arrows in a curried function type — the argument
    /// count a saturated application of a value of this type must pass. A
    /// non-function type has arity `0`. Used to detect partial application of a
    /// first-class function value, which M1 fails closed on rather than emitting
    /// an under-applied call. (The IR flattens this curried chain into one
    /// multi-parameter `Fun`, so this count is the boxed closure's parameter
    /// count.)
    fn ty_arrow_arity(t: &Ty) -> usize {
        let mut n = 0;
        let mut cur = t;
        while let Ty::Fun(_, rest) = cur {
            n += 1;
            cur = rest.as_ref();
        }
        n
    }

    /// Eta-expand a partial application `f a0 … a_{k-1}` (with `k < arity`) into a
    /// boxed closure `\eta_k … eta_{arity-1} -> f(a0, …, a_{k-1}, eta_k, …)` — a
    /// first-class function value of the residual arrow type. The supplied
    /// `lowered_args` are captured; the missing parameters take fresh,
    /// collision-free names from [`Self::eta_params`].
    ///
    /// The per-parameter and return types come from the callee's solved region
    /// type (the full arrow `T0 -> … -> T_{arity-1} -> R`) — never guessed. A
    /// missing region type, or an arrow shorter than `arity`, is unreachable for
    /// well-typed input and surfaces as a [`Diagnostic::CompilerBug`], not a
    /// silent default.
    fn eta_expand_partial(
        &self,
        callee: &canon::Expr,
        resolved: Callee,
        lowered_args: Vec<Expr>,
        arity: usize,
        call_span: Span,
    ) -> DResult<Expr> {
        let fn_ty = self.types.regions.get(&callee.span).ok_or_else(|| {
            bug(
                "sky_lower::eta_expand_partial",
                "no inferred type for a partially-applied callee",
            )
        })?;
        // Peel exactly `arity` arrows: the argument types in order, then the
        // trailing result type R.
        let mut cur = fn_ty;
        let mut arg_tys: Vec<&Ty> = Vec::with_capacity(arity);
        for _ in 0..arity {
            let Ty::Fun(arg, rest) = cur else {
                // The callee's type has fewer arrows than its declared arity —
                // ruled out for well-typed input (inference unified the callee
                // against an `arity`-deep arrow), so this is an invariant
                // violation, not a missing feature.
                return Err(bug(
                    "sky_lower::eta_expand_partial",
                    "callee type has fewer arrows than its arity",
                ));
            };
            arg_tys.push(arg);
            cur = rest.as_ref();
        }
        let ret_ty = cur;

        let supplied = lowered_args.len();
        // The missing parameters are argument positions `supplied..arity`.
        let mut params: Vec<(Symbol, IrType)> = Vec::with_capacity(arity - supplied);
        let mut call_args = lowered_args;
        for (offset, arg_ty) in arg_tys.get(supplied..).unwrap_or(&[]).iter().enumerate() {
            // Reuse pool slot `offset`: each eta-lambda is its own scope, so the
            // i-th synthesised param can share a name across sites without
            // shadowing. A miss means the pool was undersized — an invariant
            // violation, since it is sized to the module's widest arity.
            let sym = *self.eta_params.get(offset).ok_or_else(|| {
                bug(
                    "sky_lower::eta_expand_partial",
                    "eta-parameter pool smaller than the partial-application gap",
                )
            })?;
            let ir = self.ir_type_from_ty(arg_ty, call_span)?;
            params.push((sym, ir));
            call_args.push(Expr::Var(sym));
        }
        let ret = self.ir_type_from_ty(ret_ty, call_span)?;
        let body = Expr::Call {
            callee: resolved,
            args: call_args,
        };
        Ok(Expr::Lambda {
            params,
            ret,
            body: Box::new(body),
        })
    }

    /// Saturate an over-application `f a0 … a_{n-1}` (with `n > arity`): the first
    /// `arity` args form the direct [`Expr::Call`] to `f` (returning a
    /// function-typed value), and the surplus apply to that result via one
    /// [`Expr::Apply`]. A single `Apply` suffices because the IR flattens a
    /// curried result type into one multi-parameter [`IrType::Fun`], so the
    /// trailing closure accepts every remaining argument at once; the backend
    /// renders it as `(f(a0, …))(a_arity, …)`.
    ///
    /// That single-`Apply` shape is sound **only when the surplus exactly
    /// saturates the returned closure**. The closure's arity is the callee
    /// type's full arrow depth minus the `arity` parameters the direct `Call`
    /// already consumes; if the surplus is short of it, the result is itself a
    /// partial application of a first-class value — which M1 cannot lower (the
    /// returned closure is a flattened multi-parameter `Fn`; under-applying it
    /// would need first-class-value partial application). So in that case we fail
    /// closed with [`Feature::PartialOverApplication`] rather than emit
    /// `(f(a0))(a_arity)` that passes too few arguments and cargo rejects with no
    /// Sky-level diagnostic. (A surplus that EXCEEDS the returned closure's arity
    /// is ruled out earlier by type-checking — applying past the arity makes the
    /// result a non-function.) A missing/non-arrow callee region type falls
    /// through to the bare reshape, preserving behaviour for the exact-surplus
    /// case the solver always types.
    fn saturate_over(
        &self,
        callee: &canon::Expr,
        resolved: Callee,
        lowered_args: Vec<Expr>,
        arity: usize,
        call_span: Span,
    ) -> DResult<Expr> {
        let surplus = lowered_args.len().saturating_sub(arity);
        if let Some(ty) = self.types.regions.get(&callee.span) {
            let returned_arity = Self::ty_arrow_arity(ty).saturating_sub(arity);
            if surplus != returned_arity {
                return Err(unsupported(call_span, Feature::PartialOverApplication));
            }
        }
        let mut iter = lowered_args.into_iter();
        let head: Vec<Expr> = iter.by_ref().take(arity).collect();
        let rest: Vec<Expr> = iter.collect();
        Ok(Expr::Apply {
            func: Box::new(Expr::Call {
                callee: resolved,
                args: head,
            }),
            args: rest,
        })
    }

    /// The declared arity of a resolved direct callee — the argument count a
    /// saturated [`Expr::Call`] to it must pass. A kernel's arity is fixed per
    /// [`KernelFn`]; a top-level binding's is its parameter-pattern count (a
    /// nullary constant has arity 0). The [`FuncId`] was assigned from the
    /// definitions in declaration order, so the same-index lookup is exact.
    fn callee_arity(&self, callee: &Callee) -> DResult<usize> {
        match callee {
            // Both M0/M1 kernels take a single argument; widen this match as
            // the kernel set grows so a new entry can never silently inherit 1.
            Callee::Kernel(KernelFn::StringFromInt | KernelFn::LogPrintln) => Ok(1),
            Callee::Func(id) => {
                let idx = usize::try_from(id.as_raw()).unwrap_or(usize::MAX);
                let def = self.m.defs.get(idx).ok_or_else(|| {
                    bug(
                        "sky_lower::callee_arity",
                        "func id has no matching definition",
                    )
                })?;
                Ok(match def {
                    canon::Def::Typed { patterns, .. } | canon::Def::Untyped { patterns, .. } => {
                        patterns.len()
                    }
                })
            }
        }
    }

    fn lower_callee(&self, callee: &canon::Expr) -> DResult<Callee> {
        match &callee.value {
            canon::Expr_::VarKernel { module, name } => {
                match (self.resolve(*module)?, self.resolve(*name)?) {
                    ("Log", "println") => Ok(Callee::Kernel(KernelFn::LogPrintln)),
                    ("String", "fromInt") => Ok(Callee::Kernel(KernelFn::StringFromInt)),
                    // A kernel beyond the M0 set (`Time.now`, `String.length`, …).
                    // [SKY-L0108, feature: kernels]
                    (_, _) => Err(unsupported(callee.span, Feature::Kernels)),
                }
            }
            canon::Expr_::VarTopLevel { name, .. } => {
                // Every `VarTopLevel` was resolved by name resolution to a
                // declared top-level binding, all of which are registered in
                // `func_ids`; a miss is a violated invariant.
                let id = *self
                    .func_ids
                    .get(name)
                    .ok_or_else(|| bug("sky_lower::lower_callee", "unknown top-level binding"))?;
                Ok(Callee::Func(id))
            }
            // `lower_callee` resolves a *named* callee to its [`Callee`]; both
            // callers (the direct-call path in `lower_call` and the value-
            // reference arm in `lower_expr`) gate on `VarKernel`/`VarTopLevel`
            // before dispatching here, so any other shape is a violated
            // invariant, not a user-reachable feature gap. (A lambda or computed
            // callee applied as `(expr)(args)` lowers to [`Expr::Apply`]; a bare
            // lambda value stays an [`Expr::Lambda`].)
            _ => Err(bug(
                "sky_lower::lower_callee",
                "callee is neither a kernel nor a top-level name",
            )),
        }
    }

    fn binop(&self, func: Symbol, span: Span) -> DResult<BinOp> {
        match self.resolve(func)? {
            "add" => Ok(BinOp::Add),
            "sub" => Ok(BinOp::Sub),
            "mul" => Ok(BinOp::Mul),
            // `/` (`fdiv`) and `//` (`idiv`) both lower to the IR's `Div`; the
            // operand types (Float vs Int) settled by inference pick the Rust
            // semantics, matching the Go backend.
            "fdiv" | "idiv" => Ok(BinOp::Div),
            "eq" => Ok(BinOp::Eq),
            "neq" => Ok(BinOp::Neq),
            "lt" => Ok(BinOp::Lt),
            "gt" => Ok(BinOp::Gt),
            "le" => Ok(BinOp::Le),
            "ge" => Ok(BinOp::Ge),
            "and" => Ok(BinOp::And),
            "or" => Ok(BinOp::Or),
            // List / string operators (`++` → `append`, `::` → `cons`) await
            // those types. [SKY-L0101, feature: binops]
            _ => Err(unsupported(span, Feature::BinOps)),
        }
    }

    fn lower_case(&self, scrut: &canon::Expr, branches: &[canon::CaseBranch]) -> DResult<Expr> {
        let scrutinee = self.lower_expr(scrut)?;

        // The parser rejects a zero-branch `case` (CaseDefect::NoBranches), so
        // an empty branch list here is a violated invariant.
        let first = branches
            .first()
            .ok_or_else(|| bug("sky_lower::lower_case", "empty case expression"))?;
        let canon::Pattern_::PCtor { type_name, .. } = &first.pat.value else {
            // A wildcard/variable/literal arm. M0 matches only nullary
            // constructor patterns. [SKY-L0100, feature: case-pattern-kinds]
            return Err(unsupported(first.pat.span, Feature::CasePatternKinds));
        };
        // The scrutinee's enum is one this module declared (the type checker
        // pinned the constructor's union), so it is always in `enum_variants` —
        // the *true* variant set handed to `Match::new` below.
        let variants = self
            .enum_variants
            .get(type_name)
            .ok_or_else(|| bug("sky_lower::lower_case", "unknown scrutinee enum"))?;

        let arms = branches
            .iter()
            .map(|br| {
                let canon::Pattern_::PCtor {
                    type_name, name, ..
                } = &br.pat.value
                else {
                    // [SKY-L0100, feature: case-pattern-kinds]
                    return Err(unsupported(br.pat.span, Feature::CasePatternKinds));
                };
                Ok(Arm {
                    pat: Pat::Ctor {
                        ty: *type_name,
                        variant: *name,
                    },
                    body: self.lower_expr(&br.body)?,
                })
            })
            .collect::<DResult<Vec<_>>>()?;

        Ok(Expr::Match(Match::new(scrutinee, arms, variants)?))
    }
}
