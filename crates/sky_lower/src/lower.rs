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
    Arm, BinOp, BoundSet, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath,
    Module, Pat, Program, TypeDef, Variant,
};
use sky_types::{SolvedTypes, Ty, TyBounds};

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

/// The `Maybe a` type carries exactly one argument; an arity-1 guard cleared it,
/// so a missing first argument here is an unreachable internal invariant.
fn maybe_arg_bug() -> Diagnostic {
    bug(
        "sky_lower::ir_type",
        "Maybe applied without its element type",
    )
}

/// The `Result e a` type carries exactly two arguments; an arity-2 guard cleared
/// them, so a missing argument here is an unreachable internal invariant.
fn result_arg_bug() -> Diagnostic {
    bug(
        "sky_lower::ir_type",
        "Result applied without its error/success types",
    )
}

/// The `List a` type carries exactly one argument; an arity-1 guard cleared it,
/// so a missing element type here is an unreachable internal invariant.
fn list_arg_bug() -> Diagnostic {
    bug(
        "sky_lower::ir_type",
        "List applied without its element type",
    )
}

/// The `Dict k v` type carries exactly two arguments; an arity-2 guard cleared
/// them, so a missing argument here is an unreachable internal invariant.
fn dict_arg_bug() -> Diagnostic {
    bug(
        "sky_lower::ir_type",
        "Dict applied without its key/value types",
    )
}

/// The `Set a` type carries exactly one argument; an arity-1 guard cleared it,
/// so a missing element type here is an unreachable internal invariant.
fn set_arg_bug() -> Diagnostic {
    bug("sky_lower::ir_type", "Set applied without its element type")
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
        | IrType::Char
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Bytes
        | IrType::Json
        // `Decoder<T>` is an opaque struct, not a function type.
        | IrType::Decoder(_)
        | IrType::Generic(_) => false,
        IrType::Enum { args, .. } => args.iter().any(ir_contains_fun),
        IrType::Maybe(elem) | IrType::List(elem) => ir_contains_fun(elem),
        IrType::Result(err, ok) => ir_contains_fun(err) || ir_contains_fun(ok),
        IrType::Dict(k, v) => ir_contains_fun(k) || ir_contains_fun(v),
        IrType::Set(a) => ir_contains_fun(a),
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

/// The interned symbols of the built-in `Maybe` / `Result` types and their
/// constructors, minted by [`crate::lower`] through its owned `&mut Interner`.
///
/// These constructors (`Just` / `Nothing` / `Ok` / `Err`) are Prelude built-ins,
/// not user `type` declarations, so the lowerer cannot discover their variant
/// sets or payload arities from `module.unions`. Threading the symbols in lets
/// [`Lowerer::new`] seed `enum_variants` (the variant set [`Match::new`] needs to
/// prove a `Maybe` / `Result` `case` exhaustive) and `ctor_arity` (the field
/// count a saturated `Just x` / `Ok x` passes) for them, exactly as it does for a
/// user enum.
pub struct BuiltinCtors {
    pub maybe: Symbol,
    pub result: Symbol,
    pub just: Symbol,
    pub nothing: Symbol,
    pub ok: Symbol,
    pub err: Symbol,
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
        builtins: &BuiltinCtors,
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
        // Seed the built-in `Maybe` / `Result` variant sets + payload arities so
        // a `case m of Just x -> … ; Nothing -> …` takes the same validated
        // `Match::new` enum-cover path a user enum does, and `Just x` / `Ok x`
        // lower as saturated constructions.
        enum_variants.insert(builtins.maybe, vec![builtins.just, builtins.nothing]);
        enum_variants.insert(builtins.result, vec![builtins.ok, builtins.err]);
        ctor_arity.insert(builtins.just, 1);
        ctor_arity.insert(builtins.nothing, 0);
        ctor_arity.insert(builtins.ok, 1);
        ctor_arity.insert(builtins.err, 1);

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
                // A typed binding's free type variables are the type parameters
                // it quantifies. Every variable appearing in the annotation is
                // one of them (canon collects the complete set, ordered
                // deterministically by name), so each `Type::Var` in the
                // signature lowers to an `IrType::Generic` and the backend emits
                // `pub fn name<T1, T2, ..>(..)`. A variable the body uses only
                // structurally (pure pass-through) is unbounded — a bare `T{n}`;
                // a variable the body constrains to a super-type carries the
                // matching Rust trait bound (see [`Self::bounds_for`]). An empty
                // `free_vars` keeps the function monomorphic, byte-identical to a
                // non-generic binding.
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
                // Each quantified variable carries the Rust trait bound its
                // body-imposed super-type obligations require (empty for a
                // structurally-parametric variable — a bare `T{n}`).
                let var_bounds = self.types.bounds.get(&name);
                let type_params = free_vars
                    .iter()
                    .map(|v| (*v, Self::bounds_for(var_bounds, *v)))
                    .collect();
                Ok(Func {
                    id,
                    name,
                    type_params,
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

    /// The Rust trait bounds a quantified variable `var` carries, translating the
    /// type checker's super-type obligations ([`TyBounds`]) into the backend's
    /// [`BoundSet`]. A numeric obligation maps to the std arithmetic op trait it
    /// used (`Add` / `Sub` / `Mul`); an ordering obligation maps to `PartialOrd`;
    /// an equality obligation maps to `PartialEq`. A `Set`-element obligation maps
    /// to `Ord` (`BTreeSet`); a `Dict`-key obligation to `Hash + Ord + Clone`
    /// (`HashMap` + sorted key ops + key-duplicating merges).
    ///
    /// A `Number` / `Comparable` variable also gains `Copy`: those operations
    /// consume their operands by value (Rust's `Add` takes `self`), and a body
    /// that adds or orders a value reuses it, so the parameter must be
    /// bit-copyable. Equality is the exception — `PartialEq::eq` takes `&self`,
    /// so an *equality-only* variable borrows its operands and needs no `Copy`
    /// (which would also wrongly exclude `String`, a non-`Copy` but equatable
    /// type). A variable with no obligation (or a binding with no recorded
    /// bounds) is unbounded — a bare `T{n}`, byte-identical to a
    /// structurally-parametric generic.
    fn bounds_for(var_bounds: Option<&BTreeMap<Symbol, TyBounds>>, var: Symbol) -> BoundSet {
        let Some(b) = var_bounds.and_then(|m| m.get(&var)).copied() else {
            return BoundSet::UNBOUNDED;
        };
        if b.is_empty() {
            return BoundSet::UNBOUNDED;
        }
        let mut set = BoundSet::UNBOUNDED;
        if b.has_add() {
            set = set.with_add();
        }
        if b.has_sub() {
            set = set.with_sub();
        }
        if b.has_mul() {
            set = set.with_mul();
        }
        if b.has_ord() {
            set = set.with_ord();
        }
        if b.has_eq() {
            set = set.with_eq();
        }
        // A `Set` element needs Rust `Ord` (`BTreeSet<A>`); a `Dict` key needs
        // `Hash + Ord` (`HashMap<K, V>` + the determinism-sorted key ops) plus
        // `Clone` (`Dict.union` / `Dict.map` duplicate keys). `Eq` arrives as
        // `Ord`'s supertrait, so it is not emitted separately. Neither adds
        // `Copy`: the runtime kernels consume by value and a `String` key /
        // element must stay admissible.
        if b.has_set_elem() {
            set = set.with_ord_total();
        }
        if b.has_dict_key() {
            set = set.with_hash().with_ord_total().with_clone();
        }
        // Number / Comparable operations move their operand (`Add::add(self)`,
        // and the body reuses it), so the parameter must be `Copy`. Equality
        // borrows (`PartialEq::eq(&self)`), so an equality-only variable adds no
        // `Copy`.
        if b.has_number() || b.has_ord() {
            set = set.with_copy();
        }
        set
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
                "Char" => Ok(IrType::Char),
                // `Bytes` is a built-in distinct primitive (Vec<u8> on Rust;
                // distinct from String). Divergence from Sky: Sky aliases
                // Bytes = String; Sky-Rust makes Bytes a proper byte type.
                "Bytes" => Ok(IrType::Bytes),
                // The built-in `Maybe a` / `Result e a` map to dedicated IR
                // types, ahead of the user-enum lookup.
                "Maybe" if args.len() == 1 => {
                    let elem =
                        self.ir_type_from_canon(args.first().ok_or_else(maybe_arg_bug)?, generics)?;
                    Ok(IrType::Maybe(Box::new(elem)))
                }
                "Result" if args.len() == 2 => {
                    let err = self
                        .ir_type_from_canon(args.first().ok_or_else(result_arg_bug)?, generics)?;
                    let ok =
                        self.ir_type_from_canon(args.get(1).ok_or_else(result_arg_bug)?, generics)?;
                    Ok(IrType::Result(Box::new(err), Box::new(ok)))
                }
                "List" if args.len() == 1 => {
                    let elem =
                        self.ir_type_from_canon(args.first().ok_or_else(list_arg_bug)?, generics)?;
                    Ok(IrType::List(Box::new(elem)))
                }
                "Dict" if args.len() == 2 => {
                    let k =
                        self.ir_type_from_canon(args.first().ok_or_else(dict_arg_bug)?, generics)?;
                    let v =
                        self.ir_type_from_canon(args.get(1).ok_or_else(dict_arg_bug)?, generics)?;
                    Ok(IrType::Dict(Box::new(k), Box::new(v)))
                }
                "Set" if args.len() == 1 => {
                    let elem =
                        self.ir_type_from_canon(args.first().ok_or_else(set_arg_bug)?, generics)?;
                    Ok(IrType::Set(Box::new(elem)))
                }
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
    /// Lower a list literal `[]` / `[a, b, c]`. The element [`IrType`] comes from
    /// the expression's solved region type (`List elem`), so the backend can
    /// render an empty list as a typed `Vec::<T>::new()`; the items lower
    /// element-wise.
    fn lower_list(&self, elems: &[canon::Expr], span: Span) -> DResult<Expr> {
        let elem = self.list_elem_ir(span)?;
        let items = elems
            .iter()
            .map(|e| self.lower_expr(e))
            .collect::<DResult<Vec<_>>>()?;
        Ok(Expr::List { elem, items })
    }

    /// The element [`IrType`] of a list expression at `span`, read from its
    /// solved region type (`List elem`). A missing region or a non-list type is
    /// an internal invariant violation (the constraint generator pins every list
    /// expression to a `List` type), surfaced as a [`bug`] rather than guessed.
    fn list_elem_ir(&self, span: Span) -> DResult<IrType> {
        let ty = self.types.regions.get(&span).ok_or_else(|| {
            bug(
                "sky_lower::list_elem_ir",
                "no inferred type for a list literal",
            )
        })?;
        match ty {
            Ty::Con { name, args, .. } if self.resolve(*name)? == "List" && args.len() == 1 => {
                // Use the JSON-aware path: a `Value = any = Ty::Var` element
                // type (e.g. `List (String, Value)` passed to `JsonEnc.object`)
                // maps to `IrType::Json` rather than failing with Polymorphism.
                self.ir_type_from_ty_json(args.first().ok_or_else(list_arg_bug)?, span)
            }
            _ => Err(bug(
                "sky_lower::list_elem_ir",
                "list literal's region type is not a `List`",
            )),
        }
    }

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
                "Char" => Ok(IrType::Char),
                // `Bytes` is a built-in distinct primitive (Vec<u8> on Rust).
                // Divergence from Sky: Sky aliases Bytes = String.
                "Bytes" => Ok(IrType::Bytes),
                "Task" if args.len() == 1 && matches!(args.first(), Some(Ty::Unit)) => {
                    Ok(IrType::TaskUnit)
                }
                // A `Task` carrying a non-unit result (`Task Int`); M0 models
                // only `Task ()`. [SKY-L0104, feature: task-results]
                "Task" => Err(unsupported(span, Feature::TaskResults)),
                // The built-in `Maybe a` / `Result e a` map to dedicated IR
                // types (the runtime's `SkyMaybe` / `SkyResult`); they are not
                // user `type` declarations, so they precede the enum lookup.
                "Maybe" if args.len() == 1 => {
                    let elem =
                        self.ir_type_from_ty(args.first().ok_or_else(maybe_arg_bug)?, span)?;
                    Ok(IrType::Maybe(Box::new(elem)))
                }
                "Result" if args.len() == 2 => {
                    let err =
                        self.ir_type_from_ty(args.first().ok_or_else(result_arg_bug)?, span)?;
                    let ok = self.ir_type_from_ty(args.get(1).ok_or_else(result_arg_bug)?, span)?;
                    Ok(IrType::Result(Box::new(err), Box::new(ok)))
                }
                "List" if args.len() == 1 => {
                    let elem =
                        self.ir_type_from_ty(args.first().ok_or_else(list_arg_bug)?, span)?;
                    Ok(IrType::List(Box::new(elem)))
                }
                "Dict" if args.len() == 2 => {
                    let k = self.ir_type_from_ty(args.first().ok_or_else(dict_arg_bug)?, span)?;
                    let v = self.ir_type_from_ty(args.get(1).ok_or_else(dict_arg_bug)?, span)?;
                    // `Dict Float v` type-checks (Sky `Float` IS `comparable`),
                    // but the Rust backing `HashMap<f64, V>` cannot exist: `f64`
                    // is neither `Hash` nor `Eq` (NaN breaks both). Fail closed
                    // here with a dedicated diagnostic rather than emit Rust
                    // `cargo` rejects. Divergence from Sky, rationale: Rust
                    // backend capability (`f64` is not a hashable total order).
                    if matches!(k, IrType::Float) {
                        return Err(unsupported(span, Feature::FloatKeyedCollection));
                    }
                    Ok(IrType::Dict(Box::new(k), Box::new(v)))
                }
                "Set" if args.len() == 1 => {
                    let elem = self.ir_type_from_ty(args.first().ok_or_else(set_arg_bug)?, span)?;
                    // `Set Float` type-checks but its Rust backing
                    // `BTreeSet<f64>` cannot exist: `f64` is not `Ord` (NaN has
                    // no total order). Fail closed with the same dedicated
                    // diagnostic as `Dict Float`. Divergence from Sky, rationale:
                    // Rust backend capability.
                    if matches!(elem, IrType::Float) {
                        return Err(unsupported(span, Feature::FloatKeyedCollection));
                    }
                    Ok(IrType::Set(Box::new(elem)))
                }
                // `Decoder a` — the opaque JSON decoder type introduced by M4h.
                // Maps to `sky_runtime::json::Decoder<SkyError, T>`, aliased as
                // `Decoder<T>` in the emitted project's preamble.
                "Decoder" if args.len() == 1 => {
                    let inner = self.ir_type_from_ty(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Decoder applied without its element type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Decoder(Box::new(inner)))
                }
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

    /// Like [`ir_type_from_ty`] but treats an unresolved `Ty::Var` as
    /// [`IrType::Json`] instead of failing with `Feature::Polymorphism`.
    ///
    /// Used for JSON-kernel argument / return / list-element positions where
    /// `Value = any` legitimately leaves a bare type variable after HM solving.
    /// All other type forms delegate to the strict [`ir_type_from_ty`].
    fn ir_type_from_ty_json(&self, t: &Ty, span: Span) -> DResult<IrType> {
        match t {
            // The key difference: `Ty::Var` in a JSON context is `JsonVal`.
            Ty::Var(_) => Ok(IrType::Json),
            // Recursively handle compound types so embedded `Ty::Var`s also
            // map to `IrType::Json`.
            Ty::Tuple(elems) => {
                let lowered = elems
                    .iter()
                    .map(|e| self.ir_type_from_ty_json(e, span))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(IrType::Tuple(lowered))
            }
            Ty::Fun(_, _) => {
                let mut params = Vec::new();
                let mut cur = t;
                while let Ty::Fun(arg, rest) = cur {
                    params.push(self.ir_type_from_ty_json(arg, span)?);
                    cur = rest.as_ref();
                }
                let ret = self.ir_type_from_ty_json(cur, span)?;
                Ok(IrType::Fun(params, Box::new(ret)))
            }
            // For all other type forms, delegate to the strict helper.
            _ => self.ir_type_from_ty(t, span),
        }
    }

    /// Returns the exact [`IrType::Fun`] for kernels that may appear as
    /// first-class values and whose region type cannot be recovered from the Sky
    /// HM region map alone — most commonly because the return type is
    /// `Value = any = Ty::Var`, which [`Self::ir_type_from_ty_json`] maps to the
    /// opaque `IrType::Json` scalar (not `IrType::Fun`).
    ///
    /// The lookup is *only* consulted as a fallback inside the `VarKernel`
    /// value-reference path when the region type does not produce a
    /// `Fun` IR type.  Kernels handled by the arity-0 early-return (`JsonEncNull`)
    /// and the generic-`A` kernel (`JsonEncList`, which is never used as a bare
    /// value) are intentionally omitted.
    fn kernel_native_ir_type(k: KernelFn) -> Option<IrType> {
        Some(match k {
            KernelFn::JsonEncString => IrType::Fun(vec![IrType::Str], Box::new(IrType::Json)),
            KernelFn::JsonEncInt => IrType::Fun(vec![IrType::Int], Box::new(IrType::Json)),
            KernelFn::JsonEncFloat => IrType::Fun(vec![IrType::Float], Box::new(IrType::Json)),
            KernelFn::JsonEncBool => IrType::Fun(vec![IrType::Bool], Box::new(IrType::Json)),
            KernelFn::JsonEncObject => IrType::Fun(
                vec![IrType::List(Box::new(IrType::Tuple(vec![
                    IrType::Str,
                    IrType::Json,
                ])))],
                Box::new(IrType::Json),
            ),
            KernelFn::JsonEncEncode => {
                IrType::Fun(vec![IrType::Int, IrType::Json], Box::new(IrType::Str))
            }
            _ => return None,
        })
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

    // `lower_expr` is a large dispatch function that covers every canon AST
    // variant in one place for readability; split would add indirection without
    // clarity.
    #[allow(clippy::too_many_lines)]
    fn lower_expr(&self, e: &canon::Expr) -> DResult<Expr> {
        self.reject_function_through_type_var(e)?;
        match &e.value {
            canon::Expr_::Int(n) => Ok(Expr::Int(*n)),
            canon::Expr_::Float(f) => Ok(Expr::Float(*f)),
            canon::Expr_::Str(s) => Ok(Expr::Str(s.clone())),
            canon::Expr_::Char(c) => Ok(Expr::Char(c.clone())),
            canon::Expr_::Unit => Ok(Expr::Unit),
            canon::Expr_::VarLocal(s) => Ok(Expr::Var(*s)),
            canon::Expr_::VarCtor {
                type_name, name, ..
            } => {
                // `True` / `False` are the Prelude-exposed nullary constructors of
                // the built-in `Bool`; they lower to the IR boolean literal
                // (rendered as Rust `true` / `false`), not an enum construction.
                match self.resolve(*name)? {
                    "True" => return Ok(Expr::Bool(true)),
                    "False" => return Ok(Expr::Bool(false)),
                    _ => {}
                }
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
            canon::Expr_::List(elems) => self.lower_list(elems, e.span),
            canon::Expr_::Cons(head, tail) => Ok(Expr::Cons {
                head: Box::new(self.lower_expr(head)?),
                tail: Box::new(self.lower_expr(tail)?),
            }),
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
                // Arity-0 kernels (nullary constants such as `JsonEnc.null`)
                // are zero-argument calls regardless of the solved return type.
                // Bypassing `ir_type_from_ty` avoids a `Polymorphism` error
                // when the return type is `Value = any = Ty::Var`.  Rust
                // infers the concrete return type from the Rust function's
                // own declared signature.
                if matches!(&callee, Callee::Kernel(_)) && self.callee_arity(&callee)? == 0 {
                    return Ok(Expr::Call {
                        callee,
                        args: Vec::new(),
                    });
                }
                let ty = self.types.regions.get(&e.span).ok_or_else(|| {
                    bug(
                        "sky_lower::lower_expr",
                        "no inferred type for a function/value reference",
                    )
                })?;
                // For kernel callees use the JSON-aware type resolver so that
                // a `Value = any = Ty::Var` in the argument / return position
                // of a JSON kernel (e.g. `JsonEnc.string : String -> Value`)
                // maps to `IrType::Json` rather than failing with Polymorphism.
                // User top-level bindings keep the strict resolver.
                let ty_ir = if matches!(&callee, Callee::Kernel(_)) {
                    self.ir_type_from_ty_json(ty, e.span)?
                } else {
                    self.ir_type_from_ty(ty, e.span)?
                };
                if let fun @ IrType::Fun(_, _) = ty_ir {
                    Ok(Expr::FuncValue { callee, ty: fun })
                } else {
                    // When a kernel with arity > 0 has an unresolved region
                    // type (e.g. `Value = any = Ty::Var` → `IrType::Json`),
                    // the kernel is being used as a first-class function
                    // value.  Fall back to the kernel's known native
                    // signature so the backend emits a properly typed
                    // `FuncValue` (`Box::new(name)`) instead of a spurious
                    // zero-argument call (`name()`).
                    if let Callee::Kernel(k) = &callee {
                        let arity = self.callee_arity(&callee)?;
                        if arity > 0
                            && let Some(fun_ty) = Self::kernel_native_ir_type(*k)
                        {
                            return Ok(Expr::FuncValue { callee, ty: fun_ty });
                        }
                    }
                    // A nullary top-level constant or zero-arg kernel
                    // referenced as a value is its own zero-argument call
                    // (`x` → `x()`).
                    Ok(Expr::Call {
                        callee,
                        args: Vec::new(),
                    })
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
    /// Soundness gate (inference path): reject a Set/Dict-producing expression
    /// whose solved region type pins the element / key to `Float`.
    ///
    /// The shape gate in [`Self::ir_type_from_ty`] catches a `Set Float` /
    /// `Dict Float v` only when an annotation or binding type drives a
    /// conversion to IR. A Set / Dict synthesised purely by inference —
    /// `Set.fromList [1.5, 2.5]`, a `let`-bound `Set.fromList`, or a Set built
    /// from a `List.map` result — never drives that conversion, so its own
    /// region type is the only place the `Float` element / key surfaces. `f64`
    /// is neither `Ord` nor `Hash` / `Eq` (NaN has no total order), so the Rust
    /// backing `BTreeSet<f64>` / `HashMap<f64, _>` cannot exist. Fail closed
    /// with the same dedicated diagnostic. Divergence from Sky, rationale: Rust
    /// backend capability.
    ///
    /// A bare-variable element / key (`Set.empty`, an unpinned polymorphic Set)
    /// is left untouched: it carries no concrete `Float`, so it is sound to lower
    /// (and forcing it through [`Self::ir_type_from_ty`] would mis-report it as
    /// the polymorphism gap rather than this capability gap).
    fn reject_float_keyed_collection(&self, span: Span) -> DResult<()> {
        let Some(Ty::Con { name, args, .. }) = self.types.regions.get(&span) else {
            return Ok(());
        };
        let key = match (self.resolve(*name)?, args.as_slice()) {
            ("Set", [elem]) => elem,
            ("Dict", [k, _]) => k,
            _ => return Ok(()),
        };
        if self.is_concrete_float(key)? {
            return Err(unsupported(span, Feature::FloatKeyedCollection));
        }
        Ok(())
    }

    /// Whether a solved type is the concrete builtin `Float` (a nullary `Ty::Con`
    /// resolving to `"Float"`). A bare `Ty::Var` is deliberately NOT a float —
    /// an unpinned polymorphic element is sound to lower.
    fn is_concrete_float(&self, t: &Ty) -> DResult<bool> {
        Ok(
            matches!(t, Ty::Con { name, args, .. } if args.is_empty() && self.resolve(*name)? == "Float"),
        )
    }

    fn lower_call(
        &self,
        callee: &canon::Expr,
        args: &[canon::Expr],
        call_span: Span,
    ) -> DResult<Expr> {
        // A Set / Dict produced by inference (no annotation driving an
        // `ir_type_from_ty` conversion) is gated here on its own region type.
        self.reject_float_keyed_collection(call_span)?;
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
                    // `Ok x` whose `Result e a` error type `e` is still
                    // unconstrained after solving would emit an ambiguous
                    // `SkyResult<_, _>` that rustc rejects (E0282). Route it to
                    // the runtime's `ok_res`, which pins the error type to the
                    // project's `SkyError`. Sound: the `Err` arm is unreachable
                    // for an `Ok`, so any error type yields identical behaviour;
                    // `SkyError` is the canonical default. A constrained `e`
                    // (e.g. an annotated `Result String Int`) keeps the direct
                    // `SkyResult::Ok` form, byte-identical to before.
                    if arity == 1
                        && self.resolve(*name)? == "Ok"
                        && self.result_error_unresolved(call_span)
                    {
                        return Ok(Expr::Call {
                            callee: Callee::Kernel(KernelFn::ResultOkDefault),
                            args: lowered_args,
                        });
                    }
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
    #[allow(clippy::too_many_lines)] // declarative kernel-arity table — each variant listed explicitly for safety
    fn callee_arity(&self, callee: &Callee) -> DResult<usize> {
        match callee {
            // Arity is fixed per kernel. Each variant is listed explicitly so a
            // new entry can never silently inherit a wrong count.
            // ── Math constants / Dict.empty / Set.empty — arity 0 ───────────
            Callee::Kernel(
                KernelFn::MathPi
                | KernelFn::MathE
                | KernelFn::MathPhi
                | KernelFn::MathSqrt2
                | KernelFn::MathInf
                | KernelFn::MathNan
                | KernelFn::DictEmpty
                | KernelFn::SetEmpty
                // ── Bytes arity-0 ────────────────────────────────────────────
                | KernelFn::BytesEmpty
                // ── JsonEnc arity-0 (M4g) ────────────────────────────────────
                | KernelFn::JsonEncNull
                // ── JsonDec primitive decoders — arity 0 (M4h) ────────────────
                | KernelFn::JsonDecString
                | KernelFn::JsonDecInt
                | KernelFn::JsonDecFloat
                | KernelFn::JsonDecBool,
            ) => Ok(0),
            Callee::Kernel(
                KernelFn::StringFromInt
                | KernelFn::StringFromFloat
                | KernelFn::StringLength
                | KernelFn::StringIsEmpty
                | KernelFn::StringReverse
                | KernelFn::StringToUpper
                | KernelFn::StringToLower
                | KernelFn::StringCasefold
                | KernelFn::StringTrim
                | KernelFn::StringTrimStart
                | KernelFn::StringTrimEnd
                | KernelFn::StringToInt
                | KernelFn::StringToFloat
                | KernelFn::StringFromChar
                | KernelFn::StringFromList
                | KernelFn::StringConcat
                | KernelFn::StringWords
                | KernelFn::StringLines
                | KernelFn::StringToList
                | KernelFn::StringIsEmail
                | KernelFn::StringIsUrl
                | KernelFn::CharIsAlpha
                | KernelFn::CharIsDigit
                | KernelFn::CharIsLower
                | KernelFn::CharIsUpper
                | KernelFn::CharToLower
                | KernelFn::CharToUpper
                | KernelFn::CharToCode
                | KernelFn::CharFromCode
                | KernelFn::LogPrintln
                | KernelFn::ListLength
                | KernelFn::ListHead
                | KernelFn::ListTail
                | KernelFn::ListReverse
                | KernelFn::ResultOkDefault
                // ── Dict arity-1 ─────────────────────────────────────────────
                | KernelFn::DictIsEmpty
                | KernelFn::DictSize
                | KernelFn::DictKeys
                | KernelFn::DictValues
                | KernelFn::DictToList
                | KernelFn::DictFromList
                // ── Set arity-1 ──────────────────────────────────────────────
                | KernelFn::SetSize
                | KernelFn::SetToList
                | KernelFn::SetFromList
                // ── Bytes arity-1 ────────────────────────────────────────────
                | KernelFn::BytesLength
                | KernelFn::BytesIsEmpty
                | KernelFn::BytesFromString
                | KernelFn::BytesToString
                | KernelFn::BytesFromHex
                | KernelFn::BytesToHex
                | KernelFn::BytesFromBase64
                | KernelFn::BytesToBase64
                // ── Encoding arity-1 (M4f) ────────────────────────────────────
                | KernelFn::EncodingBase64Encode
                | KernelFn::EncodingBase64Decode
                | KernelFn::EncodingUrlEncode
                | KernelFn::EncodingUrlDecode
                | KernelFn::EncodingHexEncode
                | KernelFn::EncodingHexDecode
                // ── JsonEnc arity-1 (M4g) ─────────────────────────────────────
                | KernelFn::JsonEncString
                | KernelFn::JsonEncInt
                | KernelFn::JsonEncFloat
                | KernelFn::JsonEncBool
                | KernelFn::JsonEncObject
                // ── JsonDec arity-1 combinators (M4h) ─────────────────────────
                | KernelFn::JsonDecList
                | KernelFn::JsonDecSucceed
                | KernelFn::JsonDecFail
                | KernelFn::JsonDecOneOf
                // ── Math arity-1 (Int → Int) ─────────────────────────────────
                | KernelFn::MathAbs
                // ── Math arity-1 (Float → Float) ────────────────────────────
                | KernelFn::MathSqrt
                | KernelFn::MathCbrt
                | KernelFn::MathExp
                | KernelFn::MathExp2
                | KernelFn::MathLog
                | KernelFn::MathLog2
                | KernelFn::MathLog10
                | KernelFn::MathSin
                | KernelFn::MathCos
                | KernelFn::MathTan
                | KernelFn::MathAsin
                | KernelFn::MathAcos
                | KernelFn::MathAtan
                | KernelFn::MathSinh
                | KernelFn::MathCosh
                | KernelFn::MathTanh
                | KernelFn::MathAsinh
                | KernelFn::MathAcosh
                | KernelFn::MathAtanh
                // ── Math arity-1 (Float → Int) ───────────────────────────────
                | KernelFn::MathFloor
                | KernelFn::MathCeil
                | KernelFn::MathRound
                | KernelFn::MathTrunc
                // ── Crypto arity-1 (M5a) ─────────────────────────────────────
                | KernelFn::CryptoSha256
                | KernelFn::CryptoSha512
                | KernelFn::CryptoSha1
                | KernelFn::CryptoMd5
                | KernelFn::CryptoRandomBytes
                | KernelFn::CryptoRandomToken,
            ) => Ok(1),
            Callee::Kernel(
                KernelFn::StringAppend
                | KernelFn::StringContains
                | KernelFn::StringStartsWith
                | KernelFn::StringEndsWith
                | KernelFn::StringEqualFold
                | KernelFn::StringJoin
                | KernelFn::StringSplit
                | KernelFn::StringRepeat
                | KernelFn::StringDropLeft
                | KernelFn::StringDropRight
                | KernelFn::ListMap
                | KernelFn::ListFilter
                | KernelFn::ListMember
                | KernelFn::ListRange
                | KernelFn::MaybeWithDefault
                | KernelFn::MaybeMap
                | KernelFn::MaybeAndThen
                | KernelFn::ResultWithDefault
                | KernelFn::ResultMap
                | KernelFn::MathMin
                | KernelFn::MathMax
                // ── Dict arity-2 ─────────────────────────────────────────────
                | KernelFn::DictGet
                | KernelFn::DictMember
                | KernelFn::DictRemove
                | KernelFn::DictUnion
                | KernelFn::DictMap
                // ── Set arity-2 ──────────────────────────────────────────────
                | KernelFn::SetMember
                | KernelFn::SetInsert
                | KernelFn::SetRemove
                | KernelFn::SetUnion
                | KernelFn::SetIntersect
                | KernelFn::SetDiff
                // ── Bytes arity-2 ────────────────────────────────────────────
                | KernelFn::BytesAppend
                // ── JsonEnc arity-2 (M4g) ─────────────────────────────────────
                | KernelFn::JsonEncList
                | KernelFn::JsonEncEncode
                // ── JsonDec arity-2 (M4h) ─────────────────────────────────────
                | KernelFn::JsonDecDecodeString
                | KernelFn::JsonDecField
                | KernelFn::JsonDecAt
                | KernelFn::JsonDecIndex
                | KernelFn::JsonDecMap
                | KernelFn::JsonDecAndThen
                | KernelFn::JsonDecPCustom
                // ── Math arity-2 (Float → Float → Float) ────────────────────
                | KernelFn::MathPow
                | KernelFn::MathHypot
                | KernelFn::MathAtan2
                | KernelFn::MathMod
                | KernelFn::MathRemainder
                // ── Crypto arity-2 (M5a) ─────────────────────────────────────
                | KernelFn::CryptoHmacSha256
                | KernelFn::CryptoHmacSha512
                | KernelFn::CryptoRsaSha256Sign
                | KernelFn::CryptoConstantTimeEqual
                | KernelFn::CryptoAesGcmEncrypt
                | KernelFn::CryptoAesGcmDecrypt
                | KernelFn::CryptoChacha20Encrypt
                | KernelFn::CryptoChacha20Decrypt
                | KernelFn::CryptoAesKeyFromPassword
                | KernelFn::CryptoChachaKeyFromPassword,
            ) => Ok(2),
            Callee::Kernel(
                KernelFn::StringReplace
                | KernelFn::StringSlice
                | KernelFn::StringPadLeft
                | KernelFn::StringPadRight
                | KernelFn::ListFoldl
                | KernelFn::ListFoldr
                // ── Dict arity-3 ─────────────────────────────────────────────
                | KernelFn::DictInsert
                | KernelFn::DictFoldl
                // ── Bytes arity-3 ────────────────────────────────────────────
                | KernelFn::BytesSlice
                // ── JsonDec arity-3 (M4h) ─────────────────────────────────────
                | KernelFn::JsonDecMap2
                | KernelFn::JsonDecPRequired
                | KernelFn::JsonDecPRequiredAt
                // ── Crypto arity-3 (M5a) ─────────────────────────────────────
                | KernelFn::CryptoRsaSha256Verify,
            ) => Ok(3),
            // ── JsonDec arity-4 (M4h) ─────────────────────────────────────────
            Callee::Kernel(KernelFn::JsonDecMap3 | KernelFn::JsonDecPOptional) => Ok(4),
            // ── JsonDec arity-5 (M4h) ─────────────────────────────────────────
            Callee::Kernel(KernelFn::JsonDecMap4) => Ok(5),
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

    /// Whether the `Result e a` value produced at `span` still has an
    /// unconstrained error type `e` after solving. True only when the solved
    /// region type is a `Result` constructor whose first argument (the error
    /// type) is an unresolved [`Ty::Var`] — the case the backend cannot emit as a
    /// bare `SkyResult::Ok` without tripping rustc's E0282 ambiguity. A missing
    /// region type or a concrete error type yields `false`.
    fn result_error_unresolved(&self, span: Span) -> bool {
        match self.types.regions.get(&span) {
            Some(Ty::Con { name, args, .. }) => {
                self.resolve(*name).map(|n| n == "Result").unwrap_or(false)
                    && matches!(args.first(), Some(Ty::Var(_)))
            }
            _ => false,
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

    #[allow(clippy::too_many_lines)] // declarative kernel-name dispatch table
    fn lower_callee(&self, callee: &canon::Expr) -> DResult<Callee> {
        match &callee.value {
            canon::Expr_::VarKernel { module, name } => {
                match (self.resolve(*module)?, self.resolve(*name)?) {
                    ("Log", "println") => Ok(Callee::Kernel(KernelFn::LogPrintln)),
                    // ── String kernels ─────────────────────────────────────
                    ("String", "fromInt") => Ok(Callee::Kernel(KernelFn::StringFromInt)),
                    ("String", "fromFloat") => Ok(Callee::Kernel(KernelFn::StringFromFloat)),
                    ("String", "length") => Ok(Callee::Kernel(KernelFn::StringLength)),
                    ("String", "isEmpty") => Ok(Callee::Kernel(KernelFn::StringIsEmpty)),
                    ("String", "reverse") => Ok(Callee::Kernel(KernelFn::StringReverse)),
                    ("String", "toUpper") => Ok(Callee::Kernel(KernelFn::StringToUpper)),
                    ("String", "toLower") => Ok(Callee::Kernel(KernelFn::StringToLower)),
                    ("String", "casefold") => Ok(Callee::Kernel(KernelFn::StringCasefold)),
                    ("String", "trim") => Ok(Callee::Kernel(KernelFn::StringTrim)),
                    ("String", "trimStart") => Ok(Callee::Kernel(KernelFn::StringTrimStart)),
                    ("String", "trimEnd") => Ok(Callee::Kernel(KernelFn::StringTrimEnd)),
                    ("String", "toInt") => Ok(Callee::Kernel(KernelFn::StringToInt)),
                    ("String", "toFloat") => Ok(Callee::Kernel(KernelFn::StringToFloat)),
                    ("String", "fromChar") => Ok(Callee::Kernel(KernelFn::StringFromChar)),
                    ("String", "fromList") => Ok(Callee::Kernel(KernelFn::StringFromList)),
                    ("String", "concat") => Ok(Callee::Kernel(KernelFn::StringConcat)),
                    ("String", "words") => Ok(Callee::Kernel(KernelFn::StringWords)),
                    ("String", "lines") => Ok(Callee::Kernel(KernelFn::StringLines)),
                    ("String", "toList") => Ok(Callee::Kernel(KernelFn::StringToList)),
                    ("String", "isEmail") => Ok(Callee::Kernel(KernelFn::StringIsEmail)),
                    ("String", "isUrl") => Ok(Callee::Kernel(KernelFn::StringIsUrl)),
                    ("String", "append") => Ok(Callee::Kernel(KernelFn::StringAppend)),
                    ("String", "contains") => Ok(Callee::Kernel(KernelFn::StringContains)),
                    ("String", "startsWith") => Ok(Callee::Kernel(KernelFn::StringStartsWith)),
                    ("String", "endsWith") => Ok(Callee::Kernel(KernelFn::StringEndsWith)),
                    ("String", "equalFold") => Ok(Callee::Kernel(KernelFn::StringEqualFold)),
                    ("String", "join") => Ok(Callee::Kernel(KernelFn::StringJoin)),
                    ("String", "split") => Ok(Callee::Kernel(KernelFn::StringSplit)),
                    ("String", "repeat") => Ok(Callee::Kernel(KernelFn::StringRepeat)),
                    ("String", "dropLeft") => Ok(Callee::Kernel(KernelFn::StringDropLeft)),
                    ("String", "dropRight") => Ok(Callee::Kernel(KernelFn::StringDropRight)),
                    ("String", "replace") => Ok(Callee::Kernel(KernelFn::StringReplace)),
                    ("String", "slice") => Ok(Callee::Kernel(KernelFn::StringSlice)),
                    ("String", "padLeft") => Ok(Callee::Kernel(KernelFn::StringPadLeft)),
                    ("String", "padRight") => Ok(Callee::Kernel(KernelFn::StringPadRight)),
                    // ── Char kernels ───────────────────────────────────────
                    ("Char", "isAlpha") => Ok(Callee::Kernel(KernelFn::CharIsAlpha)),
                    ("Char", "isDigit") => Ok(Callee::Kernel(KernelFn::CharIsDigit)),
                    ("Char", "isLower") => Ok(Callee::Kernel(KernelFn::CharIsLower)),
                    ("Char", "isUpper") => Ok(Callee::Kernel(KernelFn::CharIsUpper)),
                    ("Char", "toLower") => Ok(Callee::Kernel(KernelFn::CharToLower)),
                    ("Char", "toUpper") => Ok(Callee::Kernel(KernelFn::CharToUpper)),
                    ("Char", "toCode") => Ok(Callee::Kernel(KernelFn::CharToCode)),
                    ("Char", "fromCode") => Ok(Callee::Kernel(KernelFn::CharFromCode)),
                    // ── List kernels ───────────────────────────────────────
                    ("List", "map") => Ok(Callee::Kernel(KernelFn::ListMap)),
                    ("List", "filter") => Ok(Callee::Kernel(KernelFn::ListFilter)),
                    ("List", "foldl") => Ok(Callee::Kernel(KernelFn::ListFoldl)),
                    ("List", "foldr") => Ok(Callee::Kernel(KernelFn::ListFoldr)),
                    ("List", "length") => Ok(Callee::Kernel(KernelFn::ListLength)),
                    ("List", "head") => Ok(Callee::Kernel(KernelFn::ListHead)),
                    ("List", "tail") => Ok(Callee::Kernel(KernelFn::ListTail)),
                    ("List", "member") => Ok(Callee::Kernel(KernelFn::ListMember)),
                    ("List", "range") => Ok(Callee::Kernel(KernelFn::ListRange)),
                    ("List", "reverse") => Ok(Callee::Kernel(KernelFn::ListReverse)),
                    // ── Maybe kernels ──────────────────────────────────────
                    ("Maybe", "withDefault") => Ok(Callee::Kernel(KernelFn::MaybeWithDefault)),
                    ("Maybe", "map") => Ok(Callee::Kernel(KernelFn::MaybeMap)),
                    ("Maybe", "andThen") => Ok(Callee::Kernel(KernelFn::MaybeAndThen)),
                    // ── Result kernels ─────────────────────────────────────
                    ("Result", "withDefault") => Ok(Callee::Kernel(KernelFn::ResultWithDefault)),
                    ("Result", "map") => Ok(Callee::Kernel(KernelFn::ResultMap)),
                    // ── Math kernels ───────────────────────────────────────
                    // `min` / `max` are polymorphic `a -> a -> a` — lowered to
                    // the runtime's generic compare, NOT through any `Int`
                    // coercion. Divergence from Sky (PR #136): Sky routes args
                    // through AsInt; Sky-Rust follows Elm's polymorphic
                    // comparable. Rationale: Elm-conformance. The args keep
                    // their solved type, so `math_min`/`math_max` infer `T` and
                    // preserve the argument's value + type unchanged.
                    ("Math", "min") => Ok(Callee::Kernel(KernelFn::MathMin)),
                    ("Math", "max") => Ok(Callee::Kernel(KernelFn::MathMax)),
                    // ── Math constants (arity 0) ─────────────────────────────
                    ("Math", "pi") => Ok(Callee::Kernel(KernelFn::MathPi)),
                    ("Math", "e") => Ok(Callee::Kernel(KernelFn::MathE)),
                    ("Math", "phi") => Ok(Callee::Kernel(KernelFn::MathPhi)),
                    ("Math", "sqrt2") => Ok(Callee::Kernel(KernelFn::MathSqrt2)),
                    ("Math", "inf") => Ok(Callee::Kernel(KernelFn::MathInf)),
                    ("Math", "nan") => Ok(Callee::Kernel(KernelFn::MathNan)),
                    // ── Math arity-1 (Int → Int) ─────────────────────────────
                    ("Math", "abs") => Ok(Callee::Kernel(KernelFn::MathAbs)),
                    // ── Math arity-1 (Float → Float) ────────────────────────
                    ("Math", "sqrt") => Ok(Callee::Kernel(KernelFn::MathSqrt)),
                    ("Math", "cbrt") => Ok(Callee::Kernel(KernelFn::MathCbrt)),
                    ("Math", "exp") => Ok(Callee::Kernel(KernelFn::MathExp)),
                    ("Math", "exp2") => Ok(Callee::Kernel(KernelFn::MathExp2)),
                    ("Math", "log") => Ok(Callee::Kernel(KernelFn::MathLog)),
                    ("Math", "log2") => Ok(Callee::Kernel(KernelFn::MathLog2)),
                    ("Math", "log10") => Ok(Callee::Kernel(KernelFn::MathLog10)),
                    ("Math", "sin") => Ok(Callee::Kernel(KernelFn::MathSin)),
                    ("Math", "cos") => Ok(Callee::Kernel(KernelFn::MathCos)),
                    ("Math", "tan") => Ok(Callee::Kernel(KernelFn::MathTan)),
                    ("Math", "asin") => Ok(Callee::Kernel(KernelFn::MathAsin)),
                    ("Math", "acos") => Ok(Callee::Kernel(KernelFn::MathAcos)),
                    ("Math", "atan") => Ok(Callee::Kernel(KernelFn::MathAtan)),
                    ("Math", "sinh") => Ok(Callee::Kernel(KernelFn::MathSinh)),
                    ("Math", "cosh") => Ok(Callee::Kernel(KernelFn::MathCosh)),
                    ("Math", "tanh") => Ok(Callee::Kernel(KernelFn::MathTanh)),
                    ("Math", "asinh") => Ok(Callee::Kernel(KernelFn::MathAsinh)),
                    ("Math", "acosh") => Ok(Callee::Kernel(KernelFn::MathAcosh)),
                    ("Math", "atanh") => Ok(Callee::Kernel(KernelFn::MathAtanh)),
                    // ── Math arity-1 (Float → Int) ───────────────────────────
                    ("Math", "floor") => Ok(Callee::Kernel(KernelFn::MathFloor)),
                    ("Math", "ceil") => Ok(Callee::Kernel(KernelFn::MathCeil)),
                    ("Math", "round") => Ok(Callee::Kernel(KernelFn::MathRound)),
                    ("Math", "trunc") => Ok(Callee::Kernel(KernelFn::MathTrunc)),
                    // ── Math arity-2 (Float → Float → Float) ────────────────
                    ("Math", "pow") => Ok(Callee::Kernel(KernelFn::MathPow)),
                    ("Math", "hypot") => Ok(Callee::Kernel(KernelFn::MathHypot)),
                    ("Math", "atan2") => Ok(Callee::Kernel(KernelFn::MathAtan2)),
                    ("Math", "mod") => Ok(Callee::Kernel(KernelFn::MathMod)),
                    ("Math", "remainder") => Ok(Callee::Kernel(KernelFn::MathRemainder)),
                    // ── Dict kernels ───────────────────────────────────────
                    ("Dict", "empty") => Ok(Callee::Kernel(KernelFn::DictEmpty)),
                    ("Dict", "isEmpty") => Ok(Callee::Kernel(KernelFn::DictIsEmpty)),
                    ("Dict", "size") => Ok(Callee::Kernel(KernelFn::DictSize)),
                    ("Dict", "keys") => Ok(Callee::Kernel(KernelFn::DictKeys)),
                    ("Dict", "values") => Ok(Callee::Kernel(KernelFn::DictValues)),
                    ("Dict", "toList") => Ok(Callee::Kernel(KernelFn::DictToList)),
                    ("Dict", "fromList") => Ok(Callee::Kernel(KernelFn::DictFromList)),
                    ("Dict", "get") => Ok(Callee::Kernel(KernelFn::DictGet)),
                    ("Dict", "member") => Ok(Callee::Kernel(KernelFn::DictMember)),
                    ("Dict", "remove") => Ok(Callee::Kernel(KernelFn::DictRemove)),
                    ("Dict", "union") => Ok(Callee::Kernel(KernelFn::DictUnion)),
                    ("Dict", "map") => Ok(Callee::Kernel(KernelFn::DictMap)),
                    ("Dict", "insert") => Ok(Callee::Kernel(KernelFn::DictInsert)),
                    ("Dict", "foldl") => Ok(Callee::Kernel(KernelFn::DictFoldl)),
                    // ── Set kernels ────────────────────────────────────────
                    ("Set", "empty") => Ok(Callee::Kernel(KernelFn::SetEmpty)),
                    ("Set", "size") => Ok(Callee::Kernel(KernelFn::SetSize)),
                    ("Set", "toList") => Ok(Callee::Kernel(KernelFn::SetToList)),
                    ("Set", "fromList") => Ok(Callee::Kernel(KernelFn::SetFromList)),
                    ("Set", "member") => Ok(Callee::Kernel(KernelFn::SetMember)),
                    ("Set", "insert") => Ok(Callee::Kernel(KernelFn::SetInsert)),
                    ("Set", "remove") => Ok(Callee::Kernel(KernelFn::SetRemove)),
                    ("Set", "union") => Ok(Callee::Kernel(KernelFn::SetUnion)),
                    ("Set", "intersect") => Ok(Callee::Kernel(KernelFn::SetIntersect)),
                    ("Set", "diff") => Ok(Callee::Kernel(KernelFn::SetDiff)),
                    // ── Bytes kernels (M4e) ────────────────────────────────
                    // Divergence from Sky: Bytes is Vec<u8> not String alias.
                    ("Bytes", "empty") => Ok(Callee::Kernel(KernelFn::BytesEmpty)),
                    ("Bytes", "length") => Ok(Callee::Kernel(KernelFn::BytesLength)),
                    ("Bytes", "isEmpty") => Ok(Callee::Kernel(KernelFn::BytesIsEmpty)),
                    ("Bytes", "fromString") => Ok(Callee::Kernel(KernelFn::BytesFromString)),
                    ("Bytes", "toString") => Ok(Callee::Kernel(KernelFn::BytesToString)),
                    ("Bytes", "fromHex") => Ok(Callee::Kernel(KernelFn::BytesFromHex)),
                    ("Bytes", "toHex") => Ok(Callee::Kernel(KernelFn::BytesToHex)),
                    ("Bytes", "fromBase64") => Ok(Callee::Kernel(KernelFn::BytesFromBase64)),
                    ("Bytes", "toBase64") => Ok(Callee::Kernel(KernelFn::BytesToBase64)),
                    ("Bytes", "append") => Ok(Callee::Kernel(KernelFn::BytesAppend)),
                    ("Bytes", "slice") => Ok(Callee::Kernel(KernelFn::BytesSlice)),
                    // ── Encoding kernels (M4f) ─────────────────────────────
                    ("Encoding", "base64Encode") => {
                        Ok(Callee::Kernel(KernelFn::EncodingBase64Encode))
                    }
                    ("Encoding", "base64Decode") => {
                        Ok(Callee::Kernel(KernelFn::EncodingBase64Decode))
                    }
                    ("Encoding", "urlEncode") => Ok(Callee::Kernel(KernelFn::EncodingUrlEncode)),
                    ("Encoding", "urlDecode") => Ok(Callee::Kernel(KernelFn::EncodingUrlDecode)),
                    ("Encoding", "hexEncode") => Ok(Callee::Kernel(KernelFn::EncodingHexEncode)),
                    ("Encoding", "hexDecode") => Ok(Callee::Kernel(KernelFn::EncodingHexDecode)),
                    // ── JsonEnc kernels (M4g) ──────────────────────────────────
                    ("JsonEnc", "string") => Ok(Callee::Kernel(KernelFn::JsonEncString)),
                    ("JsonEnc", "int") => Ok(Callee::Kernel(KernelFn::JsonEncInt)),
                    ("JsonEnc", "float") => Ok(Callee::Kernel(KernelFn::JsonEncFloat)),
                    ("JsonEnc", "bool") => Ok(Callee::Kernel(KernelFn::JsonEncBool)),
                    ("JsonEnc", "null") => Ok(Callee::Kernel(KernelFn::JsonEncNull)),
                    ("JsonEnc", "list") => Ok(Callee::Kernel(KernelFn::JsonEncList)),
                    ("JsonEnc", "object") => Ok(Callee::Kernel(KernelFn::JsonEncObject)),
                    ("JsonEnc", "encode") => Ok(Callee::Kernel(KernelFn::JsonEncEncode)),
                    // ── Json.Decode (M4h) ─────────────────────────────────────
                    ("JsonDec", "string") => Ok(Callee::Kernel(KernelFn::JsonDecString)),
                    ("JsonDec", "int") => Ok(Callee::Kernel(KernelFn::JsonDecInt)),
                    ("JsonDec", "float") => Ok(Callee::Kernel(KernelFn::JsonDecFloat)),
                    ("JsonDec", "bool") => Ok(Callee::Kernel(KernelFn::JsonDecBool)),
                    ("JsonDec", "decodeString") => {
                        Ok(Callee::Kernel(KernelFn::JsonDecDecodeString))
                    }
                    ("JsonDec", "field") => Ok(Callee::Kernel(KernelFn::JsonDecField)),
                    ("JsonDec", "at") => Ok(Callee::Kernel(KernelFn::JsonDecAt)),
                    ("JsonDec", "index") => Ok(Callee::Kernel(KernelFn::JsonDecIndex)),
                    ("JsonDec", "list") => Ok(Callee::Kernel(KernelFn::JsonDecList)),
                    ("JsonDec", "map") => Ok(Callee::Kernel(KernelFn::JsonDecMap)),
                    ("JsonDec", "andThen") => Ok(Callee::Kernel(KernelFn::JsonDecAndThen)),
                    ("JsonDec", "succeed") => Ok(Callee::Kernel(KernelFn::JsonDecSucceed)),
                    ("JsonDec", "fail") => Ok(Callee::Kernel(KernelFn::JsonDecFail)),
                    ("JsonDec", "oneOf") => Ok(Callee::Kernel(KernelFn::JsonDecOneOf)),
                    ("JsonDec", "map2") => Ok(Callee::Kernel(KernelFn::JsonDecMap2)),
                    ("JsonDec", "map3") => Ok(Callee::Kernel(KernelFn::JsonDecMap3)),
                    ("JsonDec", "map4") => Ok(Callee::Kernel(KernelFn::JsonDecMap4)),
                    // ── Json.Decode.Pipeline (M4h) ────────────────────────────
                    ("JsonDecP", "required") => Ok(Callee::Kernel(KernelFn::JsonDecPRequired)),
                    ("JsonDecP", "optional") => Ok(Callee::Kernel(KernelFn::JsonDecPOptional)),
                    ("JsonDecP", "custom") => Ok(Callee::Kernel(KernelFn::JsonDecPCustom)),
                    ("JsonDecP", "requiredAt") => Ok(Callee::Kernel(KernelFn::JsonDecPRequiredAt)),
                    // ── Crypto kernels (M5a) ──────────────────────────────────
                    ("Crypto", "sha256") => Ok(Callee::Kernel(KernelFn::CryptoSha256)),
                    ("Crypto", "sha512") => Ok(Callee::Kernel(KernelFn::CryptoSha512)),
                    ("Crypto", "sha1") => Ok(Callee::Kernel(KernelFn::CryptoSha1)),
                    ("Crypto", "md5") => Ok(Callee::Kernel(KernelFn::CryptoMd5)),
                    ("Crypto", "hmacSha256") => Ok(Callee::Kernel(KernelFn::CryptoHmacSha256)),
                    ("Crypto", "hmacSha512") => Ok(Callee::Kernel(KernelFn::CryptoHmacSha512)),
                    ("Crypto", "rsaSha256Sign") => Ok(Callee::Kernel(KernelFn::CryptoRsaSha256Sign)),
                    ("Crypto", "rsaSha256Verify") => {
                        Ok(Callee::Kernel(KernelFn::CryptoRsaSha256Verify))
                    }
                    ("Crypto", "constantTimeEqual") => {
                        Ok(Callee::Kernel(KernelFn::CryptoConstantTimeEqual))
                    }
                    ("Crypto", "aesGcmEncrypt") => {
                        Ok(Callee::Kernel(KernelFn::CryptoAesGcmEncrypt))
                    }
                    ("Crypto", "aesGcmDecrypt") => {
                        Ok(Callee::Kernel(KernelFn::CryptoAesGcmDecrypt))
                    }
                    ("Crypto", "chacha20Encrypt") => {
                        Ok(Callee::Kernel(KernelFn::CryptoChacha20Encrypt))
                    }
                    ("Crypto", "chacha20Decrypt") => {
                        Ok(Callee::Kernel(KernelFn::CryptoChacha20Decrypt))
                    }
                    ("Crypto", "aesKeyFromPassword") => {
                        Ok(Callee::Kernel(KernelFn::CryptoAesKeyFromPassword))
                    }
                    ("Crypto", "chachaKeyFromPassword") => {
                        Ok(Callee::Kernel(KernelFn::CryptoChachaKeyFromPassword))
                    }
                    ("Crypto", "randomBytes") => Ok(Callee::Kernel(KernelFn::CryptoRandomBytes)),
                    ("Crypto", "randomToken") => Ok(Callee::Kernel(KernelFn::CryptoRandomToken)),
                    // A kernel beyond the wired set (`Time.now`, …).
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
            // `++` is string append; the type checker pinned both operands to
            // `String`, so the backend's `format!` concatenation is sound.
            "append" => Ok(BinOp::Append),
            // The remaining list operator (`::` → `cons`) awaits the list type.
            // [SKY-L0101, feature: binops]
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
            // Literal leaves (M3b-3) lower to the matching refutable IR leaf.
            canon::Pattern_::PInt(n) => Ok(Pat::Int(*n)),
            canon::Pattern_::PBool(b) => Ok(Pat::Bool(*b)),
            canon::Pattern_::PChar(c) => Ok(Pat::Char(c.clone())),
            canon::Pattern_::PStr(s) => Ok(Pat::Str(s.clone())),
            // An alias `inner as name` lowers to the IR binding-with-subpattern.
            canon::Pattern_::PAlias(inner, name) => Ok(Pat::Alias(
                Box::new(Self::lower_payload_pat(inner)?),
                name.value,
            )),
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
            // List / cons sub-patterns carry no Rust `match`-over-`Vec` lowering
            // yet — fail-closed (SKY-L0116) rather than mis-lowered.
            canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _) => {
                Err(unsupported(p.span, Feature::NestedCtorDiscrimination))
            }
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
            // A constructor or literal element is REFUTABLE — it could fail to
            // match — so it cannot bind irrefutably in a `let` / parameter
            // destructure. This is the tuple-pattern gap (SKY-L0115), surfaced
            // fail-closed.
            canon::Pattern_::PCtor { .. }
            | canon::Pattern_::PInt(_)
            | canon::Pattern_::PBool(_)
            | canon::Pattern_::PChar(_)
            | canon::Pattern_::PStr(_) => Err(unsupported(p.span, Feature::TuplePatternMatch)),
            // An alias `inner as name` is irrefutable exactly when `inner` is, so
            // it recurses: a refutable inner surfaces the same SKY-L0115 gap.
            canon::Pattern_::PAlias(inner, name) => Ok(Pat::Alias(
                Box::new(Self::lower_destructure_pat(inner)?),
                name.value,
            )),
            // A record pattern nested inside a tuple destructure needs the
            // element's record type to recover the complete field set; only a
            // top-level record binder is supported (via `lower_binder_pat`).
            // [SKY-L0112]
            canon::Pattern_::PRecord(_) => Err(unsupported(p.span, Feature::NestedPayloadPatterns)),
            // List / cons elements are refutable AND have no `Vec` match lowering
            // yet — fail-closed (SKY-L0116).
            canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _) => {
                Err(unsupported(p.span, Feature::NestedCtorDiscrimination))
            }
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
            // An `inner as name` over an irrefutable destructure binds BOTH the
            // whole value (`name`) and the inner shape. The inner is lowered
            // against the SAME `value` region — an alias does not change the
            // scrutinee's type — so a nested record still recovers its full
            // field set. Lowers to Rust's binding-with-subpattern
            // `name @ <inner>`.
            canon::Pattern_::PAlias(inner, name) => Ok(Pat::Alias(
                Box::new(self.lower_binder_pat(inner, value)?),
                name.value,
            )),
            _ => Self::lower_destructure_pat(pat),
        }
    }

    /// Does this `case`-arm head destructure a product (tuple or record),
    /// possibly under one or more `as` aliases? Such a single arm is an
    /// irrefutable binding rather than an enum match. Peels `PAlias` because
    /// `(a, b) as whole` is just as irrefutable as `(a, b)`.
    fn is_destructure_head(pat: &canon::Pattern_) -> bool {
        match pat {
            canon::Pattern_::PTuple(_) | canon::Pattern_::PRecord(_) => true,
            canon::Pattern_::PAlias(inner, _) => Self::is_destructure_head(&inner.value),
            _ => false,
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
        // `case r of { x, y } -> …`, `case p of (a, b) as whole -> …`) lowers
        // to a `Destructure` binding rather than an `Expr::Match`. The head is
        // a destructure even under one or more `as` aliases. More than one arm
        // would need product exhaustiveness, the tuple-pattern gap (SKY-L0115).
        if Self::is_destructure_head(&first.pat.value) {
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
        // Each Sky `case` arm becomes its OWN Rust `match` arm, in source order.
        // Several arms may head-match the SAME top-level constructor and
        // discriminate on their nested sub-patterns (`Som (Som x)`, `Som Non`,
        // `Non`); Rust's `match` resolves the overlap and ordering natively, so
        // the arms are emitted one-to-one rather than grouped one-per-constructor.
        // Coverage over the nested shape is the exhaustiveness checker's call: it
        // runs before lowering, so a non-exhaustive nested `case` is already
        // SKY-T0010 and never reaches here, and a redundant nested arm is already
        // SKY-T0011. The `Match` constructors below carry only a cheap
        // necessary-condition backstop (every top constructor present / a
        // structural catch-all), never re-deriving that proof.
        //
        // A pure constructor `case` (every arm head a constructor) takes the
        // enum-cover `Match::new` path, whose backstop is the scrutinee's variant
        // set. Any other mix (literal heads, a wildcard / variable catch-all, an
        // alias head, or a constructor + catch-all) takes the FLAT refutable
        // `Match::new_flat` path, whose backstop is structural.
        let all_ctor = branches
            .iter()
            .all(|br| matches!(br.pat.value, canon::Pattern_::PCtor { .. }));

        let arms = branches
            .iter()
            .map(|br| {
                Ok(Arm {
                    pat: Self::lower_arm_pat(&br.pat)?,
                    body: self.lower_expr(&br.body)?,
                })
            })
            .collect::<DResult<Vec<_>>>()?;

        // A list `case` that BINDS a value (a head element or a rest list) needs
        // the backend's owned-rebind (`x.clone()` / `rest.to_vec()`), which
        // requires the element type to be `Clone`. Every CONCRETE element type
        // the backend emits derives `Clone`; a still-generic element type carries
        // no such bound (function generics emit bound-free, M2a), so binding one
        // would emit Rust that fails `go build` — a polymorphic-element list
        // pattern is a not-yet gap (SKY-L0102, feature: polymorphism) rather than
        // broken Rust. A non-binding list `case` (`[] -> … ; _ :: _ -> …`) clones
        // nothing and is unaffected.
        let is_list_case = branches.iter().any(|br| {
            matches!(
                br.pat.value,
                canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _)
            )
        });
        if is_list_case
            && arms.iter().any(|a| Self::pat_binds_value(&a.pat))
            && matches!(self.list_elem_ir(scrut.span)?, IrType::Generic(_))
        {
            return Err(unsupported(first.pat.span, Feature::Polymorphism));
        }

        if all_ctor {
            // The scrutinee's enum is one this module declared (the type checker
            // pinned the constructor's union), so it is always in
            // `enum_variants` — the *true* variant set handed to `Match::new`.
            let canon::Pattern_::PCtor { type_name, .. } = &first.pat.value else {
                return Err(bug(
                    "sky_lower::lower_case",
                    "all-ctor case without a ctor head",
                ));
            };
            let variants = self
                .enum_variants
                .get(type_name)
                .ok_or_else(|| bug("sky_lower::lower_case", "unknown scrutinee enum"))?;
            Ok(Expr::Match(Match::new(scrutinee, arms, variants)?))
        } else {
            Ok(Expr::Match(Match::new_flat(scrutinee, arms)?))
        }
    }

    /// Lower a `case`-arm HEAD pattern to its IR [`Pat`]. Handles the full M3b-3
    /// refutable head set — variable / wildcard binders, the literal leaves
    /// (`0` / `True` / `'a'` / `"hi"`), an alias / `as` binder, and a
    /// constructor pattern (whose payload sub-patterns recurse through
    /// [`Self::lower_payload_pat`]). A tuple / record head is the destructure
    /// path (handled by the single-arm branch of [`Self::lower_case`]); reaching
    /// it here is a multi-arm product `case`, the tuple-pattern gap (SKY-L0115).
    fn lower_arm_pat(p: &canon::Pattern) -> DResult<Pat> {
        match &p.value {
            canon::Pattern_::PVar(s) => Ok(Pat::Var(*s)),
            canon::Pattern_::PAnything => Ok(Pat::Wildcard),
            canon::Pattern_::PInt(n) => Ok(Pat::Int(*n)),
            canon::Pattern_::PBool(b) => Ok(Pat::Bool(*b)),
            canon::Pattern_::PChar(c) => Ok(Pat::Char(c.clone())),
            canon::Pattern_::PStr(s) => Ok(Pat::Str(s.clone())),
            canon::Pattern_::PAlias(inner, name) => Ok(Pat::Alias(
                Box::new(Self::lower_arm_pat(inner)?),
                name.value,
            )),
            canon::Pattern_::PCtor {
                type_name,
                name,
                args,
                ..
            } => {
                let sub = args
                    .iter()
                    .map(Self::lower_payload_pat)
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Pat::Ctor {
                    ty: *type_name,
                    variant: *name,
                    args: sub,
                })
            }
            canon::Pattern_::PTuple(_) | canon::Pattern_::PRecord(_) => {
                Err(unsupported(p.span, Feature::TuplePatternMatch))
            }
            // A list (`[a, b]`) or cons (`x :: xs`) case-arm head flattens to the
            // slice-shaped IR [`Pat::Slice`] (M4a).
            canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _) => Self::lower_list_arm_pat(p),
        }
    }

    /// Lower a list (`[a, b]`) or cons (`x :: xs`) case-arm pattern to the
    /// flattened IR [`Pat::Slice`]. A cons chain `a :: b :: rest` flattens to a
    /// prefix `[a, b]` with the open tail binder `rest`; a `[a, b]` literal
    /// flattens to the same prefix with no tail (an exact-length match); a mixed
    /// `x :: [a, b]` flattens to the closed prefix `[x, a, b]`. Each element
    /// sub-pattern lowers through [`Self::lower_payload_pat`] (variable /
    /// wildcard / literal / alias / nested tuple / constructor); the open tail
    /// binds a variable / wildcard / alias via [`Self::lower_rest_pat`].
    fn lower_list_arm_pat(p: &canon::Pattern) -> DResult<Pat> {
        let mut prefix = Vec::new();
        let mut cur = p;
        loop {
            match &cur.value {
                // A closed list literal terminates the prefix with no open tail.
                canon::Pattern_::PList(elems) => {
                    for e in elems {
                        prefix.push(Self::lower_payload_pat(e)?);
                    }
                    return Ok(Pat::Slice { prefix, rest: None });
                }
                canon::Pattern_::PCons(head, tail) => {
                    prefix.push(Self::lower_payload_pat(head)?);
                    match &tail.value {
                        // A cons / list tail keeps extending the same flattened
                        // slice (`a :: b :: rest`, `x :: [a, b]`).
                        canon::Pattern_::PCons(_, _) | canon::Pattern_::PList(_) => {
                            cur = tail;
                        }
                        // A variable / wildcard tail is the open rest binder —
                        // the remaining list.
                        canon::Pattern_::PVar(_) | canon::Pattern_::PAnything => {
                            let rest = Self::lower_rest_pat(tail)?;
                            return Ok(Pat::Slice {
                                prefix,
                                rest: Some(Box::new(rest)),
                            });
                        }
                        // Any other tail shape (an alias / literal / constructor /
                        // tuple / record in tail position) is not a list pattern
                        // this lowerer models. [SKY-L0116]
                        _ => return Err(unsupported(tail.span, Feature::NestedCtorDiscrimination)),
                    }
                }
                // Only PList / PCons reach here (the caller dispatches on them); a
                // non-list head is a violated invariant.
                _ => {
                    return Err(bug(
                        "sky_lower::lower_list_arm_pat",
                        "non-list pattern reached list-arm lowering",
                    ));
                }
            }
        }
    }

    /// Lower the open TAIL of a cons pattern — the remaining-list binder. A
    /// variable binds the rest list; a wildcard ignores it. A richer tail (an
    /// alias, or a sub-list pattern to match against the rest) is not modelled
    /// yet — it would need a slice binding shape the backend does not emit.
    /// [SKY-L0116]
    const fn lower_rest_pat(p: &canon::Pattern) -> DResult<Pat> {
        match &p.value {
            canon::Pattern_::PVar(s) => Ok(Pat::Var(*s)),
            canon::Pattern_::PAnything => Ok(Pat::Wildcard),
            _ => Err(unsupported(p.span, Feature::NestedCtorDiscrimination)),
        }
    }

    /// Whether an IR pattern introduces a value-binding name (a [`Pat::Var`] or a
    /// [`Pat::Alias`]) anywhere within it. A wildcard / literal binds nothing.
    /// Used by [`Self::lower_case`] to decide whether a list `case` needs the
    /// backend's owned-rebind (and so the element type's `Clone` bound).
    fn pat_binds_value(pat: &Pat) -> bool {
        match pat {
            Pat::Var(_) | Pat::Alias(_, _) => true,
            Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => false,
            Pat::Tuple(subs) => subs.iter().any(Self::pat_binds_value),
            Pat::Ctor { args, .. } => args.iter().any(Self::pat_binds_value),
            Pat::Record(fields) => fields.iter().any(|(_, p)| Self::pat_binds_value(p)),
            Pat::Slice { prefix, rest } => {
                prefix.iter().any(Self::pat_binds_value)
                    || rest.as_deref().is_some_and(Self::pat_binds_value)
            }
        }
    }
}
