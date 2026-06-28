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

use std::collections::{BTreeMap, BTreeSet};

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, Feature, Located, LowerError, Span};
use sky_intern::{Interner, Symbol};
use sky_ir::{
    Arm, BinOp, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath, Module, Pat,
    Program, TypeDef, Variant,
};
use sky_types::{SolvedTypes, Ty};

/// One lowered function parameter: its (possibly synthetic) binder name and its
/// IR type.
type IrParam = (Symbol, IrType);

/// A tuple-parameter destructure-prologue entry: the synthetic binder name the
/// parameter was given, paired with the irrefutable tuple [`Pat`] that opens it
/// at the top of the function body (`let <Pat> = <synthetic>`).
type ParamPrologue = (Symbol, Pat);

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

/// Does this solved [`Ty`] contain a free type variable anywhere? Used to keep
/// the lowerer's record-shape collection to fully-concrete shapes — a
/// variable-bearing (generic) record reaches the backend through a signature,
/// where the type variable still has a source [`Symbol`] to name the generic.
fn ty_contains_var(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) => true,
        Ty::Unit => false,
        Ty::Fun(a, b) => ty_contains_var(a) || ty_contains_var(b),
        Ty::Tuple(elems) => elems.iter().any(ty_contains_var),
        Ty::Record(fields) => fields.values().any(ty_contains_var),
        Ty::Con { args, .. } => args.iter().any(ty_contains_var),
    }
}

/// Does this solved [`Ty`] contain a function type anywhere?
///
/// A field of a synthesised record struct whose type embeds a `Box<dyn Fn>`
/// cannot satisfy the struct's derived `Clone`/`Debug`/`PartialEq` nor its
/// `SkyStringify` impl — so the field type carrying a function is the unsound
/// shape. Used by [`embeds_nonderivable_function`] to test a payload field.
fn ty_contains_fun(ty: &Ty) -> bool {
    match ty {
        Ty::Fun(_, _) => true,
        Ty::Var(_) | Ty::Unit => false,
        Ty::Tuple(elems) => elems.iter().any(ty_contains_fun),
        Ty::Con { args, .. } => args.iter().any(ty_contains_fun),
        Ty::Record(fields) => fields.values().any(ty_contains_fun),
    }
}

/// Does this solved [`Ty`] embed a record field OR an enum payload whose type
/// contains a function?
///
/// A record synthesises to a Rust struct, and a user enum to a Rust enum, both
/// deriving `Clone`/`Debug`/`PartialEq` + `SkyStringify` — none of which a
/// `Box<dyn Fn>` field satisfies — so either would emit Rust that does not build.
/// The syntactic [`Lowerer::reject_function_valued_field`] gate only sees a
/// *literally* function-typed field value; this catches the case it misses — a
/// function value flowing into a record field or constructor payload THROUGH a
/// type variable, e.g. `wrap : a -> { value : a }` applied as `wrap (\n -> n +
/// 1)` (region `{ value : Int -> Int }`), or `Som (\n -> n + 1)` for
/// `type Opt a = Som a | Non` (region `Opt (Int -> Int)`). The field instantiates
/// to a function only at the use site, so the only place to see it is the use
/// site's region type. Fail-closed: a record-field carrier is the
/// first-class-function gap ([`Feature::FirstClassFunctions`], SKY-L0107) and a
/// constructor-payload carrier is [`Feature::CtorPayloadFunction`] (SKY-L0114) —
/// see [`con_payload_carries_function`]; never broken Rust.
fn embeds_nonderivable_function(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) | Ty::Unit => false,
        Ty::Fun(a, b) => embeds_nonderivable_function(a) || embeds_nonderivable_function(b),
        Ty::Tuple(elems) => elems.iter().any(embeds_nonderivable_function),
        // A `Con` here is a user enum (which derives `Clone`/`Debug`/`PartialEq`
        // + `SkyStringify`) applied to its type arguments. A function reaching a
        // payload field — directly (`Opt (Int -> Int)`) or nested inside another
        // payload/record under it — makes those derives fail, so it is the same
        // non-derivable shape as a function in a record field.
        Ty::Con { args, .. } => args
            .iter()
            .any(|a| ty_contains_fun(a) || embeds_nonderivable_function(a)),
        Ty::Record(fields) => fields
            .values()
            .any(|f| ty_contains_fun(f) || embeds_nonderivable_function(f)),
    }
}

/// Is the carrier of a non-derivable function a CONSTRUCTOR payload — i.e. the
/// region type's head is a user enum (`Ty::Con`) whose type arguments embed a
/// function?
///
/// This distinguishes the two carriers [`embeds_nonderivable_function`] flags so
/// the diagnostic names the right one: a `Con`-headed region is a
/// constructor-payload function (SKY-L0114, [`Feature::CtorPayloadFunction`]); a
/// `Record`-headed region (or any other) is a record-field function (SKY-L0107,
/// [`Feature::FirstClassFunctions`]). Only the *head* is inspected — the gate
/// has already confirmed a function is embedded somewhere; this picks the
/// blame label, so the outermost carrier is the one named.
fn con_payload_carries_function(ty: &Ty) -> bool {
    matches!(ty, Ty::Con { args, .. }
        if args.iter().any(|a| ty_contains_fun(a) || embeds_nonderivable_function(a)))
}

/// Collect every type-variable [`Symbol`] mentioned in a canonical type into
/// `out`. Used to verify a constructor field's type variables are all bound by
/// the union's declared parameters before lowering the field.
fn collect_type_vars(t: &canon::Type, out: &mut BTreeSet<Symbol>) {
    match t {
        canon::Type::Var(s) => {
            out.insert(*s);
        }
        canon::Type::Unit => {}
        canon::Type::Lambda(a, b) => {
            collect_type_vars(a, out);
            collect_type_vars(b, out);
        }
        canon::Type::Tuple(elems) => {
            for e in elems {
                collect_type_vars(e, out);
            }
        }
        canon::Type::Con { args, .. } => {
            for a in args {
                collect_type_vars(a, out);
            }
        }
        canon::Type::Record(fields) => {
            for (_, fty) in fields {
                collect_type_vars(fty, out);
            }
        }
    }
}

/// Does this IR type embed a function type anywhere? An enum variant whose
/// payload field carries a `Box<dyn Fn>` cannot satisfy the enum's derived
/// `Clone`/`Debug`/`PartialEq` nor its `SkyStringify` impl, so a function-bearing
/// field is the fail-closed first-class gap.
fn ir_contains_fun(ty: &IrType) -> bool {
    match ty {
        IrType::Fun(_, _) => true,
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Generic(_) => false,
        IrType::Enum { args, .. } => args.iter().any(ir_contains_fun),
        IrType::Tuple(elems) => elems.iter().any(ir_contains_fun),
        IrType::Record(fields) => fields.values().any(ir_contains_fun),
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
    /// Each constructor's declared payload arity, keyed by constructor name. A
    /// saturated construction passes exactly this many arguments; a bare or
    /// partially-applied payload constructor is the constructor-as-function gap.
    ctor_arity: BTreeMap<Symbol, usize>,
    /// Pre-minted, collision-free parameter names for eta-expanding a partial
    /// application into a boxed closure. Sized in [`crate::lower`] to the widest
    /// function arity in the module — an eta-lambda introduces at most that many
    /// params — so position `i` of the pool names the i-th synthesised parameter.
    /// Each eta-lambda is its own closure scope, so the same pool entry is reused
    /// across sites without shadowing; [`Interner::fresh_symbols`] guarantees no
    /// entry aliases a user identifier.
    eta_params: Vec<Symbol>,
    /// Pre-minted, collision-free binder names for a tuple-destructuring
    /// function parameter. A parameter pattern `(a, b)` has no single name, so
    /// the lowerer gives the parameter a synthetic name from this pool (position
    /// `i` names the i-th parameter) and prepends a `Destructure` binding
    /// `let (a, b) = <synthetic>` to the body. Sized to the widest function
    /// arity in the module — the most parameters any binding can carry, hence
    /// the most synthetic binders one function can need — through the one
    /// `&mut Interner` the entry point owns. Each function is its own scope, so
    /// the pool is reused positionally across functions without collision;
    /// [`Interner::fresh_symbols`] guarantees the names dodge every user
    /// identifier and each other.
    param_binders: Vec<Symbol>,
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
        param_binders: Vec<Symbol>,
    ) -> Self {
        let mut func_ids = BTreeMap::new();
        for (idx, def) in m.defs.iter().enumerate() {
            let id = FuncId::from_raw(u32::try_from(idx).unwrap_or(u32::MAX));
            func_ids.insert(def.name().value, id);
        }

        let mut enum_variants = BTreeMap::new();
        let mut ctor_arity = BTreeMap::new();
        for union in &m.unions {
            enum_variants.insert(union.name, union.ctors.iter().map(|c| c.name).collect());
            for ctor in &union.ctors {
                ctor_arity.insert(ctor.name, ctor.arity);
            }
        }

        Self {
            m,
            types,
            interner,
            func_ids,
            enum_variants,
            ctor_arity,
            eta_params,
            param_binders,
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
        let mut types_ir: Vec<TypeDef> = Vec::with_capacity(self.m.unions.len());
        for u in &self.m.unions {
            types_ir.push(TypeDef::Enum(self.lower_enum(u)?));
        }

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

    /// Lower a union declaration into the IR enum: its quantified type variables
    /// become `type_params` (declaration order is load-bearing — the backend
    /// derives each parameter's Rust generic name from its position), and each
    /// constructor becomes a [`Variant`] whose declared payload field types lower
    /// under that generic scope.
    ///
    /// Two fail-closed gates run per constructor, both surfaced as a
    /// span-carrying [`Diagnostic::Lower`] rather than emitting Rust that cargo
    /// rejects:
    ///
    /// * a field type variable not bound by the union's parameters (`type Foo a =
    ///   Bar b`) would have no Rust generic to resolve to — the polymorphism gap
    ///   ([`Feature::Polymorphism`]);
    /// * a field whose type embeds a function (`type Box = Mk (Int -> Int)`)
    ///   would make the enum's derived `Clone`/`Debug`/`PartialEq` /
    ///   `SkyStringify` fail to hold for a `Box<dyn Fn>` field — the
    ///   constructor-payload-function gap ([`Feature::CtorPayloadFunction`]).
    fn lower_enum(&self, u: &canon::Union) -> DResult<EnumDef> {
        let type_params = u.vars.clone();
        let mut variants = Vec::with_capacity(u.ctors.len());
        for ctor in &u.ctors {
            let mut fields = Vec::with_capacity(ctor.args.len());
            for arg in &ctor.args {
                // Gate 1: every field type variable must be one the union
                // quantifies, so it resolves to a Rust generic by position.
                let mut vars = BTreeSet::new();
                collect_type_vars(arg, &mut vars);
                if !vars.iter().all(|v| type_params.contains(v)) {
                    return Err(unsupported(ctor.span, Feature::Polymorphism));
                }
                let ir = self.ir_type_from_canon(arg, &type_params)?;
                // Gate 2: a function-bearing payload field cannot satisfy the
                // enum's derives. The carrier is a constructor payload, so blame
                // the constructor declaration with the payload-specific message
                // (SKY-L0114) rather than the record-field one.
                if ir_contains_fun(&ir) {
                    return Err(unsupported(ctor.span, Feature::CtorPayloadFunction));
                }
                fields.push(ir);
            }
            variants.push(Variant {
                name: ctor.name,
                fields,
            });
        }
        Ok(EnumDef {
            name: u.name,
            type_params,
            variants,
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
                // Only a FULLY-CONCRETE record shape is surfaced here. A record
                // carrying a type variable is a generic shape that necessarily
                // appears in a (polymorphic) signature — the backend synthesises
                // and reconciles the generic struct from `func.params` / `func.ret`
                // there. Surfacing it again from the solved region/env type would
                // be redundant and, worse, has no source-level [`Symbol`] to name
                // the generic (the solver's variable id is not a source symbol),
                // so [`Self::ir_type_from_ty`] would reject the bare `Ty::Var`
                // field as an under-determined polymorphic value. Skipping it is
                // sound: an unannotated binding can never be generic (M0 rejects an
                // untyped binding with parameters), so every genuinely-generic
                // record reaches the backend through a signature.
                if !ty_contains_var(ty) {
                    let ir = self.ir_type_from_ty(ty, Span::DUMMY)?;
                    if !out.contains(&ir) {
                        out.push(ir);
                    }
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
                patterns,
                body,
                ty,
                free_vars,
                ..
            } => {
                // M2a: a typed binding's free type variables are the type
                // parameters it quantifies. Every variable appearing in the
                // annotation is one of them (canon collects the complete set,
                // ordered deterministically by name), so each `Type::Var` in the
                // signature lowers to an `IrType::Generic` and the backend emits
                // `pub fn name<T1, T2, ..>(..)`. The body of a well-typed
                // parametric binding uses these variables only structurally
                // (pure pass-through) — the type checker's rigid-skolem gate
                // already rejects any body that pins a variable to a concrete or
                // super-typed shape (`f : a -> a ; f x = x + 1`), so a function
                // that reaches lowering with `free_vars` is a true parametric
                // pass-through and never needs a Rust trait bound. An empty
                // `free_vars` keeps the function monomorphic, byte-identical to
                // M0/M1.
                let (params, prologue, ret) = self.split_typed_sig(ty, patterns, free_vars)?;
                // A tuple-destructuring parameter binds its synthetic name to the
                // tuple, then the body opens it with a `Destructure`. Fold the
                // prologue OUTERMOST-first (reverse) so the first parameter's
                // destructure is the outermost binding, matching source order.
                let mut lowered_body = self.lower_expr(body)?;
                for (binder_sym, binder_pat) in prologue.into_iter().rev() {
                    lowered_body = Expr::Destructure {
                        binder: binder_pat,
                        value: Box::new(Expr::Var(binder_sym)),
                        body: Box::new(lowered_body),
                    };
                }
                Ok(Func {
                    id,
                    name,
                    type_params: free_vars.clone(),
                    params,
                    ret,
                    body: lowered_body,
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
                    type_params: Vec::new(),
                    params: Vec::new(),
                    ret,
                    body: self.lower_expr(body)?,
                })
            }
        }
    }

    /// Split a typed binding's arrow annotation into one [`IrType`] per
    /// parameter pattern plus the trailing return type. `generics` is the
    /// binding's quantified type-variable set ([`canon::Def::Typed::free_vars`]),
    /// so each annotation `Type::Var` it contains lowers to an
    /// [`IrType::Generic`] rather than being rejected as monomorphic.
    ///
    /// Returns `(params, prologue, ret)`. A plain variable parameter contributes
    /// `(name, ty)` to `params` directly. A TUPLE parameter `(a, b)` has no
    /// single name: it contributes a synthetic binder name to `params` and a
    /// `(synthetic, tuple Pat)` entry to `prologue`, which [`Self::lower_def`]
    /// turns into a `Destructure` wrapping the body. `prologue` is in source
    /// (parameter) order.
    fn split_typed_sig(
        &self,
        ty: &canon::Type,
        patterns: &[canon::Pattern],
        generics: &[Symbol],
    ) -> DResult<(Vec<IrParam>, Vec<ParamPrologue>, IrType)> {
        let mut cur = ty;
        let mut params = Vec::with_capacity(patterns.len());
        let mut prologue = Vec::new();
        for (i, pat) in patterns.iter().enumerate() {
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
            let ir_ty = self.ir_type_from_canon(arg, generics)?;
            match &pat.value {
                canon::Pattern_::PTuple(_) => {
                    // A tuple parameter: name it with a fresh synthetic binder
                    // (one per parameter position) and record the destructure for
                    // the body prologue. The pool is sized to the widest function
                    // arity, so position `i` is always present; a missing slot is
                    // an internal invariant violation.
                    let synthetic = *self.param_binders.get(i).ok_or_else(|| {
                        bug(
                            "sky_lower::split_typed_sig",
                            "tuple-parameter binder pool exhausted",
                        )
                    })?;
                    params.push((synthetic, ir_ty));
                    prologue.push((synthetic, Self::lower_destructure_pat(pat)?));
                }
                // A plain variable parameter. A wildcard / constructor parameter
                // is parameter-destructuring the lowerer does not model yet
                // (SKY-L0105); `pattern_var` makes that report.
                _ => params.push((Self::pattern_var(pat)?, ir_ty)),
            }
            cur = rest.as_ref();
        }
        // The trailing type is the return type.
        Ok((params, prologue, self.ir_type_from_canon(cur, generics)?))
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
    /// annotations) into an [`IrType`]. `generics` is the enclosing binding's
    /// quantified type-variable set: a `Type::Var` it contains is a parametric
    /// pass-through and lowers to [`IrType::Generic`] (M2a).
    ///
    /// Every failure here is an internal-invariant violation (a `Type::Con` that
    /// resolves to neither a builtin nor a declared union, or a `Type::Var`
    /// missing from the binding's free-variable set), so no node `span` is
    /// threaded — those are [`bug`]s, not span-carrying feature gaps.
    fn ir_type_from_canon(&self, t: &canon::Type, generics: &[Symbol]) -> DResult<IrType> {
        match t {
            // A type-constructor application. A builtin (`Int`, `Bool`, …) carries
            // no args; a user enum carries its type arguments, each lowered under
            // the same generic scope so `Opt Int` → `Enum { Opt, [Int] }` and
            // `Opt a` (inside a generic signature) → `Enum { Opt, [Generic a] }`.
            canon::Type::Con { name, args, .. } => match self.resolve(*name)? {
                "Int" => Ok(IrType::Int),
                "Float" => Ok(IrType::Float),
                "Bool" => Ok(IrType::Bool),
                "String" => Ok(IrType::Str),
                _ if self.enum_variants.contains_key(name) => {
                    let mut ir_args = Vec::with_capacity(args.len());
                    for a in args {
                        ir_args.push(self.ir_type_from_canon(a, generics)?);
                    }
                    Ok(IrType::Enum {
                        name: *name,
                        args: ir_args,
                    })
                }
                other => Err(bug(
                    "sky_lower::ir_type_from_canon",
                    format!("unknown type constructor `{other}`"),
                )),
            },
            // A function type in argument/return position of a value annotation
            // (`apply : (Int -> Int) -> Int`). Flatten the curried arrow chain
            // into one boxed `Fn` value type `Fun([T0, …], R)`.
            canon::Type::Lambda(_, _) => {
                let mut params = Vec::new();
                let mut cur = t;
                while let canon::Type::Lambda(arg, rest) = cur {
                    params.push(self.ir_type_from_canon(arg, generics)?);
                    cur = rest.as_ref();
                }
                let ret = self.ir_type_from_canon(cur, generics)?;
                Ok(IrType::Fun(params, Box::new(ret)))
            }
            // A type variable in an annotation (`id : a -> a`). When the
            // enclosing binding quantifies it (M2a — a fully-parametric
            // function), it lowers to an [`IrType::Generic`] pass-through. Every
            // variable appearing in the annotation is in `free_vars` by
            // construction, so a variable absent from `generics` here means canon
            // failed to collect the binding's complete free-variable set — a
            // violated invariant, not a user-reachable feature gap.
            canon::Type::Var(v) => {
                if generics.contains(v) {
                    Ok(IrType::Generic(*v))
                } else {
                    Err(bug(
                        "sky_lower::ir_type_from_canon",
                        "annotation type variable not in the binding's free-variable set",
                    ))
                }
            }
            // The unit type `()` in an annotation (`f : () -> Int`).
            canon::Type::Unit => Ok(IrType::Unit),
            // A tuple type in an annotation (`fst : (a, b) -> a`). Lower element-
            // wise; the invariant (arity ≥ 2) is upheld by the parser.
            canon::Type::Tuple(elems) => {
                let mut ir_elems = Vec::with_capacity(elems.len());
                for e in elems {
                    ir_elems.push(self.ir_type_from_canon(e, generics)?);
                }
                Ok(IrType::Tuple(ir_elems))
            }
            // A closed record type in an annotation (`wrap : a -> { value : a }`).
            // Each field type is lowered under the same generic scope, so a field
            // typed by a quantified variable becomes an [`IrType::Generic`]
            // pass-through and the backend synthesises a GENERIC struct for the
            // shape (M2c). Keyed by field name in a [`BTreeMap`] to match the
            // backend's field-set canonicalisation.
            canon::Type::Record(fields) => {
                let mut ir_fields = BTreeMap::new();
                for (name, fty) in fields {
                    ir_fields.insert(*name, self.ir_type_from_canon(fty, generics)?);
                }
                Ok(IrType::Record(ir_fields))
            }
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
                _ if self.enum_variants.contains_key(name) => {
                    // A use-site enum type carries its solved type arguments, so
                    // `Opt Int` → `Enum { Opt, [Int] }` (rendered `MainOpt<i64>`).
                    let mut ir_args = Vec::with_capacity(args.len());
                    for a in args {
                        ir_args.push(self.ir_type_from_ty(a, span)?);
                    }
                    Ok(IrType::Enum {
                        name: *name,
                        args: ir_args,
                    })
                }
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
            // A type variable in value position. With M2a, a binding can be
            // genuinely parametric, so a region the solver left as a bare
            // variable is an under-determined polymorphic value the lowerer
            // cannot monomorphise here yet — e.g. a polymorphic function
            // referenced as a first-class value whose type never gets pinned to a
            // concrete instance at the use site. That is a real M2a feature gap
            // (the value's Rust type would itself have to be generic in a
            // position the backend does not yet model), not an invariant
            // violation, so it surfaces as a `Diagnostic::Lower` with the span —
            // never a `CompilerBug` for well-typed input.
            // [SKY-L0102, feature: polymorphism]
            Ty::Var(_) => Err(unsupported(span, Feature::Polymorphism)),
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

    /// Soundness gate (region-based): reject a function value reaching a record
    /// field OR a constructor payload THROUGH a type variable — e.g.
    /// `wrap : a -> { value : a }` applied as `wrap (\n -> n + 1)` (region
    /// `{ value : Int -> Int }`), or `Som (\n -> n + 1)` for
    /// `type Opt a = Som a | Non` (region `Opt (Int -> Int)`). The field
    /// instantiates to a function only at the use site, so the syntactic
    /// per-field gate ([`Self::reject_function_valued_field`]) cannot see it; the
    /// use-site region type can. Record/Update *literals* carry their own
    /// per-field gate that blames the offending field value's span, so they are
    /// exempt here.
    ///
    /// The diagnostic names the carrier: a function reaching a CONSTRUCTOR
    /// payload (region head is a user enum `Con`) gets the constructor-payload
    /// message blaming this construction site (SKY-L0114,
    /// [`Feature::CtorPayloadFunction`]); a function reaching a RECORD field gets
    /// the record-field message (SKY-L0107, [`Feature::FirstClassFunctions`]).
    fn reject_function_through_type_var(&self, e: &canon::Expr) -> DResult<()> {
        if !matches!(
            &e.value,
            canon::Expr_::Record(_) | canon::Expr_::Update(_, _)
        ) && let Some(ty) = self.types.regions.get(&e.span)
            && embeds_nonderivable_function(ty)
        {
            let feature = if con_payload_carries_function(ty) {
                Feature::CtorPayloadFunction
            } else {
                Feature::FirstClassFunctions
            };
            return Err(unsupported(e.span, feature));
        }
        Ok(())
    }

    fn lower_expr(&self, e: &canon::Expr) -> DResult<Expr> {
        self.reject_function_through_type_var(e)?;
        match &e.value {
            canon::Expr_::Int(n) => Ok(Expr::Int(*n)),
            canon::Expr_::Unit => Ok(Expr::Unit),
            canon::Expr_::VarLocal(s) => Ok(Expr::Var(*s)),
            canon::Expr_::VarCtor {
                type_name, name, ..
            } => {
                // A bare constructor reference. A nullary constructor is its own
                // zero-payload value (`Nothing`, `Leaf`); a payload constructor
                // referenced without arguments is a constructor-as-function value,
                // which awaits first-class-value support (a saturated construction
                // is handled in `lower_call`).
                let arity = self.ctor_arity_of(*name)?;
                if arity == 0 {
                    Ok(Expr::Ctor {
                        ty: *type_name,
                        variant: *name,
                        args: vec![],
                    })
                } else {
                    Err(unsupported(e.span, Feature::CtorAsFunction))
                }
            }
            canon::Expr_::Binop { func, lhs, rhs, .. } => Ok(Expr::BinOp {
                op: self.binop(*func, e.span)?,
                lhs: Box::new(self.lower_expr(lhs)?),
                rhs: Box::new(self.lower_expr(rhs)?),
            }),
            canon::Expr_::Call(callee, args) => self.lower_call(callee, args, e.span),
            canon::Expr_::Lambda(params, body) => self.lower_lambda(params, body, e.span),
            canon::Expr_::Let(bindings, body) => self.lower_let(bindings, body),
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
            canon::Expr_::Update(base, fields) => self.lower_update(base, fields),
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

    /// Lower a functional record update `{ base | field = value, ... }` to a copy
    /// of `base` with the listed fields replaced. Only the changed fields are
    /// carried, sorted by field name so the lowering is deterministic; the backend
    /// names each reassignment, so write order is free. The result's record struct
    /// is the base's, already surfaced via `Module.records` from the base region's
    /// solved type.
    ///
    /// M2c gate: updating a GENERIC record (a field typed by a quantified type
    /// variable) needs a `Clone`-bounded type parameter, because the backend
    /// copies the base with `.clone()`. Bounded generics are M2d, so a generic
    /// record update is a not-yet gap ([`Feature::BoundedRecordUpdate`],
    /// SKY-L0111) rather than broken Rust. The base's solved region type tells us
    /// whether it is generic; a monomorphic update is byte-identical to b3.
    fn lower_update(&self, base: &canon::Expr, fields: &[(Symbol, canon::Expr)]) -> DResult<Expr> {
        if let Some(base_ty) = self.types.regions.get(&base.span)
            && ty_contains_var(base_ty)
        {
            return Err(unsupported(base.span, Feature::BoundedRecordUpdate));
        }
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
            canon::Expr_::VarCtor {
                type_name, name, ..
            } => {
                // A constructor application. M3a lowers a *saturated* construction
                // to `Expr::Ctor`; a partial application (`Node l 1` for a
                // three-field `Node`) is a constructor-as-function value, which
                // awaits first-class-value support. Over-application is ruled out
                // by type-checking (applying past the fields makes the result a
                // non-function), so a non-equal count here is always partial.
                let arity = self.ctor_arity_of(*name)?;
                if args.len() == arity {
                    Ok(Expr::Ctor {
                        ty: *type_name,
                        variant: *name,
                        args: lowered_args,
                    })
                } else {
                    Err(unsupported(call_span, Feature::CtorAsFunction))
                }
            }
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

    /// The declared payload arity of a constructor. Name resolution guarantees
    /// every `VarCtor` / ctor pattern names a declared constructor, so a miss is a
    /// violated invariant rather than user error.
    fn ctor_arity_of(&self, name: Symbol) -> DResult<usize> {
        self.ctor_arity
            .get(&name)
            .copied()
            .ok_or_else(|| bug("sky_lower::ctor_arity_of", "unknown constructor"))
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

    /// Lower a constructor payload sub-pattern. M3a binds a payload field to a
    /// variable or ignores it with `_`; M3b-1 also admits a TUPLE payload of
    /// those (`Just (a, b)`), lowered element-wise. A nested constructor /
    /// literal / record / cons sub-pattern is the nested-payload gap (SKY-L0112),
    /// surfaced fail-closed rather than mis-lowered.
    fn lower_payload_pat(p: &canon::Pattern) -> DResult<Pat> {
        match &p.value {
            canon::Pattern_::PVar(s) => Ok(Pat::Var(*s)),
            canon::Pattern_::PAnything => Ok(Pat::Wildcard),
            canon::Pattern_::PTuple(elems) => {
                let subs = elems
                    .iter()
                    .map(Self::lower_payload_pat)
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Pat::Tuple(subs))
            }
            // M3b-2: a nested constructor sub-pattern (`Just (Just a)`,
            // `Node (Node …) x r`). The canonical pattern already carries the
            // resolved `type_name` / variant / sub-patterns, so the IR
            // `Pat::Ctor` is built directly and recurses. Whether the resulting
            // (refutable) nested shape is exhaustive is the exhaustiveness
            // checker's call (SKY-T0010); a second arm for the same top-level
            // constructor is gated separately (SKY-L0116).
            canon::Pattern_::PCtor {
                type_name,
                name,
                args,
                ..
            } => {
                let subs = args
                    .iter()
                    .map(Self::lower_payload_pat)
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Pat::Ctor {
                    ty: *type_name,
                    variant: *name,
                    args: subs,
                })
            }
            // A record sub-pattern nested in a constructor payload needs the
            // payload field's record type threaded here to recover the complete
            // field set; not yet plumbed. [SKY-L0112]
            canon::Pattern_::PRecord(_) => Err(unsupported(p.span, Feature::NestedPayloadPatterns)),
        }
    }

    /// Lower an IRREFUTABLE destructuring binder — a function-parameter pattern
    /// or a single-arm tuple `case` pattern. A variable / wildcard / nested
    /// tuple of those always matches, so the resulting `Destructure` (or a
    /// tuple function parameter) is a sound, exhaustive Rust binding. A
    /// REFUTABLE element — a constructor (a literal once those land) — could
    /// fail to match and is the tuple-pattern gap (SKY-L0115), surfaced
    /// fail-closed rather than emitted as a refutable `let`.
    fn lower_destructure_pat(p: &canon::Pattern) -> DResult<Pat> {
        match &p.value {
            canon::Pattern_::PVar(s) => Ok(Pat::Var(*s)),
            canon::Pattern_::PAnything => Ok(Pat::Wildcard),
            canon::Pattern_::PTuple(elems) => {
                let subs = elems
                    .iter()
                    .map(Self::lower_destructure_pat)
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Pat::Tuple(subs))
            }
            canon::Pattern_::PCtor { .. } => Err(unsupported(p.span, Feature::TuplePatternMatch)),
            // A record pattern nested inside a tuple destructure needs the
            // element's record type to recover the complete field set; only a
            // top-level record binder is supported (via `lower_binder_pat`).
            // [SKY-L0112]
            canon::Pattern_::PRecord(_) => Err(unsupported(p.span, Feature::NestedPayloadPatterns)),
        }
    }

    /// Lower an irrefutable destructure binder — the LHS of a `let` destructure
    /// or the single arm of a tuple / record `case`. Variables, wildcards, and
    /// nested irrefutable tuples lower structurally via [`Self::lower_destructure_pat`];
    /// a top-level RECORD binder resolves its synthesised struct from `value`'s
    /// solved record type, so the COMPLETE field set (each pattern field a binder,
    /// every other field a wildcard) reaches the backend exactly as a record
    /// literal does. `value` is the canonical expression bound (the `let` body or
    /// the `case` scrutinee); its region type supplies the record shape.
    fn lower_binder_pat(&self, pat: &canon::Pattern, value: &canon::Expr) -> DResult<Pat> {
        match &pat.value {
            canon::Pattern_::PRecord(fields) => {
                let ty = self.types.regions.get(&value.span).ok_or_else(|| {
                    bug(
                        "sky_lower::lower_binder_pat",
                        "record destructure value has no solved region type",
                    )
                })?;
                self.lower_record_pat(fields, ty, pat.span)
            }
            _ => Self::lower_destructure_pat(pat),
        }
    }

    /// Build a [`Pat::Record`] from a field-pun record pattern and the scrutinee's
    /// solved record type. The pattern names a subset of the record's fields
    /// (`{ x }` on a `{ x, y }` record is legal); the COMPLETE field set is
    /// emitted — each named field a [`Pat::Var`] binder, every other field a
    /// [`Pat::Wildcard`] — so the backend resolves the struct from the full
    /// field-name set, exactly as a record literal does. Entries are ordered by
    /// resolved field name for deterministic output.
    fn lower_record_pat(&self, fields: &[Located<Symbol>], ty: &Ty, span: Span) -> DResult<Pat> {
        let Ty::Record(rec) = ty else {
            // A record pattern whose scrutinee did not solve to a record type.
            // The type checker proves the scrutinee is a record before this runs,
            // so reaching here is fail-closed defence rather than a live path.
            return Err(unsupported(span, Feature::NestedPayloadPatterns));
        };
        let bound: BTreeSet<Symbol> = fields.iter().map(|f| f.value).collect();
        let mut entries: Vec<(Symbol, Pat)> = Vec::with_capacity(rec.len());
        for field in rec.keys() {
            let sub = if bound.contains(field) {
                Pat::Var(*field)
            } else {
                Pat::Wildcard
            };
            entries.push((*field, sub));
        }
        entries.sort_by(|a, b| {
            self.resolve(a.0)
                .unwrap_or("")
                .cmp(self.resolve(b.0).unwrap_or(""))
        });
        Ok(Pat::Record(entries))
    }

    /// Lower a `let … in body`. A multi-binding `let` becomes right-nested
    /// single-binding IR nodes (`let a = …; b = … in body` → `Let a (Let b body)`),
    /// matching the sequential (`let*`) scoping that canonicalisation and
    /// inference established. A plain `name = value` binding stays the audited
    /// single-symbol [`Expr::Let`]; an irrefutable destructure (`(a, b) = e`,
    /// `{ x } = e`, `_ = e`) lowers to an [`Expr::Destructure`] whose binder is
    /// built by [`Self::lower_binder_pat`] (a refutable binder is rejected there).
    fn lower_let(&self, bindings: &[canon::LetBinding], body: &canon::Expr) -> DResult<Expr> {
        let mut acc = self.lower_expr(body)?;
        for b in bindings.iter().rev() {
            let value = self.lower_expr(&b.body)?;
            acc = match &b.pat.value {
                canon::Pattern_::PVar(name) => Expr::Let {
                    name: *name,
                    value: Box::new(value),
                    body: Box::new(acc),
                },
                _ => Expr::Destructure {
                    binder: self.lower_binder_pat(&b.pat, &b.body)?,
                    value: Box::new(value),
                    body: Box::new(acc),
                },
            };
        }
        Ok(acc)
    }

    fn lower_case(&self, scrut: &canon::Expr, branches: &[canon::CaseBranch]) -> DResult<Expr> {
        let scrutinee = self.lower_expr(scrut)?;

        // The parser rejects a zero-branch `case` (CaseDefect::NoBranches), so
        // an empty branch list here is a violated invariant.
        let first = branches
            .first()
            .ok_or_else(|| bug("sky_lower::lower_case", "empty case expression"))?;
        // A tuple- or record-pattern arm is an irrefutable destructure, not an
        // enum match. Exactly one such arm (`case (1, 2) of (a, b) -> …`,
        // `case r of { x, y } -> …`) lowers to a `Destructure` binding rather
        // than an `Expr::Match`. More than one arm would need product
        // exhaustiveness, the tuple-pattern gap (SKY-L0115).
        if matches!(
            &first.pat.value,
            canon::Pattern_::PTuple(_) | canon::Pattern_::PRecord(_)
        ) {
            if branches.len() != 1 {
                return Err(unsupported(first.pat.span, Feature::TuplePatternMatch));
            }
            let binder = self.lower_binder_pat(&first.pat, scrut)?;
            return Ok(Expr::Destructure {
                binder,
                value: Box::new(scrutinee),
                body: Box::new(self.lower_expr(&first.body)?),
            });
        }
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

        // M3b-2 lowers a constructor `case` to a Rust `match` with one arm per
        // top-level constructor. A SECOND arm for the same constructor is nested
        // constructor discrimination — gated fail-closed here (SKY-L0116) before
        // `Match::new`, so the unsupported shape never reaches its exactly-once
        // contract as a `CompilerBug`. The exhaustiveness checker has already run
        // (a non-exhaustive nested `case` surfaced as SKY-T0010), so an
        // exhaustive two-arm shape is the one this gate catches.
        let mut seen: BTreeSet<Symbol> = BTreeSet::new();
        for br in branches {
            if let canon::Pattern_::PCtor { name, .. } = &br.pat.value
                && !seen.insert(*name)
            {
                return Err(unsupported(br.pat.span, Feature::NestedCtorDiscrimination));
            }
        }

        let arms = branches
            .iter()
            .map(|br| {
                let canon::Pattern_::PCtor {
                    type_name,
                    name,
                    args,
                    ..
                } = &br.pat.value
                else {
                    // [SKY-L0100, feature: case-pattern-kinds]
                    return Err(unsupported(br.pat.span, Feature::CasePatternKinds));
                };
                // Each payload sub-pattern binds a variable or is a wildcard;
                // richer shapes are the nested-payload gap (SKY-L0112). The type
                // checker (SKY-T0013) already proved the sub-pattern count equals
                // the constructor's field count, so the IR `Pat::Ctor` the backend
                // sees always matches the variant's declared arity.
                let sub = args
                    .iter()
                    .map(Self::lower_payload_pat)
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Arm {
                    pat: Pat::Ctor {
                        ty: *type_name,
                        variant: *name,
                        args: sub,
                    },
                    body: self.lower_expr(&br.body)?,
                })
            })
            .collect::<DResult<Vec<_>>>()?;

        Ok(Expr::Match(Match::new(scrutinee, arms, variants)?))
    }
}
