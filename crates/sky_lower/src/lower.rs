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
}

impl<'a> Lowerer<'a> {
    pub fn new(m: &'a canon::Module, types: &'a SolvedTypes, interner: &'a Interner) -> Self {
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
    /// The lambda's solved region type is a curried arrow `T0 -> T1 -> … -> R`;
    /// exactly one arrow is peeled per parameter pattern, so the parameter count
    /// — not a full flatten — fixes the boxed closure's arity (a body that
    /// itself returns a function leaves that function as the lambda's `ret`,
    /// matching how a nested lambda lowers). Parameter patterns must be plain
    /// names (M1 has no parameter destructuring).
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
        for pat in params {
            let Ty::Fun(arg, rest) = cur else {
                // The lambda's inferred type has fewer arrows than it has
                // parameters — ruled out by inference (the lambda arm builds one
                // arrow per parameter), so reaching here is an invariant
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
        let ret = self.ir_type_from_ty(cur, span)?;
        let body = self.lower_expr(body)?;
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
            canon::Expr_::Call(callee, args) => self.lower_call(callee, args),
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
    fn lower_call(&self, callee: &canon::Expr, args: &[canon::Expr]) -> DResult<Expr> {
        let lowered_args = args
            .iter()
            .map(|a| self.lower_expr(a))
            .collect::<DResult<Vec<_>>>()?;
        match &callee.value {
            canon::Expr_::VarKernel { .. } | canon::Expr_::VarTopLevel { .. } => Ok(Expr::Call {
                callee: self.lower_callee(callee)?,
                args: lowered_args,
            }),
            _ => Ok(Expr::Apply {
                func: Box::new(self.lower_expr(callee)?),
                args: lowered_args,
            }),
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
