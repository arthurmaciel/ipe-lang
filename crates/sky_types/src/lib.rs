#![forbid(unsafe_code)]
//! `sky_types` — Hindley-Milner type inference for the Milestone-0 subset of
//! Sky.
//!
//! Entry point: [`infer`]. It consumes a name-resolved [`sky_canon::ast::Module`]
//! and produces a [`SolvedTypes`] carrying (a) the inferred type of every
//! top-level binding (`env`) and (b) the inferred type of every sub-expression
//! source region (`regions`) — the latter being exactly what the type-directed
//! lowerer reads to fill its `IrType` slots.
//!
//! The implementation is a faithful but narrowed port of the Haskell compiler's
//! `Sky.Type.{Type,UnionFind,Unify,Solve}` + `Constrain.Expression`:
//!
//! * [`unionfind`] — `Vec`-backed weighted union-find (port of `UnionFind`).
//! * [`constrain`] — constraint generation over the canonical AST (M0 arms of
//!   `Constrain.Expression`).
//! * [`unify`] — in-place unification with an occurs check (port of `Unify`).
//! * [`solve`] — budget-bounded constraint discharge (port of `Solve`).
//!
//! ## Interner mutability
//! [`infer`] takes `&mut Interner`. The type checker must *name* built-in type
//! constructors that never appear in user source — notably `Task` (the result
//! of `println`). Minting their [`Symbol`]s requires interning, exactly as the
//! sibling pipeline stages (`parse_module`, `canonicalise`) already take
//! `&mut Interner`. The freshly-interned names flow downstream so the lowerer
//! (which keeps `&Interner`) can resolve them.

mod constrain;
mod doc;
mod exhaust;
mod solve;
mod ty;
mod unify;
mod unionfind;

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, Span, TypeError};
use sky_intern::{Interner, Symbol};

pub use solve::{BUDGET_ENV, Budget, DEFAULT_SOLVER_BUDGET};
pub use ty::{Ty, TyBounds};

use constrain::{Builder, FieldAccess, RecordUpdate, SchemeApp, zonk};
use doc::{VarNamer, ty_to_doc};
use solve::solve;
use ty::{Content, FlatType};
use unify::unify;
use unionfind::{UnionFind, VarId};

/// The result of inference: resolved types for bindings and for every region.
///
/// Mirrors the Haskell `SolvedTypes` record's `_stEnv` + `_stRegions`. Both
/// maps are `BTreeMap`s so iteration is deterministic.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SolvedTypes {
    /// Type of each top-level binding, keyed by `(home_module_path, bare_name)`.
    ///
    /// The qualified key ensures that same-named defs from different modules
    /// (e.g. `Lib.helper` and `Main.helper`) remain distinct after
    /// `link::link` merges them into a single flat def list.  Consumers that
    /// need the inferred type for a specific def must supply **both** the home
    /// path and the bare name; looking up by bare name alone is unsound when
    /// cross-module defs share a name.
    pub env: BTreeMap<(Vec<Symbol>, Symbol), Ty>,
    /// Type of each sub-expression source region, keyed by its [`Span`]. Drives
    /// type-directed lowering.
    pub regions: BTreeMap<Span, Ty>,
    /// Super-type obligations of each typed binding's generic variables: binding
    /// name → (annotation variable symbol → its [`TyBounds`]). Only variables the
    /// body actually constrained appear; a structurally-parametric variable is
    /// absent (its bound is empty). The lowerer turns each obligation into the
    /// matching Rust trait bound on the emitted generic parameter.
    pub bounds: BTreeMap<Symbol, BTreeMap<Symbol, TyBounds>>,
}

/// Infer the types of a canonical module.
///
/// # Errors
/// * [`sky_diagnostics::Diagnostic::Type`] with [`sky_diagnostics::TypeError::Mismatch`]
///   when two types fail to unify, or [`sky_diagnostics::TypeError::BudgetExceeded`]
///   when the solver step budget is exhausted.
/// * [`sky_diagnostics::Diagnostic::CompilerBug`] on a violated internal
///   invariant (dangling union-find id, unbound local, arity mismatch — all
///   unreachable for well-canonicalised input).
pub fn infer(m: &canon::Module, interner: &mut Interner) -> DResult<SolvedTypes> {
    let mut budget = Budget::from_env();
    infer_with_budget(m, interner, &mut budget)
}

/// Inference with an explicit solver budget. Exposed for tests that need to
/// drive the [`sky_diagnostics::TypeError::BudgetExceeded`] path deterministically
/// without mutating process-global environment state.
fn infer_with_budget(
    m: &canon::Module,
    interner: &mut Interner,
    budget: &mut Budget,
) -> DResult<SolvedTypes> {
    let mut uf = UnionFind::new();
    let generated = Builder::run(&mut uf, interner, m)?;

    solve(&mut uf, budget, interner, &generated.constraints)?;

    // Discharge deferred record field accesses now the record types are settled
    // (closed records carry no row variable, so this cannot run during the main
    // solve — see [`FieldAccess`]). Done before the region read-back so each
    // access's result variable reflects the field's type.
    resolve_field_accesses(&mut uf, budget, interner, &generated.field_accesses)?;

    // Discharge deferred record updates the same way: each updated field must
    // exist in the (now-settled) base record's type, and its new value must
    // unify with that field's type.
    resolve_record_updates(&mut uf, budget, interner, &generated.record_updates)?;

    // End-of-checking exhaustiveness + redundancy pass. Running it here — after
    // the solver settles — makes the lowerer's `Match::new` exhaustiveness
    // contract a genuinely unreachable compiler-bug case.
    exhaust::check(m, interner)?;

    // Numeric defaulting: a `Number` variable the program never pinned to a
    // concrete type resolves to `Int` (an untyped `\a b -> a + b` is `Int`, not
    // an under-determined generic). Only super-typed FLEX variables default; an
    // annotation skolem (rigid super) stays generic so its bound surfaces on the
    // emitted type parameter. (Ordering-only flex variables are left generic, as
    // before — they carry no numeric obligation to default.)
    let int_sym = interner.intern("Int")?;
    for (v, orig_bounds, span) in &generated.super_vars {
        let root = uf.find(*v)?;
        match uf.content(root)? {
            // An unpinned `Number` flex defaults to `Int` — an untyped
            // `\a b -> a + b` is `Int`, not an under-determined generic.
            // Ordering / equality flexes carry no numeric default, so an unpinned
            // one is left generic (matching the reference compiler).
            Content::Super {
                rigid: false,
                bounds,
            } if bounds.has_number() => {
                uf.set_content(
                    root,
                    Content::Structure(FlatType::Con {
                        module: Vec::new(),
                        name: int_sym,
                        args: Vec::new(),
                    }),
                )?;
            }
            // An unpinned ordering / equality flex stays generic. A super var is
            // never a plain `Flex` / `Rigid` after solving (it merges as a
            // `Super`, pins to a `Structure`, or adopts a skolem's rigidity as a
            // rigid `Super`), but those arms are covered for totality and need no
            // action either.
            Content::Super { .. } | Content::Flex | Content::Rigid => {}
            // The variable pinned to a concrete type during solving. Verify —
            // deeply, against the fully-resolved type — that the type really
            // supports the operation. The unifier's head pin-check already
            // cleared a function HEAD; this catches a function NESTED inside a
            // tuple / record / enum under an equality obligation (Rust cannot
            // compare it), failing closed with SKY-T0014 instead of emitting
            // code `cargo` rejects.
            Content::Structure(_) => {
                let ty = zonk(&mut uf, budget, root)?;
                if !concrete_super_ok(interner, *orig_bounds, &ty) {
                    return Err(super_unsatisfied(interner, *orig_bounds, &ty, *span));
                }
            }
        }
    }

    // Recover each typed binding's generic-variable super-type obligations from
    // the skolems its body constrained. A variable the body never constrained
    // stays a plain rigid (no obligation, absent from the map).
    let mut bounds: BTreeMap<Symbol, BTreeMap<Symbol, TyBounds>> = BTreeMap::new();
    for (def_name, var_rigids) in &generated.typed_rigids {
        let mut var_bounds = BTreeMap::new();
        for (var_sym, rigid) in var_rigids {
            if let Content::Super { bounds: b, .. } = uf.content(*rigid)?
                && !b.is_empty()
            {
                var_bounds.insert(*var_sym, b);
            }
        }
        if !var_bounds.is_empty() {
            bounds.insert(*def_name, var_bounds);
        }
    }

    // Soundness gate: a super-typed binding used at a concrete type must be used
    // at a type that actually supports the operations its generic emission
    // requires. Without this, `double True` (where `double` needs Number) would
    // type-check here yet emit Rust that `cargo` rejects.
    check_scheme_applications(&mut uf, budget, interner, &bounds, &generated.scheme_apps)?;

    // Read back every region's resolved type.
    let mut regions = BTreeMap::new();
    for (span, var) in generated.regions {
        regions.insert(span, zonk(&mut uf, budget, var)?);
    }

    // `env` = annotation types of typed bindings (exact) + read-back of every
    // untyped binding's inferred body type.
    let mut env = generated.top_level;
    for (name, var) in generated.untyped {
        env.insert(name, zonk(&mut uf, budget, var)?);
    }

    Ok(SolvedTypes {
        env,
        regions,
        bounds,
    })
}

/// Verify every use of a super-typed binding pins each obligated generic
/// variable to a type that satisfies the bound its generic emission requires.
///
/// A binding like `double : a -> a` whose body adds `a` to itself is emitted as
/// a generic function bounded by Rust's `Add` (and `Copy`). A use `double True`
/// instantiates `a` to `Bool`, which provides neither — so it must be rejected
/// *here*, in the type checker, rather than left to fail when `cargo` compiles
/// the emitted Rust. A use that leaves the variable non-concrete (it flows into
/// an enclosing generic, e.g. `f x = double x`) is also rejected: propagating a
/// super-type obligation across binding boundaries is not yet supported, so it
/// is a fail-closed limitation rather than unsound emission.
fn check_scheme_applications(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    bounds: &BTreeMap<Symbol, BTreeMap<Symbol, TyBounds>>,
    apps: &[SchemeApp],
) -> DResult<()> {
    for app in apps {
        let Some(var_bounds) = bounds.get(&app.name) else {
            continue;
        };
        for (var_sym, b) in var_bounds {
            let Some(fresh) = app.vars.get(&var_sym.as_raw()) else {
                continue;
            };
            let ty = zonk(uf, budget, *fresh)?;
            if !emitted_bound_satisfied(interner, *b, &ty) {
                return Err(super_unsatisfied(interner, *b, &ty, app.span));
            }
        }
    }
    Ok(())
}

/// Whether a concrete type satisfies the Rust bound a super-typed *generic*
/// emits at a use site. `Number` / `Comparable` emissions carry `Copy`, so a
/// non-`Copy` orderable type (`String`) is excluded from ordering even though it
/// supports comparison, and both require a bare scalar primitive. An equality
/// emission carries only `PartialEq` (no `Copy`), so it admits any equatable
/// type — every concrete type free of a function ([`ty_is_equatable`]), which
/// includes `String`, tuples, and enums. A non-concrete type (a bare variable
/// the obligation escaped into) satisfies nothing — fail-closed, as cross-
/// binding obligation propagation is not yet supported.
fn emitted_bound_satisfied(interner: &Interner, bounds: TyBounds, ty: &Ty) -> bool {
    let prim = match ty {
        Ty::Con { module, name, args } if module.is_empty() && args.is_empty() => {
            interner.resolve(*name)
        }
        _ => None,
    };
    let number_ok = matches!(prim, Some("Int" | "Float"));
    let ord_ok = matches!(prim, Some("Int" | "Float" | "Char" | "Bool"));
    // A `Set` element / `Dict` key emission carries no `Copy` (the runtime
    // helpers consume by value; `String` keys must be admitted), so the
    // generic-use gate uses the `String`-inclusive scalar set rather than the
    // `Copy`-restricted ordering set above.
    let key_ok = matches!(prim, Some("Int" | "Float" | "Char" | "String" | "Bool"));
    (!bounds.has_number() || number_ok)
        && (!bounds.has_ord() || ord_ok)
        && (!bounds.has_eq() || ty_is_equatable(ty))
        && (!bounds.has_comparable_key() || key_ok)
}

/// Whether a resolved concrete type satisfies super-type obligations `bounds`
/// when a variable pinned *directly* to it (a non-generic, concrete use such as
/// `n == n` on a known type). Mirrors the unifier's head pin-check
/// ([`crate::unify`]'s `super_concrete_ok`) but over the fully-resolved [`Ty`],
/// so it rejects a function nested anywhere inside an equated type — the case
/// the head check defers to here. `String` satisfies ordering (a direct
/// `"a" > "b"` borrows its operands, needing no `Copy`), unlike the
/// generic-emission gate [`emitted_bound_satisfied`].
pub(crate) fn concrete_super_ok(interner: &Interner, bounds: TyBounds, ty: &Ty) -> bool {
    let prim = match ty {
        Ty::Con { module, name, args } if module.is_empty() && args.is_empty() => {
            interner.resolve(*name)
        }
        _ => None,
    };
    let number_ok = matches!(prim, Some("Int" | "Float"));
    let ord_ok = matches!(prim, Some("Int" | "Float" | "Char" | "String" | "Bool"));
    // A `Set` element / `Dict` key pinned directly to a concrete type: the Sky
    // `comparable` scalar set, exactly as ordering. `Float` satisfies the Sky
    // typing here; the Rust-backend `f64`-as-key reality is gated at lowering.
    (!bounds.has_number() || number_ok)
        && (!bounds.has_ord() || ord_ok)
        && (!bounds.has_eq() || ty_is_equatable(ty))
        && (!bounds.has_comparable_key() || ord_ok)
        // Stringify (`toString` / `Log.*With`): showable iff it contains no
        // function anywhere — the SAME "no function nested" rule as equatable,
        // since every non-function type derives `SkyStringify`.
        && (!bounds.has_show() || ty_is_equatable(ty))
}

/// Whether a resolved type derives Rust's `PartialEq`: true for every fully
/// concrete type containing no function anywhere (primitives, unit, tuples,
/// records, and enums all derive `PartialEq`; a function never does). A bare
/// type variable is rejected (fail-closed): an equality obligation that escaped
/// into an enclosing generic is not yet propagated across binding boundaries.
fn ty_is_equatable(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) | Ty::Fun(_, _) => false,
        Ty::Unit => true,
        Ty::Tuple(elems) => elems.iter().all(ty_is_equatable),
        Ty::Record(fields) => fields.values().all(ty_is_equatable),
        Ty::Con { args, .. } => args.iter().all(ty_is_equatable),
    }
}

/// Build the [`TypeError::SuperTypeUnsatisfied`] (SKY-T0014) for a super-typed
/// binding used at a type that does not meet its obligations.
fn super_unsatisfied(interner: &Interner, bounds: TyBounds, ty: &Ty, span: Span) -> Diagnostic {
    // Name every super-type the variable owes, in a fixed order, joined with
    // `+` (`Number + Equatable` when a variable is both added and compared for
    // equality). A bound set always carries at least one obligation at a call
    // site, so the join is non-empty; the fallback keeps the function total.
    let mut classes: Vec<&str> = Vec::new();
    if bounds.has_number() {
        classes.push("Number");
    }
    // A `Set` element / `Dict` key obligation is a Sky `Comparable` (the same
    // class the ordering operators impose); name it once even when both an
    // ordering use and a Set/Dict use constrained the variable.
    if bounds.has_ord() || bounds.has_comparable_key() {
        classes.push("Comparable");
    }
    if bounds.has_eq() {
        classes.push("Equatable");
    }
    if bounds.has_show() {
        classes.push("Stringify");
    }
    let class = if classes.is_empty() {
        "Equatable".to_owned()
    } else {
        classes.join(" + ")
    };
    let mut namer = VarNamer::new();
    let found = match ty_to_doc(ty, interner, &mut namer) {
        Ok(d) => d,
        Err(bug) => return bug,
    };
    Diagnostic::Type {
        span,
        msg: TypeError::SuperTypeUnsatisfied {
            class: class.into_boxed_str(),
            found: Box::new(found),
        },
    }
}

/// Discharge every deferred record field access (`record.field`).
///
/// By the time this runs the main solve has settled each record's type. For each
/// access, the now-resolved record type is read: a closed record carrying the
/// field links the access's result variable to the field's type (so any
/// surrounding constraint already placed on the result, e.g. `record.field + 1`,
/// is checked against the field's real type); a record without the field — or a
/// base that is not a record at all — is a [`TypeError::NoSuchField`] blamed at
/// the access span.
fn resolve_field_accesses(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    accesses: &[FieldAccess],
) -> DResult<()> {
    for fa in accesses {
        let root = uf.find(fa.record)?;
        let field_var = match uf.content(root)? {
            Content::Structure(FlatType::Record(fields)) => fields.get(&fa.field).copied(),
            _ => None,
        };
        match field_var {
            Some(v) => unify(uf, budget, interner, fa.span, fa.result, v)?,
            None => {
                return Err(no_such_field(
                    uf, budget, interner, fa.record, fa.field, fa.span,
                ));
            }
        }
    }
    Ok(())
}

/// Discharge every deferred record update (`{ base | field = value, ... }`).
///
/// By the time this runs the base record's type has settled. For each updated
/// field, the field's type is read from the base record and unified with the new
/// value's type — so changing a field to a value of the wrong type is a normal
/// [`unify`] mismatch, blamed at the update span. A field absent from the base —
/// or a base that is not a record at all — is a [`TypeError::NoSuchField`].
fn resolve_record_updates(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    updates: &[RecordUpdate],
) -> DResult<()> {
    for ru in updates {
        let root = uf.find(ru.record)?;
        // Clone the field map so the base's descriptor is not borrowed across the
        // `unify` calls below (which mutate the arena).
        let base_fields = match uf.content(root)? {
            Content::Structure(FlatType::Record(fields)) => fields,
            _ => BTreeMap::new(),
        };
        for (field, value_var) in &ru.fields {
            match base_fields.get(field).copied() {
                Some(field_var) => unify(uf, budget, interner, ru.span, *value_var, field_var)?,
                None => {
                    return Err(no_such_field(
                        uf, budget, interner, ru.record, *field, ru.span,
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Build the [`TypeError::NoSuchField`] (SKY-T0012) for a field that is absent
/// from the (settled) record type, or whose base is not a record. Shared by the
/// field-access ([`resolve_field_accesses`]) and record-update
/// ([`resolve_record_updates`]) resolution passes; the record type is zonked +
/// rendered here so the reporter needs no arena access.
fn no_such_field(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    record: VarId,
    field: Symbol,
    span: Span,
) -> Diagnostic {
    let field = match interner.resolve(field) {
        Some(s) => Box::from(s),
        None => {
            return Diagnostic::CompilerBug {
                where_: "intern.resolve",
                detail: format!("no backing string for field symbol {}", field.as_raw()),
            };
        }
    };
    let record_ty = match zonk(uf, budget, record) {
        Ok(t) => t,
        Err(bug) => return bug,
    };
    let mut namer = VarNamer::new();
    let record_doc = match ty_to_doc(&record_ty, interner, &mut namer) {
        Ok(d) => d,
        Err(bug) => return bug,
    };
    Diagnostic::Type {
        span,
        msg: TypeError::NoSuchField {
            field,
            record: Box::new(record_doc),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_diagnostics::{Diagnostic, TypeError};

    const GOLDEN: &str = include_str!("../../../tests/golden/m0/Main.sky");

    /// Parse + canonicalise the golden module, returning it plus the interner.
    fn canon_golden() -> Option<(canon::Module, Interner)> {
        let mut i = Interner::new();
        let src = sky_parse::parse_module(GOLDEN, &mut i).ok()?;
        let m = sky_canon::canonicalise(&src, &mut i).ok()?;
        Some((m, i))
    }

    /// Parse + canonicalise + infer an arbitrary single-module source string.
    fn infer_src(src: &str) -> (DResult<SolvedTypes>, Interner, Option<canon::Module>) {
        let mut i = Interner::new();
        let parsed = match sky_parse::parse_module(src, &mut i) {
            Ok(p) => p,
            Err(e) => return (Err(e), i, None),
        };
        let m = match sky_canon::canonicalise(&parsed, &mut i) {
            Ok(m) => m,
            Err(e) => return (Err(e), i, None),
        };
        let solved = infer(&m, &mut i);
        (solved, i, Some(m))
    }

    const M2C_HDR: &str = "module Main exposing (main)\n\n";

    #[test]
    fn generic_record_signature_typechecks() {
        // `wrap : a -> { value : a }` over the identity-shaped body. The env entry
        // is `Fun(Var, Record{ value : Var })` with the SAME variable in both
        // positions (the field is the parameter's type).
        let src = format!(
            "{M2C_HDR}wrap : a -> {{ value : a }}\nwrap x =\n    {{ value = x }}\n\nmain = wrap 1\n"
        );
        let (solved, i, m) = infer_src(&src);
        assert!(
            solved.is_ok(),
            "generic record signature must typecheck: {solved:?}"
        );
        let (Ok(solved), Some(m)) = (solved, m) else {
            return;
        };
        let Some(wrap) = def_key(&i, &m, "wrap") else {
            return;
        };
        let Some(ty) = solved.env.get(&wrap) else {
            return;
        };
        // `wrap : a -> { value : a }` — the parameter's type variable and the
        // record field's type variable must be the SAME id. Extract both ids
        // structurally, then assert their identity (so a wrong shape fails the
        // final assertion rather than via a forbidden `panic!`).
        let ids: Option<(u32, u32)> = match ty {
            Ty::Fun(arg, ret) => match (arg.as_ref(), ret.as_ref()) {
                (Ty::Var(arg_id), Ty::Record(fields)) => fields
                    .iter()
                    .find(|(name, _)| i.resolve(**name) == Some("value"))
                    .and_then(|(_, fty)| match fty {
                        Ty::Var(fid) => Some((*arg_id, *fid)),
                        _ => None,
                    }),
                _ => None,
            },
            _ => None,
        };
        assert!(
            matches!(ids, Some((a, f)) if a == f),
            "wrap is `a -> {{ value : a }}` with the field carrying the parameter's \
             own type variable; got env type {ty:?}"
        );
    }

    #[test]
    fn generic_record_field_access_typechecks() {
        // `unwrap : { value : a } -> a ; unwrap r = r.value` — the deferred
        // field-access links the result to the rigid field var; both are the same
        // skolem, so it checks.
        let src = format!(
            "{M2C_HDR}unwrap : {{ value : a }} -> a\nunwrap r =\n    r.value\n\nmain = unwrap\n"
        );
        let (solved, ..) = infer_src(&src);
        assert!(
            solved.is_ok(),
            "generic field access must typecheck: {solved:?}"
        );
    }

    #[test]
    fn body_constraining_a_record_field_var_is_rejected() {
        // `bad : a -> { value : a } ; bad x = { value = 1 }` pins the rigid field
        // variable `a` to `Int` in the body — the rigid-skolem gate rejects it
        // (bounded generics are M2d), rather than silently accepting it.
        let src = format!(
            "{M2C_HDR}bad : a -> {{ value : a }}\nbad x =\n    {{ value = 1 }}\n\nmain = bad\n"
        );
        let (solved, ..) = infer_src(&src);
        assert!(
            solved.is_err(),
            "a body pinning a rigid record-field variable must be a type error"
        );
    }

    #[test]
    fn record_type_alias_expands_and_typechecks() {
        // `type alias Box a = { value : a }` used in a signature `mk : Int -> Box
        // Int` expands to a closed record and typechecks.
        let src = format!(
            "{M2C_HDR}type alias Box a = {{ value : a }}\n\nmk : Int -> Box Int\nmk n =\n    {{ value = n }}\n\nmain = mk 1\n"
        );
        let (solved, ..) = infer_src(&src);
        assert!(
            solved.is_ok(),
            "record-type alias must expand + typecheck: {solved:?}"
        );
    }

    /// Return the `SolvedTypes::env` key `(home_path, bare_symbol)` for the
    /// named def in a module.  `solved.env` is keyed by the qualified
    /// `(home, name)` pair so same-named defs from different modules never
    /// collide.
    fn def_key(i: &Interner, m: &canon::Module, name: &str) -> Option<(Vec<Symbol>, Symbol)> {
        for d in &m.defs {
            if i.resolve(d.name().value) == Some(name) {
                return Some((d.home().to_vec(), d.name().value));
            }
        }
        None
    }

    /// Drill into a `Call` node.
    fn as_call(e: &canon::Expr) -> Option<(&canon::Expr, &[canon::Expr])> {
        match &e.value {
            canon::Expr_::Call(callee, args) => Some((callee, args)),
            _ => None,
        }
    }

    fn ty_con_name(ty: &Ty, i: &Interner) -> Option<String> {
        match ty {
            Ty::Con { name, .. } => i.resolve(*name).map(str::to_owned),
            _ => None,
        }
    }

    #[test]
    fn env_update_is_msg_to_int_to_int() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden must parse + canonicalise");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };

        let Some(update) = def_key(&i, &m, "update") else {
            return;
        };
        let Some(ty) = solved.env.get(&update) else {
            return;
        };

        // Msg -> (Int -> Int)
        assert!(matches!(ty, Ty::Fun(..)), "update is an arrow");
        let Ty::Fun(msg_arg, tail) = ty else { return };
        assert_eq!(ty_con_name(msg_arg, &i).as_deref(), Some("Msg"));
        assert!(matches!(tail.as_ref(), Ty::Fun(..)), "tail is an arrow");
        let Ty::Fun(int_arg, ret) = tail.as_ref() else {
            return;
        };
        assert_eq!(ty_con_name(int_arg, &i).as_deref(), Some("Int"));
        assert_eq!(ty_con_name(ret, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn regions_carry_call_and_kernel_types() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };

        // main = println (String.fromInt (update Increment 0))
        let main_def = m
            .defs
            .iter()
            .find(|d| i.resolve(d.name().value) == Some("main"));
        assert!(
            matches!(main_def, Some(canon::Def::Untyped { .. })),
            "main is untyped"
        );
        let Some(canon::Def::Untyped { body, .. }) = main_def else {
            return;
        };

        // Outer call: println … : Task ()
        let outer = as_call(body);
        assert!(outer.is_some(), "main body is a call");
        let Some((_println, outer_args)) = outer else {
            return;
        };
        let println_region = solved.regions.get(&body.span);
        assert!(
            matches!(
                println_region,
                Some(Ty::Con { name, args, .. })
                    if i.resolve(*name) == Some("Task") && args.as_slice() == [Ty::Unit]
            ),
            "println region must be Task (): {println_region:?}"
        );

        // String.fromInt … : String
        let Some(from_int_call) = outer_args.first() else {
            return;
        };
        let mid = as_call(from_int_call);
        assert!(mid.is_some(), "fromInt call");
        let Some((_from_int, mid_args)) = mid else {
            return;
        };
        assert_eq!(
            solved
                .regions
                .get(&from_int_call.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("String")
        );

        // update Increment 0 : Int
        let Some(update_call) = mid_args.first() else {
            return;
        };
        assert!(as_call(update_call).is_some(), "update call");
        assert_eq!(
            solved
                .regions
                .get(&update_call.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Int")
        );
    }

    #[test]
    fn regions_carry_scrutinee_and_binop_types() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };

        let update_def = m
            .defs
            .iter()
            .find(|d| i.resolve(d.name().value) == Some("update"));
        assert!(
            matches!(update_def, Some(canon::Def::Typed { .. })),
            "update is typed"
        );
        let Some(canon::Def::Typed { body, .. }) = update_def else {
            return;
        };
        assert!(
            matches!(&body.value, canon::Expr_::Case(..)),
            "update body is case"
        );
        let canon::Expr_::Case(scrut, branches) = &body.value else {
            return;
        };

        // Scrutinee `msg` : Msg
        assert_eq!(
            solved
                .regions
                .get(&scrut.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Msg")
        );

        // First arm body `count + 1` : Int
        let Some(first) = branches.first() else {
            return;
        };
        assert!(
            matches!(first.body.value, canon::Expr_::Binop { .. }),
            "arm body is binop"
        );
        assert_eq!(
            solved
                .regions
                .get(&first.body.span)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Int")
        );
    }

    #[test]
    fn env_main_is_task_unit() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "inference must succeed");
        let Ok(solved) = solved else { return };
        let Some(main) = def_key(&i, &m, "main") else {
            return;
        };
        let main_ty = solved.env.get(&main);
        assert!(
            matches!(
                main_ty,
                Some(Ty::Con { name, args, .. })
                    if i.resolve(*name) == Some("Task") && args.as_slice() == [Ty::Unit]
            ),
            "env[main] must be Task (): {main_ty:?}"
        );
    }

    #[test]
    fn exhausted_budget_yields_budget_exceeded() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        // A budget of one step cannot discharge the golden program's
        // constraints; the very first unify trips the bound.
        let mut budget = Budget::new(1);
        let r = infer_with_budget(&m, &mut i, &mut budget);
        assert!(matches!(
            r,
            Err(Diagnostic::Type {
                msg: TypeError::StepBudgetExceeded { budget: 1 },
                ..
            })
        ));
    }

    #[test]
    fn disabled_budget_still_succeeds() {
        let opt = canon_golden();
        assert!(opt.is_some(), "golden");
        let Some((m, mut i)) = opt else { return };
        let mut budget = Budget::unbounded();
        assert!(infer_with_budget(&m, &mut i, &mut budget).is_ok());
    }

    // ── rich TypeError payloads (E3) ───────────────────────────────────────

    /// Parse + canonicalise an inline module, returning it plus the interner.
    fn canon_src(src: &str) -> Option<(canon::Module, Interner)> {
        let mut i = Interner::new();
        let parsed = sky_parse::parse_module(src, &mut i).ok()?;
        let m = sky_canon::canonicalise(&parsed, &mut i).ok()?;
        Some((m, i))
    }

    fn con_doc(name: &str) -> sky_diagnostics::TyDoc {
        sky_diagnostics::TyDoc::Con {
            module: "".into(),
            name: name.into(),
            args: Box::new([]),
        }
    }

    #[test]
    fn type_mismatch_carries_expected_and_found() {
        // `h : Int` but the body is a `Msg` constructor.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   type Msg = Increment | Decrement\n\
                   h : Int\n\
                   h = Increment\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::TypeMismatch { .. },
                    ..
                })
            ),
            "expected a TypeMismatch, got {r:?}"
        );
        let Err(Diagnostic::Type {
            msg: TypeError::TypeMismatch {
                expected, found, ..
            },
            ..
        }) = r
        else {
            return;
        };
        assert_eq!(*expected, con_doc("Int"));
        // A user type carries its defining module home.
        assert_eq!(
            *found,
            sky_diagnostics::TyDoc::Con {
                module: "Main".into(),
                name: "Msg".into(),
                args: Box::new([]),
            }
        );
    }

    #[test]
    fn if_branches_unify_to_the_annotated_return() {
        // A well-typed `if`: condition `Bool`, both branches `Int`, agreeing
        // with the `Int` return annotation.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   f : Int -> Int\n\
                   f n =\n    if n > 0 then n else 0\n\
                   main =\n    println (String.fromInt (f 1))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(r.is_ok(), "well-typed if must infer: {r:?}");
        let Ok(solved) = r else { return };
        let Some(f) = def_key(&i, &m, "f") else {
            return;
        };
        let Some(Ty::Fun(arg, ret)) = solved.env.get(&f) else {
            return;
        };
        assert_eq!(ty_con_name(arg, &i).as_deref(), Some("Int"));
        assert_eq!(ty_con_name(ret, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn if_condition_must_be_bool() {
        // `if n then …` with `n : Int` — the condition is not `Bool`.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   f : Int -> Int\n\
                   f n =\n    if n then 1 else 0\n\
                   main =\n    println (String.fromInt (f 1))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::TypeMismatch { .. },
                    ..
                })
            ),
            "a non-Bool condition must be a TypeMismatch, got {r:?}"
        );
    }

    #[test]
    fn if_branches_must_agree() {
        // The `then` branch is `Int` and the `else` is a `Msg` constructor —
        // the two branches cannot unify.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   type Msg = Increment | Decrement\n\
                   f : Int -> Int\n\
                   f n =\n    if n > 0 then 1 else Increment\n\
                   main =\n    println (String.fromInt (f 1))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::TypeMismatch { .. },
                    ..
                })
            ),
            "disagreeing branches must be a TypeMismatch, got {r:?}"
        );
    }

    #[test]
    fn too_many_parameters_names_binding_and_signature() {
        // `g : Int` but `g a = 0` binds a parameter the signature has no arrow
        // for.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   g : Int\n\
                   g a = 0\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::TooManyParameters { .. },
                    ..
                })
            ),
            "expected TooManyParameters, got {r:?}"
        );
        let Err(Diagnostic::Type {
            msg: TypeError::TooManyParameters { binding, signature },
            ..
        }) = r
        else {
            return;
        };
        assert_eq!(&*binding, "g");
        assert_eq!(*signature, con_doc("Int"));
    }

    #[test]
    fn non_exhaustive_case_lists_missing_constructors() {
        // The `case` covers only `Increment`; `Decrement` is missing.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   type Msg = Increment | Decrement\n\
                   f : Msg -> Int\n\
                   f msg =\n        case msg of\n            Increment -> 1\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::NonExhaustiveCase { .. },
                    ..
                })
            ),
            "expected NonExhaustiveCase, got {r:?}"
        );
        let Err(Diagnostic::Type {
            msg: TypeError::NonExhaustiveCase { missing },
            ..
        }) = r
        else {
            return;
        };
        let names: Vec<&str> = missing.iter().map(AsRef::as_ref).collect();
        assert_eq!(names, vec!["Decrement"]);
    }

    #[test]
    fn refutable_ctor_def_head_param_is_rejected_t0015() {
        // `f (Just x) = x` — a constructor parameter is a refutable binding
        // position, rejected by the irrefutability gate BEFORE lowering.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   f : Maybe Int -> Int\n\
                   f (Just x) = x\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::RefutablePatternParameter,
                    ..
                })
            ),
            "expected RefutablePatternParameter (SKY-T0015), got {r:?}"
        );
    }

    #[test]
    fn refutable_ctor_lambda_param_is_rejected_t0015() {
        // `\(Just x) -> x` in argument position — the lambda-param sweep must
        // catch it too (the pre-existing Lambda arm dropped its params).
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   apply : (Maybe Int -> Int) -> Int\n\
                   apply f = f (Just 1)\n\
                   main =\n    println (String.fromInt (apply (\\(Just x) -> x)))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::RefutablePatternParameter,
                    ..
                })
            ),
            "expected RefutablePatternParameter (SKY-T0015), got {r:?}"
        );
    }

    #[test]
    fn irrefutable_tuple_and_wildcard_params_pass_the_gate() {
        // `f _ (a, b) = a + b` — a wildcard and a tuple param are both
        // irrefutable, so the gate lets them through (no false positive).
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   f : Int -> (Int, Int) -> Int\n\
                   f _ (a, b) = a + b\n\
                   main =\n    println (String.fromInt (f 9 (1, 2)))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(r.is_ok(), "irrefutable params must pass the gate, got {r:?}");
    }

    #[test]
    fn redundant_case_branch_names_constructor() {
        // `Increment` is matched twice; the case is otherwise exhaustive, so the
        // redundancy is the only finding.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   type Msg = Increment | Decrement\n\
                   f : Msg -> Int\n\
                   f msg =\n        case msg of\n            Increment -> 1\n\
                   \x20           Decrement -> 2\n            Increment -> 3\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::RedundantCaseBranch { .. },
                    ..
                })
            ),
            "expected RedundantCaseBranch, got {r:?}"
        );
        let Err(Diagnostic::Type {
            msg: TypeError::RedundantCaseBranch { constructor },
            ..
        }) = r
        else {
            return;
        };
        assert_eq!(&*constructor, "Increment");
    }

    #[test]
    fn nested_non_exhaustive_case_names_the_missing_nested_pattern() {
        // `Som (Som x)` only matches when the inner value is `Som`, so the value
        // `Som Non` escapes every arm. The usefulness checker must report it as a
        // non-exhaustive case naming the precise missing pattern `Som Non` —
        // BEFORE lowering, so the Rust backend never emits a non-exhaustive match.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   type Opt a = Som a | Non\n\
                   f : Opt (Opt Int) -> Int\n\
                   f o =\n        case o of\n            Som (Som x) -> x\n\
                   \x20           Non -> 0\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::NonExhaustiveCase { .. },
                    ..
                })
            ),
            "expected NonExhaustiveCase, got {r:?}"
        );
        let Err(Diagnostic::Type {
            msg: TypeError::NonExhaustiveCase { missing },
            ..
        }) = r
        else {
            return;
        };
        let names: Vec<&str> = missing.iter().map(AsRef::as_ref).collect();
        assert_eq!(names, vec!["Som Non"], "names the nested missing pattern");
    }

    #[test]
    fn nested_redundant_arm_names_the_subsuming_constructor() {
        // `Som x` (a bare variable payload) already matches every `Som _`, so the
        // later, deeper `Som (Som y)` arm covers no new value. The redundancy
        // finding is computed over the same nested matrix as exhaustiveness, so it
        // must fire even when the useless arm is more specific than the arm that
        // subsumes it — reported as SKY-T0011 naming the top-level `Som`.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   type Opt a = Som a | Non\n\
                   f : Opt (Opt Int) -> Int\n\
                   f o =\n        case o of\n            Som x -> 1\n\
                   \x20           Som (Som y) -> y\n            Non -> 0\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::RedundantCaseBranch { .. },
                    ..
                })
            ),
            "expected RedundantCaseBranch, got {r:?}"
        );
        let Err(Diagnostic::Type {
            msg: TypeError::RedundantCaseBranch { constructor },
            ..
        }) = r
        else {
            return;
        };
        assert_eq!(&*constructor, "Som", "names the subsuming top-level ctor");
    }

    #[test]
    fn nested_exhaustive_case_passes_the_check() {
        // Every nested possibility is covered: `Som (Som x)`, `Som Non`, `Non`.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   type Opt a = Som a | Non\n\
                   f : Opt (Opt Int) -> Int\n\
                   f o =\n        case o of\n            Som (Som x) -> x\n\
                   \x20           Som Non -> 0\n            Non -> 0\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        // Exhaustiveness passes: the two `Som` arms discriminate on their nested
        // sub-pattern and together with `Non` cover every value. So `infer` must
        // succeed (the lowerer then emits one Rust arm per source arm).
        assert!(
            infer(&m, &mut i).is_ok(),
            "an exhaustive nested case must pass the exhaustiveness check"
        );
    }

    #[test]
    fn self_application_is_an_infinite_type() {
        // `f x = x x` forces `a = a -> b`, tripping the occurs check.
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   f x = x x\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::InfiniteType { .. },
                    ..
                })
            ),
            "expected InfiniteType, got {r:?}"
        );
        let Err(Diagnostic::Type {
            msg: TypeError::InfiniteType { var, ty },
            span,
        }) = r
        else {
            return;
        };
        // Real offending span — not DUMMY (the historic bug).
        assert_ne!(span, Span::DUMMY, "occurs-check span must be real");
        // `var` appears on the left of the arrow it would have to equal.
        assert!(matches!(
            ty.as_ref(),
            sky_diagnostics::TyDoc::Fun(lhs, _)
                if matches!(lhs.as_ref(), sky_diagnostics::TyDoc::Var(v) if *v == var)
        ));
    }

    #[test]
    fn exhaustive_case_passes_the_check() {
        // The golden program's `update` covers every `Msg` constructor.
        let opt = canon_golden();
        let Some((m, mut i)) = opt else { return };
        assert!(
            infer(&m, &mut i).is_ok(),
            "an exhaustive, non-redundant program must pass the new pass"
        );
    }

    /// Parse + canonicalise + infer `source`; return the resolved type of the
    /// binding named `which` from the env.
    fn infer_env_ty(source: &str, which: &str) -> Option<(Ty, Interner)> {
        let mut i = Interner::new();
        let src = sky_parse::parse_module(source, &mut i).ok()?;
        let m = sky_canon::canonicalise(&src, &mut i).ok()?;
        let solved = infer(&m, &mut i).ok()?;
        let key = def_key(&i, &m, which)?;
        let ty = solved.env.get(&key)?.clone();
        Some((ty, i))
    }

    /// Walk an arrow type to its final (return) constructor name.
    fn return_con_name(ty: &Ty, i: &Interner) -> Option<String> {
        match ty {
            Ty::Fun(_, rest) => return_con_name(rest, i),
            Ty::Con { name, .. } => i.resolve(*name).map(str::to_owned),
            _ => None,
        }
    }

    #[test]
    fn lambda_binding_infers_a_function_type() {
        // `f = \x -> x + 1` infers `Int -> Int` (the `+ 1` pins both x and the
        // result to Int).
        let opt = infer_env_ty("module Main exposing (f)\nf =\n    \\x -> x + 1\n", "f");
        assert!(opt.is_some(), "f must infer");
        let Some((ty, i)) = opt else { return };
        assert!(matches!(ty, Ty::Fun(..)), "f must be an arrow, got {ty:?}");
        let Ty::Fun(arg, ret) = &ty else { return };
        assert_eq!(ty_con_name(arg, &i).as_deref(), Some("Int"));
        assert_eq!(ty_con_name(ret, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn multi_param_lambda_infers_curried_arrows() {
        // `f = \a b -> a + b` infers `Int -> Int -> Int`.
        let opt = infer_env_ty("module Main exposing (f)\nf =\n    \\a b -> a + b\n", "f");
        assert!(opt.is_some(), "f must infer");
        let Some((ty, i)) = opt else { return };
        assert!(matches!(ty, Ty::Fun(..)), "f must be an arrow, got {ty:?}");
        let Ty::Fun(a, tail) = &ty else { return };
        assert_eq!(ty_con_name(a, &i).as_deref(), Some("Int"));
        assert!(
            matches!(tail.as_ref(), Ty::Fun(..)),
            "tail must be an arrow, got {tail:?}"
        );
        let Ty::Fun(b, ret) = tail.as_ref() else {
            return;
        };
        assert_eq!(ty_con_name(b, &i).as_deref(), Some("Int"));
        assert_eq!(ty_con_name(ret, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn applied_captured_lambda_infers_int() {
        // `(\x -> x + n) 5` with `n = 10` applies a capturing lambda; the whole
        // binding is `Int`.
        let opt = infer_env_ty(
            "module Main exposing (v)\nv =\n    let n = 10 in (\\x -> x + n) 5\n",
            "v",
        );
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(ty_con_name(&ty, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn applying_a_non_function_is_rejected() {
        // `v = 5 1` applies an Int to an argument — `Int` cannot unify with a
        // function type, so it is a type error (no panic, no silent accept).
        let mut i = Interner::new();
        let source = "module Main exposing (v)\nv : Int\nv =\n    let g = 5 in g 1\n";
        let parsed = sky_parse::parse_module(source, &mut i);
        assert!(parsed.is_ok(), "must parse");
        let Ok(src) = parsed else { return };
        let canon = sky_canon::canonicalise(&src, &mut i);
        assert!(canon.is_ok(), "must canonicalise");
        let Ok(m) = canon else { return };
        assert!(
            infer(&m, &mut i).is_err(),
            "applying a non-function must be a type error"
        );
    }

    #[test]
    fn arithmetic_chain_is_int() {
        let opt = infer_env_ty(
            "module Main exposing (v)\nv : Int\nv =\n    2 + 3 * 4 - 1\n",
            "v",
        );
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(ty_con_name(&ty, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn comparison_and_boolean_produce_bool() {
        // `f : Int -> Bool` ⇒ body `n > 10 && n < 100` must be Bool.
        let opt = infer_env_ty(
            "module Main exposing (f)\nf : Int -> Bool\nf n =\n    n > 10 && n < 100\n",
            "f",
        );
        assert!(opt.is_some(), "f must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(
            return_con_name(&ty, &i).as_deref(),
            Some("Bool"),
            "comparison + && yields Bool"
        );
    }

    #[test]
    fn untyped_comparison_infers_bool_return() {
        // No annotation: the inferred return of `g a b = a == b` must be Bool.
        let opt = infer_env_ty("module Main exposing (g)\ng a b =\n    a == b\n", "g");
        assert!(opt.is_some(), "g must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(return_con_name(&ty, &i).as_deref(), Some("Bool"));
    }

    #[test]
    fn boolean_operand_type_mismatch_is_rejected() {
        // `1 && 2` — `&&` demands Bool operands; an Int operand must fail.
        let mut i = Interner::new();
        let source = "module Main exposing (v)\nv : Bool\nv =\n    1 && 2\n";
        let parsed = sky_parse::parse_module(source, &mut i);
        assert!(parsed.is_ok(), "must parse");
        let Ok(src) = parsed else { return };
        let canon = sky_canon::canonicalise(&src, &mut i);
        assert!(canon.is_ok(), "must canonicalise");
        let Ok(m) = canon else { return };
        assert!(
            infer(&m, &mut i).is_err(),
            "Int operand to && must be a type error"
        );
    }

    #[test]
    fn string_append_infers_string() {
        // `"a" ++ "b"` — `++` is `String -> String -> String`, so the result
        // type is `String`.
        let opt = infer_env_ty("module Main exposing (v)\nv =\n    \"a\" ++ \"b\"\n", "v");
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(
            ty_con_name(&ty, &i).as_deref(),
            Some("String"),
            "`++` of two strings infers String, got {ty:?}"
        );
    }

    #[test]
    fn append_on_non_string_operand_is_rejected() {
        // `++` is pinned to `String`; an `Int` operand fails to unify and so is
        // a type error rather than reaching the backend (fail-closed — list
        // `++` is a later batch). Mirrors the would-be `List` rejection.
        let mut i = Interner::new();
        let source = "module Main exposing (v)\nv : Int\nv =\n    1 ++ 2\n";
        let parsed = sky_parse::parse_module(source, &mut i);
        assert!(parsed.is_ok(), "must parse");
        let Ok(src) = parsed else { return };
        let canon = sky_canon::canonicalise(&src, &mut i);
        assert!(canon.is_ok(), "must canonicalise");
        let Ok(m) = canon else { return };
        assert!(
            infer(&m, &mut i).is_err(),
            "Int operand to ++ must be a type error"
        );
    }

    #[test]
    fn tuple_value_infers_tuple_type() {
        // Untyped `v = (1, 2)` infers the product type `(Int, Int)`.
        let opt = infer_env_ty("module Main exposing (v)\nv =\n    (1, 2)\n", "v");
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        let shape = match &ty {
            Ty::Tuple(elems) => Some((
                elems.len(),
                elems
                    .iter()
                    .all(|e| ty_con_name(e, &i).as_deref() == Some("Int")),
            )),
            _ => None,
        };
        assert_eq!(
            shape,
            Some((2, true)),
            "v infers the 2-tuple `(Int, Int)`, got {ty:?}"
        );
    }

    #[test]
    fn tuple_against_int_annotation_is_rejected() {
        // `v : Int` with a tuple body must fail: `(Int, Int)` ≠ `Int`.
        let mut i = Interner::new();
        let source = "module Main exposing (v)\nv : Int\nv =\n    (1, 2)\n";
        let parsed = sky_parse::parse_module(source, &mut i);
        assert!(parsed.is_ok(), "must parse");
        let Ok(src) = parsed else { return };
        let canon = sky_canon::canonicalise(&src, &mut i);
        assert!(canon.is_ok(), "must canonicalise");
        let Ok(m) = canon else { return };
        assert!(
            infer(&m, &mut i).is_err(),
            "a tuple body against an Int annotation must be a type error"
        );
    }

    #[test]
    fn record_value_infers_record_type() {
        // Untyped `v = { x = 1, y = 2 }` infers the closed record type
        // `{ x : Int, y : Int }`.
        let opt = infer_env_ty("module Main exposing (v)\nv =\n    { x = 1, y = 2 }\n", "v");
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        let shape = match &ty {
            Ty::Record(fields) => Some((
                fields.len(),
                fields
                    .values()
                    .all(|t| ty_con_name(t, &i).as_deref() == Some("Int")),
            )),
            _ => None,
        };
        assert_eq!(
            shape,
            Some((2, true)),
            "v infers `{{ x : Int, y : Int }}`, got {ty:?}"
        );
    }

    #[test]
    fn field_access_infers_the_field_type() {
        // `let p = { x = 1, y = 2 } in p.x` has the field's type, `Int`.
        let opt = infer_env_ty(
            "module Main exposing (v)\nv =\n    let p = { x = 1, y = 2 } in p.x\n",
            "v",
        );
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(ty_con_name(&ty, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn field_access_constrains_through_arithmetic() {
        // `p.x + p.y` forces both fields to `Int`; the whole binding is `Int`.
        let opt = infer_env_ty(
            "module Main exposing (v)\nv =\n    let p = { x = 1, y = 2 } in p.x + p.y\n",
            "v",
        );
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(ty_con_name(&ty, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn accessing_a_missing_field_is_no_such_field() {
        // `{ x = 1 }` has no `y`: a closed record rejects the access (SKY-T0012).
        let source = "module Main exposing (v)\nv =\n    let p = { x = 1 } in p.y\n";
        let Some((m, mut i)) = canon_src(source) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::NoSuchField { .. },
                    ..
                })
            ),
            "a missing field must be NoSuchField, got {r:?}"
        );
    }

    #[test]
    fn accessing_a_field_on_a_non_record_is_no_such_field() {
        // `p` is an `Int`, so `p.x` has no field to read (SKY-T0012).
        let source = "module Main exposing (v)\nv =\n    let p = 5 in p.x\n";
        let Some((m, mut i)) = canon_src(source) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::NoSuchField { .. },
                    ..
                })
            ),
            "a field on a non-record must be NoSuchField, got {r:?}"
        );
    }

    #[test]
    fn record_update_has_the_base_record_type() {
        // `{ p | x = 41 }` is the same record type as `p`, so reading `q.x`
        // afterwards is an `Int`.
        let opt = infer_env_ty(
            "module Main exposing (v)\nv =\n    let p = { x = 1, y = 2 } in let q = { p | x = 41 } in q.y\n",
            "v",
        );
        assert!(opt.is_some(), "v must infer");
        let Some((ty, i)) = opt else { return };
        assert_eq!(ty_con_name(&ty, &i).as_deref(), Some("Int"));
    }

    #[test]
    fn updating_a_missing_field_is_no_such_field() {
        // `{ p | z = 0 }` where `p` has only `x`/`y`: a closed record rejects the
        // update of an absent field (SKY-T0012).
        let source =
            "module Main exposing (v)\nv =\n    let p = { x = 1, y = 2 } in { p | z = 0 }\n";
        let Some((m, mut i)) = canon_src(source) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::NoSuchField { .. },
                    ..
                })
            ),
            "updating a missing field must be NoSuchField, got {r:?}"
        );
    }

    #[test]
    fn updating_a_field_to_the_wrong_type_is_rejected() {
        // `p.x` is an `Int`; updating it to a record `{ a = 1 }` cannot unify, so
        // the whole binding is a type error.
        let source = "module Main exposing (v)\nv =\n    let p = { x = 1, y = 2 } in { p | x = { a = 1 } }\n";
        let Some((m, mut i)) = canon_src(source) else {
            return;
        };
        assert!(
            infer(&m, &mut i).is_err(),
            "updating a field to a value of the wrong type must be a type error"
        );
    }

    #[test]
    fn updating_a_field_on_a_non_record_is_no_such_field() {
        // `p` is an `Int`, so `{ p | x = 1 }` has no field to update (SKY-T0012).
        let source = "module Main exposing (v)\nv =\n    let p = 5 in { p | x = 1 }\n";
        let Some((m, mut i)) = canon_src(source) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            matches!(
                r,
                Err(Diagnostic::Type {
                    msg: TypeError::NoSuchField { .. },
                    ..
                })
            ),
            "updating a field on a non-record must be NoSuchField, got {r:?}"
        );
    }

    #[test]
    fn records_with_different_field_sets_do_not_unify() {
        // `{ x = 1 } == { y = 1 }`: closed records unify only at equal field
        // sets, so this is a type error.
        let source = "module Main exposing (v)\nv : Bool\nv =\n    { x = 1 } == { y = 1 }\n";
        let Some((m, mut i)) = canon_src(source) else {
            return;
        };
        assert!(
            infer(&m, &mut i).is_err(),
            "records with different field sets must not unify"
        );
    }

    #[test]
    fn tuple_arity_mismatch_is_rejected() {
        // Comparing a 2-tuple with a 3-tuple must fail: tuples unify only at
        // equal arity.
        let mut i = Interner::new();
        let source = "module Main exposing (v)\nv : Bool\nv =\n    (1, 2) == (1, 2, 3)\n";
        let parsed = sky_parse::parse_module(source, &mut i);
        assert!(parsed.is_ok(), "must parse");
        let Ok(src) = parsed else { return };
        let canon = sky_canon::canonicalise(&src, &mut i);
        assert!(canon.is_ok(), "must canonicalise");
        let Ok(m) = canon else { return };
        assert!(
            infer(&m, &mut i).is_err(),
            "2-tuple vs 3-tuple must be a type error"
        );
    }

    // ── M2a: let-generalization + per-call-site instantiation ───────────────

    /// A polymorphic annotation `a -> a` reads back into `env` as one quantified
    /// variable used on both sides of the arrow — `Fun(Var p, Var p)` with the
    /// *same* `p`. That single quantified var is what a later lowering pass turns
    /// into one Rust generic parameter (`fn identity<T1>(x: T1) -> T1`).
    #[test]
    fn polymorphic_identity_generalises_to_one_var() {
        let opt = infer_env_ty(
            "module Main exposing (identity)\n\
             import Sky.Core.Prelude exposing (..)\n\
             identity : a -> a\n\
             identity x =\n    x\n",
            "identity",
        );
        assert!(opt.is_some(), "identity must infer");
        let Some((ty, _i)) = opt else { return };
        assert!(
            matches!(&ty, Ty::Fun(a, r)
                if matches!((a.as_ref(), r.as_ref()),
                    (Ty::Var(x), Ty::Var(y)) if x == y)),
            "identity must generalise to one quantified var `a -> a`, got {ty:?}"
        );
    }

    /// One polymorphic function, two concrete uses in the same module: applied to
    /// an `Int` and to a `Bool`, both must type-check. Each `VarTopLevel`
    /// reference instantiates `identity`'s scheme into *fresh* variables, so the
    /// two uses are satisfied independently (Rust later monomorphises the single
    /// generic fn at both types).
    #[test]
    fn polymorphic_identity_used_at_int_and_bool_both_unify() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   identity : a -> a\n\
                   identity x =\n    x\n\
                   useInt : Int\n\
                   useInt =\n    identity 5\n\
                   useBool : Bool\n\
                   useBool =\n    identity (0 == 0)\n\
                   main =\n    println (String.fromInt useInt)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let r = infer(&m, &mut i);
        assert!(
            r.is_ok(),
            "identity used at Int and Bool in one module must infer: {r:?}"
        );
        let Ok(solved) = r else { return };
        // The two consumers settle at their concrete result types.
        let Some(use_int) = def_key(&i, &m, "useInt") else {
            return;
        };
        let Some(use_bool) = def_key(&i, &m, "useBool") else {
            return;
        };
        assert_eq!(
            solved
                .env
                .get(&use_int)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Int")
        );
        assert_eq!(
            solved
                .env
                .get(&use_bool)
                .and_then(|t| ty_con_name(t, &i))
                .as_deref(),
            Some("Bool")
        );
    }

    /// `const : a -> b -> a` keeps two *distinct* quantified variables: the first
    /// parameter and the return share one, the second is its own. Confirms the
    /// per-signature instantiation maps each annotation variable consistently
    /// without conflating different ones.
    #[test]
    fn const_keeps_two_distinct_type_vars() {
        let opt = infer_env_ty(
            "module Main exposing (constant)\n\
             import Sky.Core.Prelude exposing (..)\n\
             constant : a -> b -> a\n\
             constant x y =\n    x\n",
            "constant",
        );
        assert!(opt.is_some(), "constant must infer");
        let Some((ty, _i)) = opt else { return };
        // `a -> b -> a`: positions 1 and 3 share one var; position 2 is distinct.
        assert!(
            matches!(&ty, Ty::Fun(a1, tail)
                if matches!(tail.as_ref(), Ty::Fun(b, a2)
                    if matches!((a1.as_ref(), b.as_ref(), a2.as_ref()),
                        (Ty::Var(x), Ty::Var(y), Ty::Var(z)) if x == z && x != y))),
            "constant must be `a -> b -> a` (first param == result, distinct from second), got {ty:?}"
        );
    }

    /// `apply : (a -> b) -> a -> b` — a structural pass-through over a function
    /// argument — infers with `a` and `b` threaded through correctly.
    #[test]
    fn higher_order_apply_infers_structurally() {
        let opt = infer_env_ty(
            "module Main exposing (apply)\n\
             import Sky.Core.Prelude exposing (..)\n\
             apply : (a -> b) -> a -> b\n\
             apply f x =\n    f x\n",
            "apply",
        );
        assert!(opt.is_some(), "apply must infer");
        let Some((ty, _i)) = opt else { return };
        // `(a -> b) -> a -> b`: the `a`s match, the `b`s match, `a` != `b`.
        assert!(
            matches!(&ty, Ty::Fun(fa, tail)
                if matches!((fa.as_ref(), tail.as_ref()),
                    (Ty::Fun(a1, b1), Ty::Fun(a2, b2))
                    if matches!((a1.as_ref(), b1.as_ref(), a2.as_ref(), b2.as_ref()),
                        (Ty::Var(va1), Ty::Var(vb1), Ty::Var(va2), Ty::Var(vb2))
                        if va1 == va2 && vb1 == vb2 && va1 != vb1))),
            "apply must be `(a -> b) -> a -> b`, got {ty:?}"
        );
    }

    /// `bad : a -> b; bad x = x` returns a value of the parameter's type from a
    /// signature that promised an *independent* return variable. The rigid
    /// (skolem) check rejects it — the body cannot conflate two distinct
    /// annotation variables.
    #[test]
    fn annotation_returning_a_different_var_is_rejected() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   bad : a -> b\n\
                   bad x =\n    x\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        assert!(
            matches!(
                infer(&m, &mut i),
                Err(Diagnostic::Type {
                    msg: TypeError::TypeMismatch { .. },
                    ..
                })
            ),
            "returning the parameter from `a -> b` must be a type mismatch"
        );
    }

    /// `f : a -> a; f x = x + 1` annotates a fully-parametric `a`, but the
    /// literal `1` pins `a` to `Int`. The annotation promised *any* type while
    /// the body needs a concrete one, so the rigid skolem `a` meeting the `Int`
    /// the literal forces is a mismatch — the signature is too general for its
    /// body. (Contrast `f x = x + x`, which carries no literal: `a` stays a
    /// Number-bounded generic.)
    #[test]
    fn parametric_annotation_body_forcing_concrete_is_rejected() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   f : a -> a\n\
                   f x =\n    x + 1\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        assert!(
            matches!(
                infer(&m, &mut i),
                Err(Diagnostic::Type {
                    msg: TypeError::TypeMismatch { .. },
                    ..
                })
            ),
            "a body pinning a parametric `a` to Int must be a type mismatch"
        );
    }

    /// An *un*annotated binding reconstructs its full arrow type into `env`
    /// (parameters included), and an unconstrained parameter generalises: for
    /// `k a b = a`, `env[k]` is `a -> b -> a` with the first parameter and the
    /// result sharing one inferred variable.
    #[test]
    fn untyped_binding_reconstructs_and_generalises_arrow() {
        let opt = infer_env_ty(
            "module Main exposing (k)\n\
             import Sky.Core.Prelude exposing (..)\n\
             k a b =\n    a\n",
            "k",
        );
        assert!(opt.is_some(), "k must infer");
        let Some((ty, _i)) = opt else { return };
        // Reconstructed `a -> b -> a` (params included), first param == result.
        assert!(
            matches!(&ty, Ty::Fun(a1, tail)
                if matches!(tail.as_ref(), Ty::Fun(b, a2)
                    if matches!((a1.as_ref(), b.as_ref(), a2.as_ref()),
                        (Ty::Var(x), Ty::Var(y), Ty::Var(z)) if x == z && x != y))),
            "k must reconstruct + generalise to `a -> b -> a`, got {ty:?}"
        );
    }

    /// Documents the M2a limitation: an *un*annotated polymorphic binding is
    /// monomorphic at its use sites (no rank-based generalisation yet), so using
    /// it at two different concrete types in one module is a sound rejection. The
    /// fix is to annotate it (see
    /// [`polymorphic_identity_used_at_int_and_bool_both_unify`]).
    #[test]
    fn untyped_polymorphic_use_at_two_types_is_rejected() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   ident x =\n    x\n\
                   useInt : Int\n\
                   useInt =\n    ident 5\n\
                   useBool : Bool\n\
                   useBool =\n    ident (0 == 0)\n\
                   main =\n    println (String.fromInt useInt)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        assert!(
            infer(&m, &mut i).is_err(),
            "an unannotated binding used at Int and Bool must be rejected (monomorphic)"
        );
    }

    /// The single recorded [`TyBounds`] of a binding's one bounded variable, or
    /// `None` if the binding recorded no obligated variable.
    fn sole_bound(solved: &SolvedTypes, i: &mut Interner, binding: &str) -> Option<TyBounds> {
        let sym = i.intern(binding).ok()?;
        solved.bounds.get(&sym)?.values().next().copied()
    }

    /// `double : a -> a; double x = x + x` constrains `a` numerically (no literal
    /// pins it), so instead of the rigid-skolem rejection a structurally-
    /// parametric variable would get, `a` carries the `Add` (Number) obligation.
    #[test]
    fn number_generic_double_carries_add_bound() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   double : a -> a\n\
                   double x =\n    x + x\n\
                   main =\n    println (String.fromInt (double 21))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "double must type-check, got {solved:?}");
        let Ok(solved) = solved else { return };
        let bound = sole_bound(&solved, &mut i, "double");
        assert!(bound.is_some(), "double records a bound");
        let Some(b) = bound else { return };
        assert!(b.has_add(), "double's `a` carries the Add (Number) bound");
        assert!(
            !b.has_ord() && !b.has_sub() && !b.has_mul(),
            "double needs only Add, got {b:?}"
        );
    }

    /// `maxOf : a -> a -> a; maxOf p q = if p > q then p else q` orders `a`, so
    /// `a` carries the `PartialOrd` (Comparable) obligation.
    #[test]
    fn comparable_generic_max_carries_ord_bound() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   maxOf : a -> a -> a\n\
                   maxOf p q =\n    if p > q then p else q\n\
                   main =\n    println (String.fromInt (maxOf 3 7))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        let solved = infer(&m, &mut i);
        assert!(solved.is_ok(), "maxOf must type-check, got {solved:?}");
        let Ok(solved) = solved else { return };
        let bound = sole_bound(&solved, &mut i, "maxOf");
        assert!(bound.is_some(), "maxOf records a bound");
        let Some(b) = bound else { return };
        assert!(b.has_ord(), "maxOf's `a` carries the ordering bound");
        assert!(
            !b.has_add() && !b.has_sub() && !b.has_mul(),
            "maxOf needs only ordering, got {b:?}"
        );
    }

    /// A Number generic used at both `Int` (a literal) and `Float` (through an
    /// annotated forwarder) type-checks: both satisfy the `Add` obligation.
    #[test]
    fn number_generic_used_at_int_and_float_checks() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   double : a -> a\n\
                   double x =\n    x + x\n\
                   doubleFloat : Float -> Float\n\
                   doubleFloat x =\n    double x\n\
                   main =\n    println (String.fromInt (double 21))\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        assert!(
            infer(&m, &mut i).is_ok(),
            "double used at Int and Float must type-check"
        );
    }

    /// A Number generic used at `Bool` is rejected: `Bool` is not a `Number`, so
    /// the use surfaces SKY-T0014 rather than emitting Rust `cargo` cannot build.
    #[test]
    fn number_generic_at_bool_is_super_type_unsatisfied() {
        let src = "module Main exposing (main)\n\
                   import Sky.Core.Prelude exposing (..)\n\
                   double : a -> a\n\
                   double x =\n    x + x\n\
                   doubleBool : Bool -> Bool\n\
                   doubleBool x =\n    double x\n\
                   main =\n    println (String.fromInt 0)\n";
        let Some((m, mut i)) = canon_src(src) else {
            return;
        };
        assert!(
            matches!(
                infer(&m, &mut i),
                Err(Diagnostic::Type {
                    msg: TypeError::SuperTypeUnsatisfied { .. },
                    ..
                })
            ),
            "a Number generic used at Bool must be SuperTypeUnsatisfied"
        );
    }

    /// An unannotated `\\a b -> a + b` is `Int -> Int -> Int` by numeric
    /// defaulting: the body constrains the parameters to `Number`, and a
    /// `Number` the program never pins resolves to `Int`.
    #[test]
    fn unpinned_numeric_binding_defaults_to_int() {
        let opt = infer_env_ty("module Main exposing (f)\nf =\n    \\a b -> a + b\n", "f");
        assert!(opt.is_some(), "f must infer");
        let Some((ty, i)) = opt else { return };
        // `Int -> Int -> Int`.
        assert_eq!(return_con_name(&ty, &i).as_deref(), Some("Int"));
        assert!(matches!(ty, Ty::Fun(..)), "f must be an arrow, got {ty:?}");
        let Ty::Fun(a, tail) = &ty else { return };
        assert_eq!(ty_con_name(a, &i).as_deref(), Some("Int"));
        assert!(
            matches!(tail.as_ref(), Ty::Fun(..)),
            "f's tail must be an arrow, got {tail:?}"
        );
        let Ty::Fun(b, _) = tail.as_ref() else { return };
        assert_eq!(ty_con_name(b, &i).as_deref(), Some("Int"));
    }
}
