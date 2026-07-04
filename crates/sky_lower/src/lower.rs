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

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, Feature, Located, LowerError, Span};
use sky_intern::{Interner, Symbol};
use sky_ir::{
    Arm, BinOp, BoundSet, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath,
    Module, Pat, Program, TypeDef, UiCtor, UiPlain, Variant,
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

/// `Task Error a` carries two arguments in a user annotation (Error, a); an
/// arity guard cleared that, so a missing argument is an internal invariant.
fn task_arg_bug() -> Diagnostic {
    bug(
        "sky_lower::ir_type",
        "Task applied without its success type",
    )
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

/// The built-in, heap-boxed OPAQUE wrapper type constructors whose payload the
/// runtime stores behind a `Box<dyn Fn>` / trait object and NEVER derives
/// `Clone`/`Debug`/`PartialEq`/`SkyStringify` over.
///
/// A function in one of their type arguments is therefore legitimate — a
/// `Decoder (a -> b)` factory is the entire point of `JsonDec.succeed makeRecord
/// |> required … |> required …`, and a `Cmd`/`Sub`/`Task` may carry a callback —
/// so such a value must NOT be flagged as a non-derivable-function carrier the
/// way a user enum's payload (`type Opt a = Som a`, `Opt (Int -> Int)`) is. Each
/// maps to `IrType::Decoder` / `IrType::Task` / `IrType::Cmd` / `IrType::Sub`,
/// aliased in the emitted project to a runtime type that boxes its payload
/// (`sky_runtime::json::Decoder<E, T>` holds a `Box<dyn Fn(&JsonVal) -> …>`);
/// the payload `T` is opaque to any derive, and the emitter already lowers
/// `decode_succeed(curryN(f))`.
///
/// Matched by name only — consistent with [`Lowerer::ir_type_from_ty`], and
/// sound because these are kernel-implicit Prelude type constructors the
/// canonicaliser forbids a user program from redefining.
fn is_opaque_boxed_wrapper(interner: &Interner, name: Symbol) -> bool {
    matches!(
        interner.resolve(name),
        Some("Decoder" | "Task" | "Cmd" | "Sub")
    )
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
///
/// Exception: a built-in opaque boxed wrapper ([`is_opaque_boxed_wrapper`] —
/// `Decoder`/`Task`/`Cmd`/`Sub`) boxes its payload and derives nothing over it,
/// so a function in its type arguments is a legitimate value, not a
/// non-derivable carrier. Such a `Con` head short-circuits to `false`. A wrapper
/// value nested INSIDE a real derive carrier is still caught by that outer
/// carrier's own [`ty_contains_fun`] check (unchanged), so this only exempts the
/// wrapper as the outermost shape.
fn embeds_nonderivable_function(interner: &Interner, ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) | Ty::Unit => false,
        Ty::Fun(a, b) => {
            embeds_nonderivable_function(interner, a) || embeds_nonderivable_function(interner, b)
        }
        Ty::Tuple(elems) => elems
            .iter()
            .any(|e| embeds_nonderivable_function(interner, e)),
        // An opaque boxed wrapper (`Decoder (a -> b)`, `Cmd msg`, …) stores its
        // payload behind a trait object and derives nothing over it — a function
        // there is legitimate, so it is NOT a non-derivable carrier.
        Ty::Con { name, .. } if is_opaque_boxed_wrapper(interner, *name) => false,
        // Otherwise a `Con` is a user enum (which derives `Clone`/`Debug`/
        // `PartialEq` + `SkyStringify`) applied to its type arguments. A function
        // reaching a payload field — directly (`Opt (Int -> Int)`) or nested
        // inside another payload/record under it — makes those derives fail, so
        // it is the same non-derivable shape as a function in a record field.
        Ty::Con { args, .. } => args
            .iter()
            .any(|a| ty_contains_fun(a) || embeds_nonderivable_function(interner, a)),
        Ty::Record(fields) => fields
            .values()
            .any(|f| ty_contains_fun(f) || embeds_nonderivable_function(interner, f)),
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
///
/// A built-in opaque boxed wrapper head ([`is_opaque_boxed_wrapper`]) is not a
/// user-enum payload carrier and is excluded — though in practice
/// [`embeds_nonderivable_function`] already returns `false` for such a bare head,
/// so this is only reached for genuine user-enum `Con`s.
fn con_payload_carries_function(interner: &Interner, ty: &Ty) -> bool {
    matches!(ty, Ty::Con { name, args, .. }
        if !is_opaque_boxed_wrapper(interner, *name)
            && args.iter().any(|a| ty_contains_fun(a) || embeds_nonderivable_function(interner, a)))
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
        // `SkyTask<E,A>`, `SkyCmd<M>`, `SkySub<M>` are opaque runtime types; the
        // inner type parameter might itself embed a function, so recurse.
        IrType::Task(inner) | IrType::Cmd(inner) | IrType::Sub(inner) => ir_contains_fun(inner),
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        // `Decoder<T>` is an opaque struct, not a function type.
        | IrType::Decoder(_)
        // `Db` is an opaque connection pool handle, not a function type.
        | IrType::Db
        // M6 opaque server types are opaque handles, not function types.
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        | IrType::Generic(_)
        // M7: nullary plain types (`Length`, `Color`, etc.) trivially contain no
        // functions.  `LiveReq` / `LiveRoute` are opaque handles with no `Fn` fields.
        | IrType::UiPlain(_)
        | IrType::LiveReq
        | IrType::LiveRoute => false,
        IrType::Enum { args, .. } => args.iter().any(ir_contains_fun),
        IrType::Maybe(elem) | IrType::List(elem) => ir_contains_fun(elem),
        IrType::Result(err, ok) => ir_contains_fun(err) || ir_contains_fun(ok),
        IrType::Dict(k, v) => ir_contains_fun(k) || ir_contains_fun(v),
        IrType::Set(a) => ir_contains_fun(a),
        IrType::Tuple(elems) => elems.iter().any(ir_contains_fun),
        IrType::Record(fields) => fields.values().any(ir_contains_fun),
        // M7: `Element<M>` / `Html<M>` carry a msg type parameter — recurse.
        IrType::Ui { msg, .. } => ir_contains_fun(msg),
    }
}

// ── TCO (#49): tail-recursion detection + rewrite ────────────────────────────
//
// Mirrors the reference implementation (`Sky.Build.TailCallOpt`:
// `isTailRecursive` / `rewriteTailCalls`), improving the jump transport (a typed
// `Expr::TailRecur`, never a stringly kernel-name sentinel) and the self-call
// identity (`FuncId`, not `(module, name)`).

/// Outcome of the tail-recursion analysis for one `Func`. Computed once; the
/// rewrite consumes it. Distinct constructors keep "should we TCO?" a value —
/// never a re-derived predicate.
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TailRecursion {
    /// No self-call, or ≥ 1 self-call in non-tail position → leave as ordinary
    /// recursion (Limitation #8, O(N) stack).
    NotTailRecursive,
    /// Every self-call is a tail-position call at the correct arity, and there is
    /// ≥ 1 of them → safe to rewrite to a loop.
    TailRecursive,
}

/// Classify `body` for TCO. Semantics mirror the reference's `isTailRecursive`:
/// `tail_self_calls > 0 && non_tail_self_calls == 0`.
#[doc(hidden)]
#[must_use]
pub fn analyze_tail_recursion(self_id: FuncId, arity: usize, body: &Expr) -> TailRecursion {
    let mut tail = 0usize;
    let mut non_tail = 0usize;
    count_self_calls(self_id, arity, body, true, &mut tail, &mut non_tail);
    if tail > 0 && non_tail == 0 {
        TailRecursion::TailRecursive
    } else {
        TailRecursion::NotTailRecursive
    }
}

/// Walk `expr`, counting self-calls to `self_id` split by tail vs non-tail
/// position. `in_tail` is `true` only where the enclosing context puts `expr` in
/// tail position: the trailing expression, `If.then_`/`.else_` (never `.cond`),
/// every `Match` arm body (never the scrutinee), and `Let`/`Destructure` bodies
/// (never their `value`). Every other descent — critically `Lambda.body`, all
/// call/apply arguments, operands, list/tuple/record/ctor elements, and both
/// `TaskSeq` sub-terms — is non-tail.
fn count_self_calls(
    self_id: FuncId,
    arity: usize,
    expr: &Expr,
    in_tail: bool,
    tail: &mut usize,
    non_tail: &mut usize,
) {
    match expr {
        // A direct call to the enclosing fn.
        Expr::Call {
            callee: Callee::Func(id),
            args,
        } if *id == self_id => {
            if in_tail && args.len() == arity {
                *tail += 1;
            } else {
                // A tail self-call at the WRONG arity, or a self-call in a
                // non-tail position, is a genuine escape the loop must not touch:
                // count it as non-tail so it disqualifies TCO.
                *non_tail += 1;
            }
            // Arguments are ALWAYS non-tail, regardless of the call's position.
            for a in args {
                count_self_calls(self_id, arity, a, false, tail, non_tail);
            }
        }
        // Forms that descend into an `args` vector with every element non-tail: a
        // call to a DIFFERENT fn / kernel (self-calls are handled by the guarded
        // arm above), a constructor application, and a `TailRecur` jump (its
        // next-iteration args are evaluated non-tail).
        Expr::Call { args, .. } | Expr::Ctor { args, .. } | Expr::TailRecur { args } => {
            for a in args {
                count_self_calls(self_id, arity, a, false, tail, non_tail);
            }
        }
        // A first-class reference to OUR fn that is not a direct call = escape.
        Expr::FuncValue {
            callee: Callee::Func(id),
            ..
        } if *id == self_id => {
            *non_tail += 1;
        }
        Expr::Apply { func, args } => {
            count_self_calls(self_id, arity, func, false, tail, non_tail);
            for a in args {
                count_self_calls(self_id, arity, a, false, tail, non_tail);
            }
        }
        // Tail propagators.
        Expr::If { cond, then_, else_ } => {
            count_self_calls(self_id, arity, cond, false, tail, non_tail);
            count_self_calls(self_id, arity, then_, in_tail, tail, non_tail);
            count_self_calls(self_id, arity, else_, in_tail, tail, non_tail);
        }
        Expr::Match(m) => {
            count_self_calls(self_id, arity, m.scrutinee(), false, tail, non_tail);
            for arm in m.arms() {
                count_self_calls(self_id, arity, &arm.body, in_tail, tail, non_tail);
            }
        }
        // `Let` and `Destructure` share the shape `value` (non-tail) + `body`
        // (in tail position).
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            count_self_calls(self_id, arity, value, false, tail, non_tail);
            count_self_calls(self_id, arity, body, in_tail, tail, non_tail);
        }
        // Non-tail descents.
        Expr::Lambda { body, .. } => {
            count_self_calls(self_id, arity, body, false, tail, non_tail);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            count_self_calls(self_id, arity, lhs, false, tail, non_tail);
            count_self_calls(self_id, arity, rhs, false, tail, non_tail);
        }
        Expr::Cons { head, tail: t } => {
            count_self_calls(self_id, arity, head, false, tail, non_tail);
            count_self_calls(self_id, arity, t, false, tail, non_tail);
        }
        Expr::Tuple(xs) | Expr::List { items: xs, .. } => {
            for x in xs {
                count_self_calls(self_id, arity, x, false, tail, non_tail);
            }
        }
        Expr::Record(fs) => {
            for (_, v) in fs {
                count_self_calls(self_id, arity, v, false, tail, non_tail);
            }
        }
        Expr::Update { record, fields } => {
            count_self_calls(self_id, arity, record, false, tail, non_tail);
            for (_, v) in fields {
                count_self_calls(self_id, arity, v, false, tail, non_tail);
            }
        }
        Expr::Access { record, .. } => {
            count_self_calls(self_id, arity, record, false, tail, non_tail);
        }
        // Task recursion excluded in v1: BOTH sub-terms non-tail (a Task-recursive
        // fn is simply not TCO'd = today's behaviour, no regression).
        Expr::TaskSeq { effect, rest } => {
            count_self_calls(self_id, arity, effect, false, tail, non_tail);
            count_self_calls(self_id, arity, rest, false, tail, non_tail);
        }
        // Leaves + a non-self `FuncValue` reference — no self-call to count.
        Expr::FuncValue { .. }
        | Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => {}
        // The TCO nodes are not yet produced when analysis runs (the rewrite is
        // the sole producer and runs AFTER analysis), but the walk stays explicit
        // and total: a `TailLoop` body is tail (`TailRecur` is merged into the
        // args-descent arm above).
        Expr::TailLoop { body, .. } => {
            count_self_calls(self_id, arity, body, in_tail, tail, non_tail);
        }
    }
}

/// Wrap a proven-tail-recursive body for loop emission. `analyze_tail_recursion`
/// MUST have returned `TailRecursive` first (no non-tail self-call survives), so
/// this cannot strand a self-`Call` outside the loop. Mirrors the reference's
/// `rewriteTailCalls`.
#[doc(hidden)]
#[must_use]
pub fn rewrite_tail_calls(
    self_id: FuncId,
    arity: usize,
    params: Vec<(Symbol, IrType)>,
    body: Expr,
) -> Expr {
    let rewritten = rewrite_in_tail(self_id, arity, body);
    Expr::TailLoop {
        params,
        body: Box::new(rewritten),
    }
}

/// Replace each qualifying tail self-call in tail position with `Expr::TailRecur`.
/// Only the tail propagators recurse in-tail; every non-tail form is returned
/// verbatim (the analysis proved no self-`Call` survives there, so nothing to
/// rewrite).
fn rewrite_in_tail(self_id: FuncId, arity: usize, expr: Expr) -> Expr {
    match expr {
        // The one transformation: a qualifying tail self-call becomes a jump.
        Expr::Call {
            callee: Callee::Func(id),
            args,
        } if id == self_id && args.len() == arity => Expr::TailRecur { args },
        Expr::If { cond, then_, else_ } => Expr::If {
            cond,
            then_: Box::new(rewrite_in_tail(self_id, arity, *then_)),
            else_: Box::new(rewrite_in_tail(self_id, arity, *else_)),
        },
        Expr::Match(m) => {
            // Map only the arm bodies in tail position; the scrutinee and every
            // pattern are preserved. A body-only remap keeps each arm's pattern,
            // so whichever structural-exhaustiveness condition the original
            // `Match` satisfied still holds → `new_flat` cannot fail here. On the
            // impossible error, fall back to the un-rewritten `Match` (sound:
            // ordinary recursion, never a stranded jump).
            let scrutinee = m.scrutinee().clone();
            let arms: Vec<Arm> = m
                .arms()
                .iter()
                .map(|arm| Arm {
                    pat: arm.pat.clone(),
                    body: rewrite_in_tail(self_id, arity, arm.body.clone()),
                })
                .collect();
            Match::new_flat(scrutinee, arms).map_or(Expr::Match(m), Expr::Match)
        }
        Expr::Let { name, value, body } => Expr::Let {
            name,
            value,
            body: Box::new(rewrite_in_tail(self_id, arity, *body)),
        },
        Expr::Destructure {
            binder,
            value,
            body,
        } => Expr::Destructure {
            binder,
            value,
            body: Box::new(rewrite_in_tail(self_id, arity, *body)),
        },
        // Every non-tail form (incl. non-jump Calls, Apply, Lambda, leaves,
        // TaskSeq) is returned verbatim.
        other => other,
    }
}

/// Test-only re-export of the crate-private TCO analysis/rewrite so the
/// integration-test binary (`tests/tail_analysis.rs`) can drive them directly.
#[doc(hidden)]
pub mod tco_analysis {
    // The re-exports are consumed only by the integration-test binary
    // (`tests/tail_analysis.rs`), which the in-crate unused-import lint cannot see.
    #[allow(unused_imports)]
    pub use super::{TailRecursion, analyze_tail_recursion, rewrite_tail_calls};
}

// ── M5b-db: Db-kernel presence detection ─────────────────────────────────────

/// Return `true` when `expr` (or any of its sub-expressions, recursively)
/// contains a call whose callee is one of the `Db*` kernel variants.
///
/// Used by [`Lowerer::run`] to decide whether the synthetic `SqlValue` /
/// `SqlField` `EnumDef`s must be injected into the module's type list.
fn expr_uses_db_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let callee_is_db = matches!(callee, Callee::Kernel(k) if kernel_is_db(*k));
            callee_is_db || args.iter().any(expr_uses_db_kernel)
        }
        // FuncValue reifies a callee as a first-class value (not a direct call),
        // but if that callee is a Db kernel it still implies Db usage.
        Expr::FuncValue { callee, .. } => {
            matches!(callee, Callee::Kernel(k) if kernel_is_db(*k))
        }
        Expr::Apply { func, args } => {
            expr_uses_db_kernel(func) || args.iter().any(expr_uses_db_kernel)
        }
        Expr::Let { value, body, .. } => expr_uses_db_kernel(value) || expr_uses_db_kernel(body),
        Expr::Destructure { value, body, .. } => {
            expr_uses_db_kernel(value) || expr_uses_db_kernel(body)
        }
        Expr::If { cond, then_, else_ } => {
            expr_uses_db_kernel(cond) || expr_uses_db_kernel(then_) || expr_uses_db_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_db_kernel(m.scrutinee())
                || m.arms().iter().any(|arm| expr_uses_db_kernel(&arm.body))
        }
        // `TailLoop` (a TCO'd body) recurses into its tail body exactly as the
        // pre-TCO body would, so kernel-presence detection is unchanged in meaning.
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_db_kernel(body),
        Expr::Cons { head, tail } => expr_uses_db_kernel(head) || expr_uses_db_kernel(tail),
        Expr::Tuple(elems) => elems.iter().any(expr_uses_db_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_db_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_db_kernel(v)),
        Expr::Access { record, .. } => expr_uses_db_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_db_kernel(record) || fields.iter().any(|(_, v)| expr_uses_db_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => expr_uses_db_kernel(lhs) || expr_uses_db_kernel(rhs),
        Expr::TaskSeq { effect, rest } => expr_uses_db_kernel(effect) || expr_uses_db_kernel(rest),
        // A `TailRecur` (a TCO jump) carries its next-iteration args like a `Ctor`.
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => args.iter().any(expr_uses_db_kernel),
        // Leaf expressions that cannot contain a kernel call.
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

/// Return `true` when `k` is one of the `Db*` kernel variants (including
/// `DbDec*`).
///
/// Delegates to [`KernelFn::is_db`], the single authoritative list maintained
/// in `sky_ir`.  Note that `matches!` always has an implicit `_ => false` arm,
/// so this function does NOT cause a compiler warning if a new `Db*` variant is
/// added without being listed — callers that need that guarantee should use the
/// result as a guard inside their own exhaustive `match`.
const fn kernel_is_db(k: KernelFn) -> bool {
    k.is_db()
}

// ── M5c: TEA kernel presence detection ───────────────────────────────────────

/// Return `true` when `expr` (or any of its sub-expressions, recursively)
/// contains a call whose callee is one of the TEA (`Cmd*` / `Sub*` /
/// `TimeEvery`) kernel variants introduced in M5c.
///
/// Used by [`Lowerer::run`] to decide whether the emitted project needs
/// `pub mod tea; pub use tea::*;` appended to `sky_runtime/mod.rs`.
fn expr_uses_tea_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let callee_is_tea = matches!(callee, Callee::Kernel(k) if kernel_is_tea(*k));
            callee_is_tea || args.iter().any(expr_uses_tea_kernel)
        }
        Expr::FuncValue { callee, .. } => {
            matches!(callee, Callee::Kernel(k) if kernel_is_tea(*k))
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            expr_uses_tea_kernel(value) || expr_uses_tea_kernel(body)
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_tea_kernel(body),
        Expr::If { cond, then_, else_ } => {
            expr_uses_tea_kernel(cond) || expr_uses_tea_kernel(then_) || expr_uses_tea_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_tea_kernel(m.scrutinee())
                || m.arms().iter().any(|arm| expr_uses_tea_kernel(&arm.body))
        }
        Expr::Tuple(elems) => elems.iter().any(expr_uses_tea_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_tea_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_tea_kernel(v)),
        Expr::Access { record, .. } => expr_uses_tea_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_tea_kernel(record) || fields.iter().any(|(_, v)| expr_uses_tea_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => expr_uses_tea_kernel(lhs) || expr_uses_tea_kernel(rhs),
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => args.iter().any(expr_uses_tea_kernel),
        Expr::Cons { head, tail } => expr_uses_tea_kernel(head) || expr_uses_tea_kernel(tail),
        Expr::Apply { func, args } => {
            expr_uses_tea_kernel(func) || args.iter().any(expr_uses_tea_kernel)
        }
        Expr::TaskSeq { effect, rest } => {
            expr_uses_tea_kernel(effect) || expr_uses_tea_kernel(rest)
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

/// Return `true` when `k` is one of the TEA kernel variants introduced in M5c
/// (including M6-reserved variants).
///
/// Delegates to [`KernelFn::is_tea`].
const fn kernel_is_tea(k: KernelFn) -> bool {
    k.is_tea()
}

// ── M6: Sky.Http.Server kernel presence detection ────────────────────────────

/// Return `true` when `expr` (or any of its sub-expressions, recursively)
/// contains a call whose callee is one of the `Sky.Http.Server` kernel
/// variants introduced in M6.
///
/// Used by [`Lowerer::run`] to decide whether the emitted project needs the
/// `server` feature in its `Cargo.toml` and the server module appended to
/// `sky_runtime/mod.rs`.
fn expr_uses_server_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let callee_is_server = matches!(callee, Callee::Kernel(k) if kernel_is_server(*k));
            callee_is_server || args.iter().any(expr_uses_server_kernel)
        }
        Expr::FuncValue { callee, .. } => {
            matches!(callee, Callee::Kernel(k) if kernel_is_server(*k))
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            expr_uses_server_kernel(value) || expr_uses_server_kernel(body)
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_server_kernel(body),
        Expr::If { cond, then_, else_ } => {
            expr_uses_server_kernel(cond)
                || expr_uses_server_kernel(then_)
                || expr_uses_server_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_server_kernel(m.scrutinee())
                || m.arms()
                    .iter()
                    .any(|arm| expr_uses_server_kernel(&arm.body))
        }
        Expr::Tuple(elems) => elems.iter().any(expr_uses_server_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_server_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_server_kernel(v)),
        Expr::Access { record, .. } => expr_uses_server_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_server_kernel(record)
                || fields.iter().any(|(_, v)| expr_uses_server_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => {
            expr_uses_server_kernel(lhs) || expr_uses_server_kernel(rhs)
        }
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => {
            args.iter().any(expr_uses_server_kernel)
        }
        Expr::Cons { head, tail } => expr_uses_server_kernel(head) || expr_uses_server_kernel(tail),
        Expr::Apply { func, args } => {
            expr_uses_server_kernel(func) || args.iter().any(expr_uses_server_kernel)
        }
        Expr::TaskSeq { effect, rest } => {
            expr_uses_server_kernel(effect) || expr_uses_server_kernel(rest)
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

/// Return `true` when `k` is one of the Sky.Http.Server kernel variants
/// introduced in M6.
///
/// Delegates to [`KernelFn::is_server`].
const fn kernel_is_server(k: KernelFn) -> bool {
    k.is_server()
}

// ── M7: Std.Ui / Std.Html / Std.Live / Std.Tui / Std.Webview detection ───────

/// Return `true` when `expr` (or any sub-expression) contains a call to a
/// Std.Ui / Std.Html render kernel introduced in M7.
fn expr_uses_ui_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let is_ui = matches!(callee, Callee::Kernel(k) if k.is_ui());
            is_ui || args.iter().any(expr_uses_ui_kernel)
        }
        Expr::FuncValue { callee, .. } => matches!(callee, Callee::Kernel(k) if k.is_ui()),
        Expr::Apply { func, args } => {
            expr_uses_ui_kernel(func) || args.iter().any(expr_uses_ui_kernel)
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            expr_uses_ui_kernel(value) || expr_uses_ui_kernel(body)
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_ui_kernel(body),
        Expr::If { cond, then_, else_ } => {
            expr_uses_ui_kernel(cond) || expr_uses_ui_kernel(then_) || expr_uses_ui_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_ui_kernel(m.scrutinee())
                || m.arms().iter().any(|arm| expr_uses_ui_kernel(&arm.body))
        }
        Expr::Tuple(elems) => elems.iter().any(expr_uses_ui_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_ui_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_ui_kernel(v)),
        Expr::Access { record, .. } => expr_uses_ui_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_ui_kernel(record) || fields.iter().any(|(_, v)| expr_uses_ui_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => expr_uses_ui_kernel(lhs) || expr_uses_ui_kernel(rhs),
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => args.iter().any(expr_uses_ui_kernel),
        Expr::Cons { head, tail } => expr_uses_ui_kernel(head) || expr_uses_ui_kernel(tail),
        Expr::TaskSeq { effect, rest } => expr_uses_ui_kernel(effect) || expr_uses_ui_kernel(rest),
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

/// Return `true` when `expr` (or any sub-expression) contains a call to a
/// `Sky.Core.CssSafety` leaf security kernel (the `Std.Css` backing, #47).
///
/// Mirrors [`expr_uses_ui_kernel`] exactly, delegating to
/// [`KernelFn::is_css`]. A pure `Std.Css` program (CSS + `println`, no
/// `Std.Ui` / `Std.Html`) never sets `uses_ui`, so the backend needs this
/// independent flag to declare `css_safety` / `css` in the emitted
/// `sky_runtime/mod.rs`.
fn expr_uses_css_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let is_css = matches!(callee, Callee::Kernel(k) if k.is_css());
            is_css || args.iter().any(expr_uses_css_kernel)
        }
        Expr::FuncValue { callee, .. } => matches!(callee, Callee::Kernel(k) if k.is_css()),
        Expr::Apply { func, args } => {
            expr_uses_css_kernel(func) || args.iter().any(expr_uses_css_kernel)
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            expr_uses_css_kernel(value) || expr_uses_css_kernel(body)
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_css_kernel(body),
        Expr::If { cond, then_, else_ } => {
            expr_uses_css_kernel(cond) || expr_uses_css_kernel(then_) || expr_uses_css_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_css_kernel(m.scrutinee())
                || m.arms().iter().any(|arm| expr_uses_css_kernel(&arm.body))
        }
        Expr::Tuple(elems) => elems.iter().any(expr_uses_css_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_css_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_css_kernel(v)),
        Expr::Access { record, .. } => expr_uses_css_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_css_kernel(record) || fields.iter().any(|(_, v)| expr_uses_css_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => expr_uses_css_kernel(lhs) || expr_uses_css_kernel(rhs),
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => {
            args.iter().any(expr_uses_css_kernel)
        }
        Expr::Cons { head, tail } => expr_uses_css_kernel(head) || expr_uses_css_kernel(tail),
        Expr::TaskSeq { effect, rest } => {
            expr_uses_css_kernel(effect) || expr_uses_css_kernel(rest)
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

/// Return `true` when `expr` (or any sub-expression) contains a call to a
/// Std.Live / Sky.Live app-entry kernel introduced in M7.
fn expr_uses_live_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let is_live = matches!(callee, Callee::Kernel(k) if k.is_live());
            is_live || args.iter().any(expr_uses_live_kernel)
        }
        Expr::FuncValue { callee, .. } => matches!(callee, Callee::Kernel(k) if k.is_live()),
        Expr::Apply { func, args } => {
            expr_uses_live_kernel(func) || args.iter().any(expr_uses_live_kernel)
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            expr_uses_live_kernel(value) || expr_uses_live_kernel(body)
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_live_kernel(body),
        Expr::If { cond, then_, else_ } => {
            expr_uses_live_kernel(cond)
                || expr_uses_live_kernel(then_)
                || expr_uses_live_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_live_kernel(m.scrutinee())
                || m.arms().iter().any(|arm| expr_uses_live_kernel(&arm.body))
        }
        Expr::Tuple(elems) => elems.iter().any(expr_uses_live_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_live_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_live_kernel(v)),
        Expr::Access { record, .. } => expr_uses_live_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_live_kernel(record) || fields.iter().any(|(_, v)| expr_uses_live_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => expr_uses_live_kernel(lhs) || expr_uses_live_kernel(rhs),
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => {
            args.iter().any(expr_uses_live_kernel)
        }
        Expr::Cons { head, tail } => expr_uses_live_kernel(head) || expr_uses_live_kernel(tail),
        Expr::TaskSeq { effect, rest } => {
            expr_uses_live_kernel(effect) || expr_uses_live_kernel(rest)
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

/// Return `true` when `expr` (or any sub-expression) contains a call to a
/// Std.Tui / Sky.Tui app-entry kernel introduced in M7.
fn expr_uses_tui_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let is_tui = matches!(callee, Callee::Kernel(k) if k.is_tui());
            is_tui || args.iter().any(expr_uses_tui_kernel)
        }
        Expr::FuncValue { callee, .. } => matches!(callee, Callee::Kernel(k) if k.is_tui()),
        Expr::Apply { func, args } => {
            expr_uses_tui_kernel(func) || args.iter().any(expr_uses_tui_kernel)
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            expr_uses_tui_kernel(value) || expr_uses_tui_kernel(body)
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_tui_kernel(body),
        Expr::If { cond, then_, else_ } => {
            expr_uses_tui_kernel(cond) || expr_uses_tui_kernel(then_) || expr_uses_tui_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_tui_kernel(m.scrutinee())
                || m.arms().iter().any(|arm| expr_uses_tui_kernel(&arm.body))
        }
        Expr::Tuple(elems) => elems.iter().any(expr_uses_tui_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_tui_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_tui_kernel(v)),
        Expr::Access { record, .. } => expr_uses_tui_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_tui_kernel(record) || fields.iter().any(|(_, v)| expr_uses_tui_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => expr_uses_tui_kernel(lhs) || expr_uses_tui_kernel(rhs),
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => args.iter().any(expr_uses_tui_kernel),
        Expr::Cons { head, tail } => expr_uses_tui_kernel(head) || expr_uses_tui_kernel(tail),
        Expr::TaskSeq { effect, rest } => {
            expr_uses_tui_kernel(effect) || expr_uses_tui_kernel(rest)
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

/// Return `true` when `expr` (or any sub-expression) contains a call to a
/// Std.Webview / Sky.Webview app-entry kernel introduced in M7.
fn expr_uses_webview_kernel(expr: &Expr) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            let is_webview = matches!(callee, Callee::Kernel(k) if k.is_webview());
            is_webview || args.iter().any(expr_uses_webview_kernel)
        }
        Expr::FuncValue { callee, .. } => matches!(callee, Callee::Kernel(k) if k.is_webview()),
        Expr::Apply { func, args } => {
            expr_uses_webview_kernel(func) || args.iter().any(expr_uses_webview_kernel)
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            expr_uses_webview_kernel(value) || expr_uses_webview_kernel(body)
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => expr_uses_webview_kernel(body),
        Expr::If { cond, then_, else_ } => {
            expr_uses_webview_kernel(cond)
                || expr_uses_webview_kernel(then_)
                || expr_uses_webview_kernel(else_)
        }
        Expr::Match(m) => {
            expr_uses_webview_kernel(m.scrutinee())
                || m.arms()
                    .iter()
                    .any(|arm| expr_uses_webview_kernel(&arm.body))
        }
        Expr::Tuple(elems) => elems.iter().any(expr_uses_webview_kernel),
        Expr::List { items, .. } => items.iter().any(expr_uses_webview_kernel),
        Expr::Record(fields) => fields.iter().any(|(_, v)| expr_uses_webview_kernel(v)),
        Expr::Access { record, .. } => expr_uses_webview_kernel(record),
        Expr::Update { record, fields } => {
            expr_uses_webview_kernel(record)
                || fields.iter().any(|(_, v)| expr_uses_webview_kernel(v))
        }
        Expr::BinOp { lhs, rhs, .. } => {
            expr_uses_webview_kernel(lhs) || expr_uses_webview_kernel(rhs)
        }
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => {
            args.iter().any(expr_uses_webview_kernel)
        }
        Expr::Cons { head, tail } => {
            expr_uses_webview_kernel(head) || expr_uses_webview_kernel(tail)
        }
        Expr::TaskSeq { effect, rest } => {
            expr_uses_webview_kernel(effect) || expr_uses_webview_kernel(rest)
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_) => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────

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
    /// Builtin constructor symbols — used by [`run`] to synthesise `SqlValue` /
    /// `SqlField` `EnumDef`s when the program uses any Db kernel.
    builtins: &'a BuiltinCtors,
    /// Each top-level binding's [`FuncId`], keyed by `(home_path, name)` so
    /// that same-named bindings from different source modules (e.g. `Lib.helper`
    /// and `Main.helper` both merged into the linked module) each get a distinct
    /// id. A `VarTopLevel { module, name }` reference resolves by looking up
    /// `(module.clone(), name)` — the module path it carries is the defining
    /// module's path, not the merged entry module's path.
    func_ids: BTreeMap<(Vec<Symbol>, Symbol), FuncId>,
    /// Each union's complete, in-declaration-order constructor set — the *true*
    /// variant set handed to [`Match::new`] — keyed by the type's nominal identity
    /// `(home, type name)`. Keyed by `(home, name)`, not `name` alone, so two
    /// modules each declaring `type Color` keep DISTINCT variant sets: a collapsed
    /// `Symbol`-only key would hand a `case` on one `Color` the other's ctor set,
    /// tripping the [`Match::new`] cover backstop (#100).
    enum_variants: BTreeMap<(ModPath, Symbol), Vec<Symbol>>,
    /// Each constructor's declared payload arity, keyed by its enum's nominal
    /// identity paired with the constructor name `(home, ctor name)`. A saturated
    /// construction passes exactly this many arguments; a bare or partially-applied
    /// payload constructor is the constructor-as-function gap. Keyed by
    /// `(home, ctor name)` so two same-short-named types whose constructors share a
    /// name but differ in arity do not collapse (#100).
    ctor_arity: BTreeMap<(ModPath, Symbol), usize>,
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
    ///
    /// Sized by [`count_destructure_param_sites`] (defs AND every lambda), the
    /// pool is handed out through [`Self::param_cursor`] as a GLOBALLY-unique
    /// supply — never positionally — so a def param and a lambda param inside its
    /// body can never be minted the same `arg_i`. Distinct-per-site binders make
    /// cross-nesting collision unrepresentable; the lowerer never relies on Rust
    /// shadowing.
    param_binders: Vec<Symbol>,
    /// Monotonic cursor into [`Self::param_binders`]. Each call to
    /// [`Self::fresh_param_binder`] returns the next distinct synthetic binder and
    /// advances; overrun fails closed as a [`bug`] (never an index panic). Interior
    /// mutability so the lowering walk stays over a shared `&self`.
    param_cursor: Cell<usize>,
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
///
/// Also carries the `SqlValue` / `SqlField` ADT symbols (M5b-db). These are not
/// user declarations either — they are synthesised by the lowerer into
/// `module.types` when any Db kernel call is detected, so the backend can emit
/// the concrete Rust enum and its `into_sql_param()` / `into_field_param()`
/// boundary conversions.
pub struct BuiltinCtors {
    pub maybe: Symbol,
    pub result: Symbol,
    pub just: Symbol,
    pub nothing: Symbol,
    pub ok: Symbol,
    pub err: Symbol,
    // ── SqlValue / SqlField (M5b-db) ─────────────────────────────────────────
    pub sqlvalue: Symbol,
    pub sqlfield: Symbol,
    pub sql_string: Symbol,
    pub sql_int: Symbol,
    pub sql_float: Symbol,
    pub sql_bool: Symbol,
    pub sql_bytes: Symbol,
    pub sql_time: Symbol,
    pub sql_decimal: Symbol,
    pub sql_money: Symbol,
    pub sql_null: Symbol,
    pub set_field: Symbol,
    pub omit_field: Symbol,
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

/// Count every **non-variable** parameter pattern across the whole module — both
/// function-def heads AND every (possibly nested) lambda. Each such site needs
/// one globally-unique synthetic `arg_N` binder (a `PVar` param reuses its own
/// name and needs none). This sizes the synthetic-binder pool so the monotonic
/// `Cell` cursor in [`Lowerer`] can hand out a distinct name per site: a def
/// param and a lambda param inside its body can never collide on `arg_i`, so the
/// lowerer never leans on Rust shadowing (make-invalid-states-unrepresentable).
///
/// Over-counting is harmless (a few unused interned symbols); under-counting
/// would let the cursor overrun, which fails closed as a [`bug`] — never an
/// index panic, never a silent reuse.
pub fn count_destructure_param_sites(m: &canon::Module) -> usize {
    fn non_var_params(pats: &[canon::Pattern]) -> usize {
        pats.iter()
            .filter(|p| !matches!(p.value, canon::Pattern_::PVar(_)))
            .count()
    }
    fn walk_expr(e: &canon::Expr) -> usize {
        match &e.value {
            canon::Expr_::Lambda(params, body) => {
                non_var_params(params) + walk_expr(body)
            }
            // Recurse into every sub-expression that can host a lambda.
            canon::Expr_::Call(callee, args) => {
                walk_expr(callee) + args.iter().map(walk_expr).sum::<usize>()
            }
            canon::Expr_::Binop { lhs, rhs, .. } => walk_expr(lhs) + walk_expr(rhs),
            canon::Expr_::Case(scrut, branches) => {
                walk_expr(scrut) + branches.iter().map(|b| walk_expr(&b.body)).sum::<usize>()
            }
            canon::Expr_::Let(bindings, body) => {
                bindings.iter().map(|b| walk_expr(&b.body)).sum::<usize>() + walk_expr(body)
            }
            canon::Expr_::If(branches, else_expr) => {
                branches
                    .iter()
                    .map(|(c, b)| walk_expr(c) + walk_expr(b))
                    .sum::<usize>()
                    + walk_expr(else_expr)
            }
            canon::Expr_::Tuple(elems) | canon::Expr_::List(elems) => {
                elems.iter().map(walk_expr).sum()
            }
            canon::Expr_::Cons(head, tail) => walk_expr(head) + walk_expr(tail),
            canon::Expr_::Record(fields) => fields.iter().map(|(_, v)| walk_expr(v)).sum(),
            canon::Expr_::Access(record, _) => walk_expr(record),
            canon::Expr_::Update(base, fields) => {
                walk_expr(base) + fields.iter().map(|(_, v)| walk_expr(v)).sum::<usize>()
            }
            // Leaves host no lambda.
            canon::Expr_::VarLocal(_)
            | canon::Expr_::VarTopLevel { .. }
            | canon::Expr_::VarKernel { .. }
            | canon::Expr_::VarCtor { .. }
            | canon::Expr_::Int(_)
            | canon::Expr_::Float(_)
            | canon::Expr_::Str(_)
            | canon::Expr_::Char(_)
            | canon::Expr_::Unit => 0,
        }
    }
    m.defs
        .iter()
        .map(|d| match d {
            canon::Def::Typed { patterns, body, .. }
            | canon::Def::Untyped { patterns, body, .. } => {
                non_var_params(patterns) + walk_expr(body)
            }
        })
        .sum()
}

impl<'a> Lowerer<'a> {
    pub fn new(
        m: &'a canon::Module,
        types: &'a SolvedTypes,
        interner: &'a Interner,
        eta_params: Vec<Symbol>,
        param_binders: Vec<Symbol>,
        builtins: &'a BuiltinCtors,
    ) -> Self {
        let mut func_ids = BTreeMap::new();
        for (idx, def) in m.defs.iter().enumerate() {
            let id = FuncId::from_raw(u32::try_from(idx).unwrap_or(u32::MAX));
            // Key by (home_path, name) so same-named defs from different source
            // modules get distinct ids after link::link merges them.
            func_ids.insert((def.home().to_vec(), def.name().value), id);
        }

        let mut enum_variants = BTreeMap::new();
        let mut ctor_arity = BTreeMap::new();
        for union in &m.unions {
            // Key by the union's HOME `(home, name)` so same-short-named types from
            // different source modules keep distinct variant/arity entries (#100).
            let uhome = ModPath(union.home.clone());
            enum_variants.insert(
                (uhome.clone(), union.name),
                union.ctors.iter().map(|c| c.name).collect(),
            );
            for ctor in &union.ctors {
                ctor_arity.insert((uhome.clone(), ctor.name), ctor.arity);
            }
        }
        // Seed the built-in `Maybe` / `Result` variant sets + payload arities so
        // a `case m of Just x -> … ; Nothing -> …` takes the same validated
        // `Match::new` enum-cover path a user enum does, and `Just x` / `Ok x`
        // lower as saturated constructions.
        // Prelude built-ins carry the empty canon home (`home: Vec::new()` in
        // `Env`), so they key the identity map under the empty `ModPath` — the
        // same home the lowered `Expr::Ctor` / `Pat::Ctor` for `Just` / `Ok` / …
        // carry (#100).
        let prelude_home = ModPath(Vec::new());
        enum_variants.insert(
            (prelude_home.clone(), builtins.maybe),
            vec![builtins.just, builtins.nothing],
        );
        enum_variants.insert(
            (prelude_home.clone(), builtins.result),
            vec![builtins.ok, builtins.err],
        );
        ctor_arity.insert((prelude_home.clone(), builtins.just), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.nothing), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ok), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.err), 1);

        // Seed `SqlValue` / `SqlField` variant sets + arities (M5b-db).
        // These are Prelude built-ins (like Maybe/Result) — no user `type`
        // declaration; the symbols must be present here so any `case v of
        // SqlString s -> … ; SqlInt i -> …` pattern is exhaustively validated and
        // constructor applications (e.g. `SqlInt 42`) lower as saturated.
        enum_variants.insert(
            (prelude_home.clone(), builtins.sqlvalue),
            vec![
                builtins.sql_string,
                builtins.sql_int,
                builtins.sql_float,
                builtins.sql_bool,
                builtins.sql_bytes,
                builtins.sql_time,
                builtins.sql_decimal,
                builtins.sql_money,
                builtins.sql_null,
            ],
        );
        enum_variants.insert(
            (prelude_home.clone(), builtins.sqlfield),
            vec![builtins.set_field, builtins.omit_field],
        );
        ctor_arity.insert((prelude_home.clone(), builtins.sql_string), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.sql_int), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.sql_float), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.sql_bool), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.sql_bytes), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.sql_time), 1);
        ctor_arity.insert((prelude_home.clone(), builtins.sql_decimal), 1); // SqlDecimal(String)
        ctor_arity.insert((prelude_home.clone(), builtins.sql_money), 1); // SqlMoney(String) — "ISO_CODE AMOUNT"
        ctor_arity.insert((prelude_home.clone(), builtins.sql_null), 1); // SqlNull(SqlValue)
        ctor_arity.insert((prelude_home.clone(), builtins.set_field), 1); // SetField(SqlValue)
        ctor_arity.insert((prelude_home, builtins.omit_field), 0);

        Self {
            m,
            types,
            interner,
            builtins,
            func_ids,
            enum_variants,
            ctor_arity,
            eta_params,
            param_binders,
            param_cursor: Cell::new(0),
        }
    }

    /// Hand out the next globally-unique synthetic parameter binder from
    /// [`Self::param_binders`], advancing the monotonic cursor. Fails closed as a
    /// [`bug`] if the pool is exhausted — the pool is sized by
    /// [`count_destructure_param_sites`] to cover every non-var param site in the
    /// module, so an overrun is an internal invariant violation, never a user
    /// error and never an index panic.
    fn fresh_param_binder(&self) -> DResult<Symbol> {
        let i = self.param_cursor.get();
        let sym = *self.param_binders.get(i).ok_or_else(|| {
            bug(
                "sky_lower::fresh_param_binder",
                "synthetic parameter-binder pool exhausted",
            )
        })?;
        self.param_cursor.set(i + 1);
        Ok(sym)
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
    #[allow(clippy::similar_names)] // `uses_ui` / `uses_tui` are intentionally similar
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

        // M5b-db: when any Db kernel call is present, inject the synthetic
        // `SqlValue` and `SqlField` `EnumDef`s into `module.types`.  They are
        // Prelude built-ins — not user `type` declarations — but the backend
        // needs real `EnumDef`s in the module to:
        //
        //   1. emit the Rust enum (so the generated code can construct
        //      `MainSqlValue::SqlInt(42)`);
        //   2. register them in `enum_names` + `variant_fields` inside
        //      `EmitCtx::build`, so `enum_name(sqlvalue_sym)` and
        //      `variant_fields(sqlvalue_sym, sql_int_sym)` resolve;
        //   3. detect db usage in `project::emit_program` so it can emit the
        //      db-enabled Cargo.toml, mod.rs, and the `into_sql_param` /
        //      `into_field_param` impl blocks.
        //
        // The injection is skipped when no Db kernel is used — a program with
        // no `import Std.Db` is not affected.
        if funcs.iter().any(|f| expr_uses_db_kernel(&f.body)) {
            types_ir.push(TypeDef::Enum(self.synthetic_sqlvalue_enum()));
            types_ir.push(TypeDef::Enum(self.synthetic_sqlfield_enum()));
        }

        let records = self.collect_record_types()?;

        // M5c: detect whether any TEA kernel call is present. The backend uses
        // this flag to append `pub mod tea; pub use tea::*;` to mod.rs and to
        // add `SkyCmd<M>` / `SkySub<M>` type aliases.
        let uses_tea = funcs.iter().any(|f| expr_uses_tea_kernel(&f.body));

        // M6: detect whether any Sky.Http.Server kernel call is present. The
        // backend uses this flag to inject the `server` feature in Cargo.toml
        // and append `pub mod server; pub use server::*; pub mod server_stream;
        // pub use server_stream::*;` to mod.rs.
        let uses_server = funcs.iter().any(|f| expr_uses_server_kernel(&f.body));

        // M7: detect Std.Ui / Std.Html / Std.Live / Std.Tui / Std.Webview usage.
        let (uses_ui, uses_live, uses_tui, uses_webview) = (
            funcs.iter().any(|f| expr_uses_ui_kernel(&f.body)),
            funcs.iter().any(|f| expr_uses_live_kernel(&f.body)),
            funcs.iter().any(|f| expr_uses_tui_kernel(&f.body)),
            funcs.iter().any(|f| expr_uses_webview_kernel(&f.body)),
        );

        // #47: detect Std.Css (Sky.Core.CssSafety) leaf-kernel usage. Independent
        // of `uses_ui` — a pure-Std.Css program uses no render kernel.
        let uses_css = funcs.iter().any(|f| expr_uses_css_kernel(&f.body));

        let module = Module {
            name: ModPath(self.m.name.clone()),
            types: types_ir,
            funcs,
            entry,
            records,
            uses_tea,
            uses_server,
            uses_ui,
            uses_live,
            uses_tui,
            uses_webview,
            uses_css,
        };
        Ok(Program {
            modules: vec![module],
        })
    }

    /// Synthesise the built-in `SqlValue` ADT as an [`EnumDef`].
    ///
    /// ```text
    /// type SqlValue
    ///     = SqlString String
    ///     | SqlInt Int
    ///     | SqlFloat Float
    ///     | SqlBool Bool
    ///     | SqlBytes Bytes
    ///     | SqlTime Int          -- Unix-millisecond timestamp
    ///     | SqlNull SqlValue     -- self-referential witness; backend boxes it
    /// ```
    ///
    /// Non-generic (no type parameters); the self-referential `SqlNull(SqlValue)`
    /// field is detected as cyclic by `EmitCtx::is_cyclic_self_field` and boxed
    /// at emission, exactly as user-defined recursive enums are.
    fn synthetic_sqlvalue_enum(&self) -> EnumDef {
        let b = self.builtins;
        // `SqlValue` is a Prelude built-in (not a user `type`): its constructors
        // carry the empty canon home, so its nominal identity uses the empty
        // `ModPath` everywhere (EnumDef / IrType::Enum / Expr::Ctor). The backend's
        // empty-home→entry-module naming fallback reproduces the pre-#100 Rust name
        // byte-for-byte.
        let sv = IrType::Enum {
            home: ModPath(Vec::new()),
            name: b.sqlvalue,
            args: Vec::new(),
        };
        EnumDef {
            name: b.sqlvalue,
            home: ModPath(Vec::new()),
            type_params: Vec::new(),
            variants: vec![
                Variant {
                    name: b.sql_string,
                    fields: vec![IrType::Str],
                },
                Variant {
                    name: b.sql_int,
                    fields: vec![IrType::Int],
                },
                Variant {
                    name: b.sql_float,
                    fields: vec![IrType::Float],
                },
                Variant {
                    name: b.sql_bool,
                    fields: vec![IrType::Bool],
                },
                Variant {
                    name: b.sql_bytes,
                    fields: vec![IrType::Bytes],
                },
                Variant {
                    name: b.sql_time,
                    fields: vec![IrType::Int],
                },
                // SqlDecimal and SqlMoney carry their value as a lossless String
                // representation — decimal digits for SqlDecimal,
                // "ISO_CODE AMOUNT" for SqlMoney.  Using IrType::Str is the
                // minimal wiring until a native IrType::Decimal is added.
                Variant {
                    name: b.sql_decimal,
                    fields: vec![IrType::Str],
                },
                Variant {
                    name: b.sql_money,
                    fields: vec![IrType::Str],
                },
                // SqlNull wraps a SqlValue (type witness, discarded by
                // `into_sql_param`).  The self-edge makes the enum recursive;
                // the backend boxes this field automatically.
                Variant {
                    name: b.sql_null,
                    fields: vec![sv],
                },
            ],
        }
    }

    /// Synthesise the built-in `SqlField` ADT as an [`EnumDef`].
    ///
    /// ```text
    /// type SqlField
    ///     = SetField SqlValue   -- SET this column to the given param value
    ///     | OmitField           -- omit this column from the generated SQL
    /// ```
    fn synthetic_sqlfield_enum(&self) -> EnumDef {
        let b = self.builtins;
        // `SqlField` / `SqlValue` are Prelude built-ins: empty canon home (see
        // [`Self::synthetic_sqlvalue_enum`]).
        let sv = IrType::Enum {
            home: ModPath(Vec::new()),
            name: b.sqlvalue,
            args: Vec::new(),
        };
        EnumDef {
            name: b.sqlfield,
            home: ModPath(Vec::new()),
            type_params: Vec::new(),
            variants: vec![
                Variant {
                    name: b.set_field,
                    fields: vec![sv],
                },
                Variant {
                    name: b.omit_field,
                    fields: Vec::new(),
                },
            ],
        }
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
            // Carry the union's DEFINING module (its home) so the backend derives
            // the emitted Rust enum name from the home, not the merged entry module
            // (#100): `Std.Palette.Shade` → `StdPaletteShade`, `Lib.Color` →
            // `LibColor`, `Main.Msg` → `MainMsg` (single-module unchanged).
            home: ModPath(u.home.clone()),
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
                    // G-b gate: skip records whose IR carries a function type.
                    // The `Live.app` cfg record has function-typed fields
                    // (init/update/view/subscriptions); emitting a Rust struct
                    // for it would need `Box<dyn Fn>` fields, which cannot
                    // derive `Clone`/`Debug`/`PartialEq`.  The cfg record is
                    // consumed structurally by `emit_live_app_inner` (never
                    // materialised as a runtime value), so its IR struct is
                    // not needed.
                    if !ir_contains_fun(&ir) && !out.contains(&ir) {
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
            .get(&(def.home().to_vec(), name))
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
                // TCO: if every self-call is a tail call, rewrite the body to a
                // loop so the Rust stack stays flat (mirrors Sky's TailCallOpt).
                // Self-recursion only, keyed on `FuncId`; Task-recursion excluded
                // (see `analyze_tail_recursion`). Guarded by `TailRecursive` so the
                // rewrite can never strand a self-`Call` outside the loop.
                let arity = params.len();
                if analyze_tail_recursion(id, arity, &lowered_body) == TailRecursion::TailRecursive
                {
                    lowered_body = rewrite_tail_calls(id, arity, params.clone(), lowered_body);
                }
                Ok(Func {
                    id,
                    name,
                    home: ModPath(def.home().to_vec()),
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
                let ret_ty = self
                    .types
                    .env
                    .get(&(def.home().to_vec(), name))
                    .ok_or_else(|| bug("sky_lower::lower_def", "no inferred type for binding"))?;
                let ret = self.ir_type_from_ty(ret_ty, sig_span)?;
                Ok(Func {
                    id,
                    name,
                    home: ModPath(def.home().to_vec()),
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
        // Stringify (`toString` / `Log.*With`) → Rust `SkyStringify`. Like `eq`,
        // it adds no `Copy` (a single stringify moves/borrows the value); the
        // multi-use case is the general Clone concern, not Stringify-specific.
        if b.has_show() {
            set = set.with_show();
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
            let ir_ty = self.ir_type_from_canon(arg, generics)?;
            // One shared path for every parameter shape (see `lower_param`): a
            // plain-var param contributes its name directly; a tuple / record /
            // alias / wildcard param takes a fresh synthetic binder and (for the
            // destructuring shapes) a `Destructure` prologue.
            let (param, maybe_prologue) = self.lower_param(pat, ir_ty)?;
            params.push(param);
            if let Some(p) = maybe_prologue {
                prologue.push(p);
            }
            cur = rest.as_ref();
        }
        // The trailing type is the return type.
        Ok((params, prologue, self.ir_type_from_canon(cur, generics)?))
    }

    /// Lower ONE binding-position parameter pattern (a function-def head param or
    /// a lambda param) into its IR parameter plus an optional destructure
    /// prologue. This is the single shared path for BOTH binding sites — one code
    /// path cannot disagree with itself about what a pattern param means
    /// (the design rejects upstream's asymmetric lambda-vs-def lowering).
    ///
    /// The SKY-T0015 gate (exhaustiveness phase, before lowering) has already
    /// proven `pat` irrefutable, so only irrefutable shapes are reachable:
    ///
    /// * `PVar(s)` — the param IS the name: `(s, ir_ty)`, no prologue, zero cost.
    /// * `PAnything` — a fresh unused binder, no prologue. `\_ ->` rides the
    ///   emitted crate's `#![allow(unused)]`, so no `let _ =` and no branch.
    /// * `PTuple` / `PRecord` / `PAlias` — a fresh binder plus a `Destructure`
    ///   prologue built by [`Self::lower_param_binder_pat`]; a record recovers its
    ///   COMPLETE field set from the param's SOLVED type (not a name heuristic).
    ///
    /// A refutable pattern is a fail-closed [`bug`] — it can no longer reach the
    /// lowerer (SKY-T0015 rejected it), so reaching this arm is an invariant
    /// violation, never a user error and never an emitted panic arm.
    fn lower_param(
        &self,
        pat: &canon::Pattern,
        ir_ty: IrType,
    ) -> DResult<(IrParam, Option<ParamPrologue>)> {
        match &pat.value {
            // The param is its own name — no synthetic binder, no prologue.
            canon::Pattern_::PVar(s) => Ok(((*s, ir_ty), None)),
            // A wildcard param needs a name (Rust params are named) but binds
            // nothing: a fresh unused binder, no destructure.
            canon::Pattern_::PAnything => Ok(((self.fresh_param_binder()?, ir_ty), None)),
            // A destructuring param: a fresh binder holds the whole argument, and
            // a `Destructure` prologue opens it in the body.
            canon::Pattern_::PTuple(_)
            | canon::Pattern_::PRecord(_)
            | canon::Pattern_::PAlias(_, _) => {
                let fresh = self.fresh_param_binder()?;
                let binder = self.lower_param_binder_pat(pat, pat.span)?;
                Ok(((fresh, ir_ty), Some((fresh, binder))))
            }
            // Refutable — rejected upstream by SKY-T0015. Fail closed.
            canon::Pattern_::PCtor { .. }
            | canon::Pattern_::PInt(_)
            | canon::Pattern_::PBool(_)
            | canon::Pattern_::PChar(_)
            | canon::Pattern_::PStr(_)
            | canon::Pattern_::PList(_)
            | canon::Pattern_::PCons(_, _) => Err(bug(
                "sky_lower::lower_param",
                "refutable parameter pattern reached the lowerer — the SKY-T0015 \
                 irrefutability gate should have rejected it",
            )),
        }
    }

    /// Like [`Self::lower_binder_pat`] but for a PARAMETER pattern, whose solved
    /// type lives at its own region span (recorded by the constraint generator on
    /// every param) rather than at a bound value expression. A record param
    /// recovers its COMPLETE field set from that solved type; an alias recurses on
    /// the SAME `param_span` (an alias does not change the scrutinee's type), so a
    /// nested record still recovers its full field set. Everything else
    /// (variable / wildcard / nested irrefutable tuple) lowers structurally.
    fn lower_param_binder_pat(&self, pat: &canon::Pattern, param_span: Span) -> DResult<Pat> {
        match &pat.value {
            canon::Pattern_::PRecord(fields) => {
                let ty = self.types.regions.get(&param_span).ok_or_else(|| {
                    bug(
                        "sky_lower::lower_param_binder_pat",
                        "record parameter has no solved region type",
                    )
                })?;
                self.lower_record_pat(fields, ty, pat.span)
            }
            canon::Pattern_::PAlias(inner, name) => Ok(Pat::Alias(
                Box::new(self.lower_param_binder_pat(inner, param_span)?),
                name.value,
            )),
            _ => Self::lower_destructure_pat(pat),
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
    #[allow(clippy::too_many_lines)] // declarative type-constructor dispatch — each builtin listed explicitly for safety
    fn ir_type_from_canon(&self, t: &canon::Type, generics: &[Symbol]) -> DResult<IrType> {
        match t {
            // A type-constructor application. A builtin (`Int`, `Bool`, …) carries
            // no args; a user enum carries its type arguments, each lowered under
            // the same generic scope so `Opt Int` → `Enum { Opt, [Int] }` and
            // `Opt a` (inside a generic signature) → `Enum { Opt, [Generic a] }`.
            canon::Type::Con { home, name, args } => match self.resolve(*name)? {
                "Int" => Ok(IrType::Int),
                "Float" => Ok(IrType::Float),
                "Bool" => Ok(IrType::Bool),
                // `Error` is Sky's fixed error-channel type, backed by `SkyError =
                // String` in the runtime.  Merged with `String` here since they
                // share the same IR representation (`IrType::Str`).
                "String" | "Error" => Ok(IrType::Str),
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
                // `Task Error a` — the canonical user annotation has two type
                // args: the error type (arg 0, always the implicit `Error`) and
                // the success type (arg 1). The IR discards the error type since
                // it is always `SkyError = String` at the Rust level.
                "Task" if args.len() == 2 => {
                    let inner =
                        self.ir_type_from_canon(args.get(1).ok_or_else(task_arg_bug)?, generics)?;
                    Ok(IrType::Task(Box::new(inner)))
                }
                // `Task a` — rare single-arg form (e.g. inside a user type alias
                // that already applied the error parameter).
                "Task" if args.len() == 1 => {
                    let inner =
                        self.ir_type_from_canon(args.first().ok_or_else(task_arg_bug)?, generics)?;
                    Ok(IrType::Task(Box::new(inner)))
                }
                "Task" => Err(bug(
                    "sky_lower::ir_type_from_canon",
                    format!(
                        "Task applied to {} type argument(s); expected 1 or 2",
                        args.len()
                    ),
                )),
                // `Decoder a` — the opaque JSON decoder type introduced by M4h.
                // Canonical annotations use it directly; maps to `IrType::Decoder`.
                "Decoder" if args.len() == 1 => {
                    let inner = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Decoder applied without its element type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Decoder(Box::new(inner)))
                }
                // `Db` — opaque connection pool handle introduced by M5b-db.
                "Db" => Ok(IrType::Db),
                // `Cmd msg` / `Sub msg` — TEA command and subscription types
                // introduced in M5c.  Users may write annotations like
                // `myCmd : Cmd Int`.
                "Cmd" if args.len() == 1 => {
                    let inner = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Cmd applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Cmd(Box::new(inner)))
                }
                "Sub" if args.len() == 1 => {
                    let inner = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Sub applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Sub(Box::new(inner)))
                }
                // ── M7: Std.Ui / Std.Html parametric type constructors ────────────
                // These are kernel types that carry a message type parameter `msg`.
                // They appear in user annotations like `staticView : Msg -> Html Msg`
                // and are lowered to `IrType::Ui { ctor, msg }` so the backend can
                // emit the correct Rust generic spelling (`Html<Msg>`, `Element<M>`,
                // etc.) and so BLOCKER-1's `emit_func` can extract the enclosing
                // function's `msg` type for the `ui_layout::<M>` turbofish.
                //
                // `Html msg` — the rendered HTML tree type from `Std.Html`.
                "Html" if args.len() == 1 => {
                    let msg = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Html applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::Html,
                        msg: Box::new(msg),
                    })
                }
                // `Element msg` — a Std.Ui layout element.
                "Element" if args.len() == 1 => {
                    let msg = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Element applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::Element,
                        msg: Box::new(msg),
                    })
                }
                _ if self
                    .enum_variants
                    .contains_key(&(ModPath(home.clone()), *name)) =>
                {
                    let mut ir_args = Vec::with_capacity(args.len());
                    for a in args {
                        ir_args.push(self.ir_type_from_canon(a, generics)?);
                    }
                    Ok(IrType::Enum {
                        home: ModPath(home.clone()),
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
        // Destructure prologues for the flattened params, in source (parameter)
        // order; folded around the body outermost-first below.
        let mut prologue: Vec<ParamPrologue> = Vec::new();
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
                let ir_ty = self.ir_type_from_ty(arg, pat.span)?;
                // Same shared path as the def-head params (see `lower_param`): a
                // plain-var param contributes its name; a destructuring param
                // takes a fresh binder + a `Destructure` prologue.
                let (param, maybe_prologue) = self.lower_param(pat, ir_ty)?;
                ir_params.push(param);
                if let Some(p) = maybe_prologue {
                    prologue.push(p);
                }
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
        let mut body = self.lower_expr(cur_body)?;
        // Fold each destructuring param's `Destructure` around the body,
        // OUTERMOST-first (reverse of source order) so the first parameter's
        // destructure is the outermost binding — identical to the def-head
        // prologue folding in `lower_def`. (Lambdas are not TCO'd, so there is no
        // TailLoop interaction here.)
        for (binder_sym, binder_pat) in prologue.into_iter().rev() {
            body = Expr::Destructure {
                binder: binder_pat,
                value: Box::new(Expr::Var(binder_sym)),
                body: Box::new(body),
            };
        }
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

    // The match has one arm per Sky builtin type — each arm adds ~5-10 lines;
    // pushing past clippy's 100-line ceiling is unavoidable without splitting on
    // an arbitrary boundary. The allow is narrow: only this function.
    #[allow(clippy::too_many_lines)]
    fn ir_type_from_ty(&self, t: &Ty, span: Span) -> DResult<IrType> {
        match t {
            Ty::Unit => Ok(IrType::Unit),
            // Reserved builtin names are matched first. This precedence is sound
            // because `sky_canon`'s `RESERVED_BUILTIN_TYPES` gate (resolve.rs,
            // SKY-N0026) rejects any user `type` / `type alias` whose name is one
            // of these builtin constructors, so those arms can never silently
            // override a user `type Int = …` / `type Html = …`.
            //
            // The nullary Std.Ui / Sky.Live opaque names (`Length` / `Color` /
            // `HAlign` / `VAlign` / `Location` / `PseudoClass` / `Description` /
            // `LayoutContext` / `LiveReq`) and `Value` are the exceptions:
            // #101 moved them BELOW the `enum_variants` guard so a program union
            // of the same name (a user ADT or a compiled-source `Std.Css` type)
            // wins by its `(home, name)` identity, and only a genuine opaque
            // builtin (no union entry) falls through to the `UiPlain` arm. This
            // matches `ir_type_from_canon`, so the inferred and annotated paths
            // agree. See RESERVED_BUILTIN_TYPES for the per-name cite list.
            Ty::Con { name, args, module } => match self.resolve(*name)? {
                "Int" => Ok(IrType::Int),
                "Float" => Ok(IrType::Float),
                "Bool" => Ok(IrType::Bool),
                // `Error` is Sky's fixed error-channel type, backed by `SkyError =
                // String` in the runtime.  Lambda parameters typed as `Error` (e.g.
                // the `e` in `\e -> ...` when `onError`/`mapError` pins the handler)
                // must lower to `IrType::Str`.  Merged with `String` since they share
                // the same IR representation.
                "String" | "Error" => Ok(IrType::Str),
                "Char" => Ok(IrType::Char),
                // `Bytes` is a built-in distinct primitive (Vec<u8> on Rust).
                // Divergence from Sky: Sky aliases Bytes = String.
                "Bytes" => Ok(IrType::Bytes),
                // M5a: all `Task a` shapes are now supported — `Task ()` →
                // `IrType::Task(Unit)`, `Task Int` → `IrType::Task(Int)`, etc.
                "Task" if args.len() == 1 => {
                    let inner = self.ir_type_from_ty(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Task applied without its type argument",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Task(Box::new(inner)))
                }
                "Task" => Err(bug(
                    "sky_lower::ir_type_from_ty",
                    "Task applied to wrong number of type arguments",
                )),
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
                // `Db` — the opaque connection pool handle introduced by M5b-db.
                // Zero type arguments; maps to `sky_runtime::Db`.
                "Db" => Ok(IrType::Db),
                // `Cmd msg` / `Sub msg` — the TEA command and subscription types
                // introduced in M5c.  Each carries exactly one type argument (the
                // message type `M`).  Maps to `sky_runtime::tea::SkyCmd<M>` /
                // `sky_runtime::tea::SkySub<M>`, aliased in the emitted preamble.
                "Cmd" if args.len() == 1 => {
                    let inner = self.ir_type_from_ty(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Cmd applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Cmd(Box::new(inner)))
                }
                "Sub" if args.len() == 1 => {
                    let inner = self.ir_type_from_ty(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Sub applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Sub(Box::new(inner)))
                }
                // `SqlValue` / `SqlField` — the builtin-injected ADT enums for
                // typed SQL parameters (M5b-db). Resolved as `IrType::Enum` so the
                // backend emits the generated `StdDbSqlValue` / `StdDbSqlField`
                // Rust enum name at use sites.
                "SqlValue" | "SqlField" => Ok(IrType::Enum {
                    // Prelude built-in: empty home, matching the synthetic EnumDef
                    // and the `Expr::Ctor` home (#100).
                    home: ModPath(Vec::new()),
                    name: *name,
                    args: Vec::new(),
                }),
                // M6 opaque server types — map directly to their dedicated
                // `IrType` variants so the backend emits the runtime names
                // (`ServerRequest`, `ServerResponse`, `ServerRoute`,
                // `ServerCookie`) without synthesising record structs.
                "Request" => Ok(IrType::ServerRequest),
                "Response" => Ok(IrType::ServerResponse),
                "Route" => Ok(IrType::ServerRoute),
                "Cookie" => Ok(IrType::ServerCookie),
                // ── M7: Std.Ui / Std.Html parametric type constructors ────────
                // Mirror of `ir_type_from_canon` (which handles user-written
                // type ANNOTATIONS).  This path handles SOLVED types from the
                // HM region map — `list_elem_ir` calls here when lowering a
                // `List (Attribute msg)` region, among others.
                //
                // Key differences from `ir_type_from_canon`:
                // 1. The msg arg is recursed through `ir_type_from_ty_ui_msg`
                //    (free `Ty::Var` → `IrType::Unit`, not `Json`, not error).
                // 2. `Attribute` is disambiguated by `Ty::Con.module` (T2 trap:
                //    BOTH `Std.Ui.Attribute` and `Std.Html.Attribute` exist —
                //    check the module path, never just the name).
                // 3. Plain Ui types (`Length`, `Color`, …) are nullary — no msg.
                // 4. `LiveReq` maps to `IrType::LiveReq` (opaque init arg).
                "Html" if args.len() == 1 => {
                    let msg = self.ir_type_from_ty_ui_msg(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Html applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::Html,
                        msg: Box::new(msg),
                    })
                }
                "Element" if args.len() == 1 => {
                    let msg = self.ir_type_from_ty_ui_msg(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Element applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::Element,
                        msg: Box::new(msg),
                    })
                }
                // T2 trap: `Attribute` exists in BOTH `Std.Ui` and `Std.Html`.
                // Disambiguate by `Ty::Con.module` — a module path containing
                // "Html" identifies the `Std.Html.Attribute` form.
                "Attribute" if args.len() == 1 => {
                    let msg = self.ir_type_from_ty_ui_msg(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Attribute applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    // A module path containing "Html" (e.g. ["Std","Html"] or
                    // ["Html"]) selects `HtmlAttribute`; everything else
                    // (["Std","Ui"], ["Ui"], or empty for builtin-injected)
                    // selects `UiAttribute`.  `any` short-circuits on first hit.
                    let is_html = module.iter().any(|s| self.resolve(*s).ok() == Some("Html"));
                    let ctor = if is_html {
                        UiCtor::HtmlAttribute
                    } else {
                        UiCtor::UiAttribute
                    };
                    Ok(IrType::Ui {
                        ctor,
                        msg: Box::new(msg),
                    })
                }
                "Event" if args.len() == 1 => {
                    let msg = self.ir_type_from_ty_ui_msg(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Event applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::HtmlEvent,
                        msg: Box::new(msg),
                    })
                }
                // ── Program-defined enum guard (home-aware; #100/#101) ────────
                // Checked BEFORE the bare-name Std.Ui / Sky.Live opaque arms
                // below, mirroring `ir_type_from_canon`'s ordering (the annotated
                // path already places its enum guard ahead of every non-reserved
                // name) so the inferred (ty) path and the annotated (canon) path
                // resolve the SAME `(home, name)` identically.
                //
                // A program-defined `type Color` — a user ADT OR a compiled-source
                // `Std.Css` type — is keyed in `enum_variants` under its real HOME
                // (#100), so it resolves to ITS OWN enum (`MainColor` /
                // `StdCssColor`) instead of being hijacked to the opaque
                // `UiPlain::Color`. A genuine Std.Ui builtin (`Length` / `Color` /
                // … that is NOT a program union — the real runtime `UiPlain`
                // types) has no `enum_variants` entry for any home, so the guard
                // fails and it falls through to the `UiPlain` arms below,
                // unchanged. This closes the #101 exit-0-then-cargo-fail hole (HOF
                // `applyTo _ Magenta` emitting a `UiPlain::Color` slot) and the
                // SKY-I0001 ty-vs-canon disagreement on `{ c : Color }` literals.
                _ if self
                    .enum_variants
                    .contains_key(&(ModPath(module.clone()), *name)) =>
                {
                    // A use-site enum type carries its solved type arguments, so
                    // `Opt Int` → `Enum { Opt, [Int] }` (rendered `MainOpt<i64>`).
                    // `module` is the type's HOME (the solver threads it on
                    // `Ty::Con`), which is the same identity the union was keyed
                    // under (#100).
                    let mut ir_args = Vec::with_capacity(args.len());
                    for a in args {
                        ir_args.push(self.ir_type_from_ty(a, span)?);
                    }
                    Ok(IrType::Enum {
                        home: ModPath(module.clone()),
                        name: *name,
                        args: ir_args,
                    })
                }
                // ── M7: Nullary Std.Ui plain types (no message parameter) ─────
                // Reached ONLY when `(home, name)` is NOT a program-defined enum
                // (guard above) — i.e. the genuine opaque Std.Ui builtin. A
                // program `type Color` / `type Length` never lands here.
                "Length" => Ok(IrType::UiPlain(UiPlain::Length)),
                "Color" => Ok(IrType::UiPlain(UiPlain::Color)),
                "HAlign" => Ok(IrType::UiPlain(UiPlain::HAlign)),
                "VAlign" => Ok(IrType::UiPlain(UiPlain::VAlign)),
                "Location" => Ok(IrType::UiPlain(UiPlain::Location)),
                "PseudoClass" => Ok(IrType::UiPlain(UiPlain::PseudoClass)),
                "Description" => Ok(IrType::UiPlain(UiPlain::Description)),
                "LayoutContext" => Ok(IrType::UiPlain(UiPlain::LayoutContext)),
                // ── M7: Sky.Live opaque types ─────────────────────────────────
                "LiveReq" => Ok(IrType::LiveReq),
                // The opaque JSON value type (`Value = any` in Sky). A concrete
                // `Con { name: "Value" }` reaches here only from the schemed
                // `JsonEnc.*` encoders (constrain's `json_value` builtin); it
                // maps to the same `IrType::Json` (`JsonVal`) that the free-var
                // JSON path (`ir_type_from_ty_json`) produces, so scheming
                // JsonEnc leaves the emitted Rust byte-identical while closing the
                // former `Ty::Var(u32::MAX)` exit-0 hole.
                //
                // Placed AFTER the `enum_variants` guard (like the nullary
                // `Length` / `Color` / … opaque arms, which #101 moved below the
                // guard): the built-in JSON `Value` is never a program union, so a
                // user-declared `type Value` still resolves as its own enum here.
                // The parametric reserved builtins (`Decoder` / `Cmd` / `Html` /
                // …) stay ABOVE the guard — they are name-reserved
                // (`RESERVED_BUILTIN_TYPES`, SKY-N0026) and so can never collide
                // with a program union.
                "Value" => Ok(IrType::Json),
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
    /// [`IrType::Unit`] instead of failing with `Feature::Polymorphism`.
    ///
    /// Used for the `msg` type parameter inside `Html msg` / `Element msg` /
    /// `Attribute msg` / `Event msg` when the solver left `msg` as a bare type
    /// variable (e.g. an empty attrs list `[]` whose element variable was never
    /// further constrained to a concrete message type).  Mapping the free var to
    /// `IrType::Unit` is sound because message-free subtrees carry no event
    /// handlers and Rust represents them as `Html<()>` / `Element<()>` etc.,
    /// which is byte-compatible with any monomorphisation at the call site via
    /// type inference.
    ///
    /// DISTINCT from [`ir_type_from_ty_json`] (`Ty::Var` → Json): the Json path
    /// is for `Value = any` kernel positions; this path is strictly for the
    /// `msg` slot of Ui parametric types.  Using Json here would emit
    /// `Html<JsonVal>` which conflicts with the typed callee's `Html<MainMsg>`.
    fn ir_type_from_ty_ui_msg(&self, t: &Ty, span: Span) -> DResult<IrType> {
        match t {
            // A free type variable in msg position is a message-free subtree.
            // Unit is the Rust conventional placeholder (`Html<()>`).
            Ty::Var(_) => Ok(IrType::Unit),
            // All other forms delegate to the strict helper — a concrete `Msg`
            // type becomes `IrType::Enum(Msg)`, `()` becomes `IrType::Unit`, etc.
            _ => self.ir_type_from_ty(t, span),
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
            && embeds_nonderivable_function(self.interner, ty)
        {
            let feature = if con_payload_carries_function(self.interner, ty) {
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
                home,
                type_name,
                name,
                ..
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
                let ctor_home = ModPath(home.clone());
                let arity = self.ctor_arity_of(&ctor_home, *name)?;
                if arity == 0 {
                    Ok(Expr::Ctor {
                        home: ctor_home,
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
                    // ── M5c TEA gate: `Cmd.none` / `Sub.none` carry an opaque
                    // `msg` type-parameter (`SkyCmd<M>` / `SkySub<M>`).  When the
                    // HM solver leaves `msg` as a free `Ty::Var` — the common
                    // shape in M5c since there is no update loop to anchor `msg`
                    // via a user `Msg` ADT — the emitted `cmd_none()` / `sub_none()`
                    // has an uninferrable `SkyCmd<_>` type that `cargo build`
                    // rejects with E0282.  Call `ir_type_from_ty` on the region
                    // type here; it naturally raises `Feature::Polymorphism`
                    // (SKY-L0102) when the `msg` argument is still a free var,
                    // failing closed at `skyc` rather than emitting invalid Rust.
                    // An anchored `msg` (inferred from a sibling `Cmd`/`Sub` in
                    // the same batch) succeeds and falls through to the standard
                    // arity-0 emit; Rust infers the concrete type from context.
                    //
                    // All other arity-0 kernels (e.g. `JsonEnc.null` whose return
                    // type is `Value = any = Ty::Var`) MUST keep the bypass: their
                    // `Ty::Var` is intentional (the JSON `any` slot), and calling
                    // `ir_type_from_ty` would raise a spurious `Polymorphism` error.
                    if matches!(
                        &callee,
                        Callee::Kernel(KernelFn::CmdNone | KernelFn::SubNone)
                    ) && let Some(ty) = self.types.regions.get(&e.span)
                    {
                        // Return value discarded — only the error path matters.
                        let _ = self.ir_type_from_ty(ty, e.span)?;
                    }
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

    /// Lower the `Live.app` cfg record literal, intentionally omitting the
    /// per-field [`Self::reject_function_valued_field`] gate (the L0107 exemption).
    ///
    /// Only a *direct* record literal in the single-argument position of a
    /// `KernelFn::LiveApp` call reaches here — the callee-peeked intercept in
    /// [`Self::lower_call`] enforces the exemption boundary.  A non-literal cfg
    /// (let-bound, piped, etc.) still goes through `lower_expr`, which fires
    /// [`Self::reject_function_through_type_var`] for function-embedding types —
    /// correct fail-closed behaviour.
    ///
    /// `lower_expr` IS called on each field *value*: it applies
    /// `reject_function_through_type_var`, which is correctly fail-closed for
    /// models that have function-typed fields (a `Model { fn : Int -> Int }`
    /// cannot be derived; the embedded function in the model's region type is
    /// detected and rejected before it would produce broken emit output).
    fn lower_app_cfg_record(&self, fields: &[(Symbol, canon::Expr)]) -> DResult<Expr> {
        let mut lowered: Vec<(Symbol, Expr)> = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            // Omit `reject_function_valued_field` — the L0107 exemption.
            lowered.push((*name, self.lower_expr(value)?));
        }
        lowered.sort_by(|a, b| {
            self.resolve(a.0)
                .unwrap_or("")
                .cmp(self.resolve(b.0).unwrap_or(""))
        });
        Ok(Expr::Record(lowered))
    }

    /// Lower the single cfg argument of an app-entry kernel, fail-closed on any
    /// non-literal shape.
    ///
    /// The Rust backend emits the runtime entry call by reading the cfg record's
    /// field expressions directly (see `emit_{live,tui,webview}_call`), so the cfg
    /// MUST be an inline `canon::Expr_::Record`. A let-bound / piped / call-result
    /// cfg has no literal fields to read and is rejected here with `SKY-L0119`
    /// ([`Feature::LetBoundAppCfg`]) at the argument's span — never allowed to
    /// reach emit, where it would fire a spanless `CompilerBug`.
    ///
    /// For `Webview.app`, the nested `window` field must itself be an inline
    /// record literal and its `size` field an inline 2-tuple literal (the G4 emit
    /// gates). Those are validated here on the canon fields (which carry spans) so
    /// a let-bound `window`/`size` gets `SKY-L0119` at the offending span, not an
    /// ICE.
    fn lower_app_entry_cfg(&self, peek: &Callee, arg0: &canon::Expr) -> DResult<Expr> {
        let canon::Expr_::Record(fields) = &arg0.value else {
            return Err(unsupported(arg0.span, Feature::LetBoundAppCfg));
        };
        if matches!(peek, Callee::Kernel(KernelFn::WebviewApp)) {
            self.reject_non_literal_webview_window(fields)?;
        }
        self.lower_app_cfg_record(fields)
    }

    /// Webview `window` must be an inline record and `window.size` an inline
    /// tuple. Checked on canon (spanned) fields; a present-but-non-literal shape
    /// is `SKY-L0119` at that value's span. A MISSING window/size is left
    /// untouched — the constrain scheme enforces the 5-field shape, so absence is
    /// a genuine compiler bug handled fail-closed by emit's field lookup.
    fn reject_non_literal_webview_window(&self, fields: &[(Symbol, canon::Expr)]) -> DResult<()> {
        for (name, value) in fields {
            if self.resolve(*name)? == "window" {
                let canon::Expr_::Record(win_fields) = &value.value else {
                    return Err(unsupported(value.span, Feature::LetBoundAppCfg));
                };
                for (wname, wvalue) in win_fields {
                    if self.resolve(*wname)? == "size"
                        && !matches!(&wvalue.value, canon::Expr_::Tuple(_))
                    {
                        return Err(unsupported(wvalue.span, Feature::LetBoundAppCfg));
                    }
                }
            }
        }
        Ok(())
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

        // ── Phase-1b: App-entry intercept ──────────────────────────────────
        // Intercept `Live.app` / `Live.appRouted` BEFORE the uniform arg
        // lowering below.  The `Live.app` cfg record carries function-typed
        // fields (`init`/`update`/`view`/`subscriptions`) that would trip
        // SKY-L0107 in the `Record` arm of `lower_expr`.
        //
        // `lower_callee` is a pure symbol-table lookup (no side effects); the
        // `VarKernel | VarTopLevel` arm re-calls it below for all other
        // callees — that second call is safe and deliberate (minimal diff).
        if let canon::Expr_::VarKernel { .. } | canon::Expr_::VarTopLevel { .. } = &callee.value {
            let peek = self.lower_callee(callee)?;
            match &peek {
                // ── Live.app cfg literal (L0107 exemption, Phase-1b) ────────────
                Callee::Kernel(KernelFn::LiveApp) if args.len() == 1 => {
                    // `args.len() == 1` is the match guard above; `first()` is
                    // always `Some` here.  Using `first()` instead of `args[0]`
                    // keeps `clippy::indexing_slicing` clean.
                    if let Some(arg0) = args.first() {
                        // Borrow `peek` for the gate BEFORE moving it into the
                        // returned `Expr::Call`.  A non-literal cfg (let-bound,
                        // piped, call-result) is rejected here with SKY-L0119
                        // rather than reaching emit's spanless `CompilerBug`.
                        let lowered_cfg = self.lower_app_entry_cfg(&peek, arg0)?;
                        return Ok(Expr::Call {
                            callee: peek,
                            args: vec![lowered_cfg],
                        });
                    }
                }
                // ── Tui.app / Tui.program / Webview.app cfg literal (L0107 exemption) ──
                //
                // Same pattern as `Live.app`: intercept the single cfg-record arg
                // BEFORE the uniform `lower_expr` path so function-typed fields
                // (init/update/view/subscriptions/onKey) do not trip SKY-L0107.
                // Phase-1c: TuiApp / TuiProgram.
                // Phase-1d: WebviewApp — the extra `window` field is a plain record
                //   value (no functions); `lower_app_entry_cfg` additionally
                //   requires that record — and its `size` tuple — to be inline
                //   literals (the G4 emit gates).
                // A non-literal cfg (let-bound, piped, etc.) is rejected here with
                // SKY-L0119 at the argument span — fail-closed, never an ICE.
                Callee::Kernel(KernelFn::TuiApp | KernelFn::TuiProgram | KernelFn::WebviewApp)
                    if args.len() == 1 =>
                {
                    if let Some(arg0) = args.first() {
                        // Borrow `peek` for the gate BEFORE moving it below.
                        let lowered_cfg = self.lower_app_entry_cfg(&peek, arg0)?;
                        return Ok(Expr::Call {
                            callee: peek,
                            args: vec![lowered_cfg],
                        });
                    }
                }
                Callee::Kernel(KernelFn::LiveAppRouted) => {
                    return Err(unsupported(call_span, Feature::RoutedLiveApp));
                }
                _ => {}
            }
            // Any other callee: fall through to the uniform path below.
            // `lower_callee` will be called again in the match arm — pure, safe.
        }

        let lowered_args = args
            .iter()
            .map(|a| self.lower_expr(a))
            .collect::<DResult<Vec<_>>>()?;
        match &callee.value {
            canon::Expr_::VarCtor {
                home,
                type_name,
                name,
                ..
            } => {
                // A constructor application. M3a lowers a *saturated* construction
                // to `Expr::Ctor`; a partial application (`Node l 1` for a
                // three-field `Node`) is a constructor-as-function value, which
                // awaits first-class-value support. Over-application is ruled out
                // by type-checking (applying past the fields makes the result a
                // non-function), so a non-equal count here is always partial.
                let ctor_home = ModPath(home.clone());
                let arity = self.ctor_arity_of(&ctor_home, *name)?;
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
                        home: ctor_home,
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
    #[allow(clippy::match_same_arms)] // UI arity blocks are separate for documentation clarity
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
                | KernelFn::JsonDecBool
                // ── TEA arity-0 (M5c) ─────────────────────────────────────────
                // `Cmd.none : Cmd msg`
                | KernelFn::CmdNone
                // `Sub.none : Sub msg`
                | KernelFn::SubNone
                // ── Error nullary constructors (#86) : `Error` ────────────────
                | KernelFn::ErrorTimeout
                | KernelFn::ErrorNotFound
                | KernelFn::ErrorPermissionDenied,
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
                | KernelFn::LogInfo
                | KernelFn::LogDebug
                | KernelFn::LogWarn
                | KernelFn::LogError
                | KernelFn::ListLength
                | KernelFn::ListHead
                | KernelFn::ListTail
                | KernelFn::ListReverse
                | KernelFn::ListConcat
                | KernelFn::ListIsEmpty
                | KernelFn::BasicsNot
                | KernelFn::BasicsToString
                | KernelFn::BasicsIdentity
                | KernelFn::BasicsFst
                | KernelFn::BasicsSnd
                | KernelFn::ResultOkDefault
                // ── Result/Maybe combine — arity 1 (#88) ─────────────────────
                | KernelFn::ResultCombine
                | KernelFn::MaybeCombine
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
                | KernelFn::CryptoRandomToken
                // ── Uuid arity-1 (M5b) ────────────────────────────────────────
                // `v4`/`v7` are `() -> Task Error String` (task #54): they take
                // the unit argument, exactly like `Time.now`. `parse` is the
                // pure `String -> Maybe String` parser.
                | KernelFn::UuidV4
                | KernelFn::UuidV7
                | KernelFn::UuidParse
                // ── Task combinators arity-1 (M5a) ────────────────────────────
                | KernelFn::TaskSucceed
                | KernelFn::TaskFail
                | KernelFn::TaskFromResult
                | KernelFn::TaskSequence
                | KernelFn::TaskParallel
                | KernelFn::TaskRun
                // ── Io arity-1 (M5a) ──────────────────────────────────────────
                | KernelFn::IoReadLine
                | KernelFn::IoWriteStdout
                | KernelFn::IoWriteStderr
                // ── Time arity-1 (M5a) ────────────────────────────────────────
                | KernelFn::TimeNow
                | KernelFn::TimeSleep
                | KernelFn::TimeUnixMillis
                // ── System arity-1 (M5a) ──────────────────────────────────────
                | KernelFn::SystemArgs
                | KernelFn::SystemGetenv
                | KernelFn::SystemGetArg
                | KernelFn::SystemGetenvInt
                | KernelFn::SystemGetenvBool
                | KernelFn::SystemUnsetenv
                | KernelFn::SystemCwd
                | KernelFn::SystemLoadEnv
                | KernelFn::SystemExit
                // ── Random arity-1 (M5a) ──────────────────────────────────────
                | KernelFn::RandomChoice
                // ── File arity-1 (M5a) ────────────────────────────────────────
                | KernelFn::FileReadFile
                | KernelFn::FileExists
                | KernelFn::FileRemove
                | KernelFn::FileMkdirAll
                | KernelFn::FileReadFileBytes
                | KernelFn::FileReadDir
                | KernelFn::FileIsDir
                | KernelFn::FileTempFile
                | KernelFn::FileTempDir
                | KernelFn::FileDelete
                // ── Http arity-1 (M5b) ────────────────────────────────────────
                // `HttpGet` : String -> Task Error HttpResponse
                // `HttpRequest` : HttpRequest -> Task Error HttpResponse
                // `HttpParseQuery` : String -> Dict String String (pure)
                // `HttpDefaultRequest` : String -> HttpRequest (pure builder)
                | KernelFn::HttpGet
                | KernelFn::HttpRequest
                | KernelFn::HttpParseQuery
                | KernelFn::HttpDefaultRequest
                // ── Db arity-1 (M5b-db) ───────────────────────────────────────
                // `DbConnect : () -> Task Error Db` — takes unit
                | KernelFn::DbConnect
                // `DbClose : Db -> Task Error ()` — takes the pool handle
                | KernelFn::DbClose
                // ── Db.Decode arity-1 (M5b-db) ────────────────────────────────
                // Primitive column decoders: `String -> Decoder T`
                | KernelFn::DbDecString
                | KernelFn::DbDecInt
                | KernelFn::DbDecFloat
                | KernelFn::DbDecBool
                // `nullable : Decoder a -> Decoder (Maybe a)`
                | KernelFn::DbDecNullable
                // `succeed : a -> Decoder a`
                | KernelFn::DbDecSucceed
                // `fail : String -> Decoder a`
                | KernelFn::DbDecFail
                // ── TEA arity-1 (M5c) ─────────────────────────────────────────
                // `Cmd.batch : List (Cmd msg) -> Cmd msg`
                | KernelFn::CmdBatch
                // `Sub.batch : List (Sub msg) -> Sub msg`
                | KernelFn::SubBatch
                // ── Server arity-1 (M6) ───────────────────────────────────────
                // `Server.text / json / html / redirect : String -> Response`
                | KernelFn::ServerText
                | KernelFn::ServerJson
                | KernelFn::ServerHtml
                | KernelFn::ServerRedirect
                // `Server.body / path / method : Request -> String`
                | KernelFn::ServerBody
                | KernelFn::ServerPath
                | KernelFn::ServerMethod
                // `Middleware.withLogging : Handler -> Handler`
                | KernelFn::MiddlewareWithLogging
                // ── Error message constructors (#86) : `String -> Error` ──────
                | KernelFn::ErrorUnexpected
                | KernelFn::ErrorInvalidInput
                | KernelFn::ErrorIo
                | KernelFn::ErrorNetwork
                | KernelFn::ErrorFfi
                | KernelFn::ErrorDecode
                | KernelFn::ErrorConflict
                | KernelFn::ErrorUnavailable
                // `Error.toString : Error -> String`
                | KernelFn::ErrorToString
                // ── CssSafety arity-1 (Std.Css leaf kernels, #47) ─────────────
                // `safeValue`/`safePropName`/`safeSelector : String -> Maybe String`
                // `stripStyleClose : String -> String`
                | KernelFn::CssSafetySafeValue
                | KernelFn::CssSafetySafePropName
                | KernelFn::CssSafetySafeSelector
                | KernelFn::CssSafetyStripStyleClose,
            ) => Ok(1),
            Callee::Kernel(
                KernelFn::StringAppend
                | KernelFn::StringContains
                | KernelFn::StringStartsWith
                | KernelFn::StringEndsWith
                | KernelFn::StringContainsIn
                | KernelFn::StringStartsWithIn
                | KernelFn::StringEndsWithIn
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
                | KernelFn::ListAppend
                | KernelFn::ListTake
                | KernelFn::ListDrop
                | KernelFn::ListZip
                | KernelFn::ListCons
                | KernelFn::ListConcatMap
                | KernelFn::ListIndexedMap
                | KernelFn::ListAny
                | KernelFn::ListAll
                | KernelFn::ListFind
                // ── List batch (#119) ────────────────────────────────────────
                | KernelFn::ListFilterMap
                | KernelFn::ListSortBy
                | KernelFn::BasicsAlways
                | KernelFn::BasicsModBy
                | KernelFn::LogInfoWith
                | KernelFn::LogDebugWith
                | KernelFn::LogWarnWith
                | KernelFn::LogErrorWith
                | KernelFn::MaybeWithDefault
                | KernelFn::MaybeMap
                | KernelFn::MaybeAndThen
                | KernelFn::ResultWithDefault
                | KernelFn::ResultMap
                | KernelFn::ResultAndThen
                | KernelFn::ResultMapError
                // ── Result/Maybe andMap + Result.traverse — arity 2 (#88) ────
                | KernelFn::ResultAndMap
                | KernelFn::ResultTraverse
                | KernelFn::MaybeAndMap
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
                | KernelFn::CryptoChachaKeyFromPassword
                // ── Jwt arity-2 (M5b) ─────────────────────────────────────────
                | KernelFn::JwtEncodeHs256
                | KernelFn::JwtDecodeHs256
                | KernelFn::JwtEncodeRs256
                | KernelFn::JwtDecodeRs256
                // ── Task combinators arity-2 (M5a) ────────────────────────────
                | KernelFn::TaskMap
                | KernelFn::TaskAndThen
                | KernelFn::TaskMapError
                | KernelFn::TaskOnError
                | KernelFn::TaskAndThenResult
                // ── System arity-2 (M5a) ──────────────────────────────────────
                | KernelFn::SystemGetenvOr
                | KernelFn::SystemSetenv
                // ── Random arity-2 (M5a) ──────────────────────────────────────
                | KernelFn::RandomInt
                | KernelFn::RandomFloat
                // ── File arity-2 (M5a) ────────────────────────────────────────
                | KernelFn::FileWriteFile
                | KernelFn::FileReadFileLimit
                | KernelFn::FileAppend
                | KernelFn::FileCopy
                | KernelFn::FileRename
                // ── Http arity-2 (M5b) ────────────────────────────────────────
                // `HttpPost` : String -> String -> Task Error HttpResponse
                // `HttpWithMethod` / `HttpWithTimeout` / `HttpWithBody` : pure builders
                | KernelFn::HttpPost
                | KernelFn::HttpWithMethod
                | KernelFn::HttpWithTimeout
                | KernelFn::HttpWithBody
                // ── Db arity-2 (M5b-db) ───────────────────────────────────────
                // `DbOpen : String -> String -> Task Error Db`
                | KernelFn::DbOpen
                // `DbExecRaw : Db -> String -> Task Error Int`
                | KernelFn::DbExecRaw
                // pure row helpers: `String -> Dict String String -> T`
                | KernelFn::DbGetString
                | KernelFn::DbGetInt
                | KernelFn::DbGetBool
                | KernelFn::DbGetField
                // `DbWithTransaction : Db -> (Db -> Task Error a) -> Task Error a`
                | KernelFn::DbWithTransaction
                // `DbMigrate : Db -> List (String, String) -> Task Error (List String)`
                | KernelFn::DbMigrate
                // ── Db.Decode arity-2 (M5b-db) ────────────────────────────────
                // `map : (a -> b) -> Decoder a -> Decoder b`
                | KernelFn::DbDecMap
                // `andThen : (a -> Decoder b) -> Decoder a -> Decoder b`
                | KernelFn::DbDecAndThen
                // ── TEA arity-2 (M5c wired) ───────────────────────────────────
                // `Cmd.perform : Task Error a -> (Result Error a -> msg) -> Cmd msg`
                | KernelFn::CmdPerform
                // `Sub.every : Int -> msg -> Sub msg`
                | KernelFn::SubEvery
                // `Time.every : Int -> msg -> Sub msg`  (alias)
                | KernelFn::TimeEvery
                // ── TEA arity-2 (M6 reserved — not emitted yet) ───────────────
                // `Cmd.publish : String -> a -> Cmd msg`
                | KernelFn::CmdPublish
                // `Cmd.publishNoEcho : String -> a -> Cmd msg`
                | KernelFn::CmdPublishNoEcho
                // `Sub.subscribeTopic : String -> (a -> msg) -> Sub msg`
                | KernelFn::SubSubscribeTopic
                // `PubSub.publish : String -> a -> Task Error ()`
                | KernelFn::PubSubPublish
                // `PubSub.publishNoEcho : String -> a -> Task Error ()`
                | KernelFn::PubSubPublishNoEcho
                // ── Server arity-2 (M6) ───────────────────────────────────────
                // `Server.get/post/put/delete/any/api : String -> Handler -> Route`
                | KernelFn::ServerGet
                | KernelFn::ServerPost
                | KernelFn::ServerPut
                | KernelFn::ServerDelete
                | KernelFn::ServerAny
                | KernelFn::ServerApi
                // `Server.static : String -> String -> Route`
                | KernelFn::ServerStatic
                // `Server.listen : Int -> List Route -> Task Error ()`
                | KernelFn::ServerListen
                // `Server.withStatus : Int -> Response -> Response`
                | KernelFn::ServerWithStatus
                // `Server.param/queryParam/header/getCookie : String -> Request -> Maybe String`
                | KernelFn::ServerParam
                | KernelFn::ServerQueryParam
                | KernelFn::ServerHeader
                | KernelFn::ServerGetCookie
                // `Server.cookie : String -> String -> Cookie`
                | KernelFn::ServerCookieNew
                // `Server.withCookie : Cookie -> Response -> Response`
                | KernelFn::ServerWithCookie
                // `Middleware.withCors : List String -> Handler -> Handler`
                | KernelFn::MiddlewareWithCors
                // `Error.withMessage : String -> Error -> Error` (#86)
                | KernelFn::ErrorWithMessage,
            ) => Ok(2),
            Callee::Kernel(
                KernelFn::StringReplace
                | KernelFn::StringSlice
                | KernelFn::StringPadLeft
                | KernelFn::StringPadRight
                | KernelFn::BasicsClamp
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
                // ── Result/Maybe map2 — arity 3 (#88) ────────────────────────
                | KernelFn::ResultMap2
                | KernelFn::MaybeMap2
                // ── Crypto arity-3 (M5a) ─────────────────────────────────────
                | KernelFn::CryptoRsaSha256Verify
                // ── Http arity-3 (M5b) ───────────────────────────────────────
                // `HttpWithHeader` : String -> String -> HttpRequest -> HttpRequest
                | KernelFn::HttpWithHeader
                // ── Db arity-3 (M5b-db) ───────────────────────────────────────
                // `DbExec : Db -> String -> List SqlValue -> Task Error Int`
                | KernelFn::DbExec
                // `DbQuery : Db -> String -> List SqlValue -> Task Error (List Row)`
                | KernelFn::DbQuery
                // `DbInsertRow : Db -> String -> List (String, String) -> Task Error Int`
                | KernelFn::DbInsertRow
                // `DbGetById : Db -> String -> String -> Task Error (Maybe Row)`
                | KernelFn::DbGetById
                // `DbDeleteById : Db -> String -> String -> Task Error Int`
                | KernelFn::DbDeleteById
                // `DbFindByConditions : Db -> String -> Dict String String -> Task Error (List Row)`. Arity 3.
                | KernelFn::DbFindByConditions
                // `DbInsertFields : Db -> String -> List (String, SqlField) -> Task Error Int`
                | KernelFn::DbInsertFields
                // ── Db.Decode arity-3 (M5b-db) ────────────────────────────────
                // `map2 : (a -> b -> c) -> Decoder a -> Decoder b -> Decoder c`
                | KernelFn::DbDecMap2
                // `required : String -> Decoder a -> Decoder (a -> b) -> Decoder b`
                | KernelFn::DbDecRequired
                // ── Server arity-3 (M6) ───────────────────────────────────────
                // `Server.withHeader : String -> String -> Response -> Response`
                | KernelFn::ServerWithHeader
                // `Middleware.withBasicAuth : String -> String -> Handler -> Handler`
                | KernelFn::MiddlewareWithBasicAuth,
            ) => Ok(3),
            // ── JsonDec arity-4 (M4h) ─────────────────────────────────────────
            Callee::Kernel(
                KernelFn::JsonDecMap3
                // ── Result/Maybe map3 — arity 4 (#88) ────────────────────────
                | KernelFn::ResultMap3
                | KernelFn::MaybeMap3
                | KernelFn::JsonDecPOptional
                // ── Db arity-4 (M5b-db) ───────────────────────────────────────
                // `DbQueryDecode : Db -> String -> List SqlValue -> Decoder a -> Task Error (List a)`
                | KernelFn::DbQueryDecode
                // `DbUpdateById : Db -> String -> String -> List (String, String) -> Task Error Int`
                | KernelFn::DbUpdateById
                // `DbFindOneByField : Db -> String -> String -> String -> Task Error (Maybe Row)`
                | KernelFn::DbFindOneByField
                // `DbFindManyByField : Db -> String -> String -> String -> Task Error (List Row)`
                | KernelFn::DbFindManyByField
                // `DbUnsafeFindWhere : Db -> String -> String -> List String -> Task Error (List Row)`. Arity 4.
                // The List String provides parameterized SQL bindings (? placeholders) — injection-safe.
                | KernelFn::DbUnsafeFindWhere
                // `DbUpdateFields : Db -> String -> List (String, SqlValue) -> List (String, SqlField) -> Task Error Int`
                | KernelFn::DbUpdateFields
                // ── Db.Decode arity-4 (M5b-db) ────────────────────────────────
                // `map3 : (a->b->c->d) -> Decoder a -> Decoder b -> Decoder c -> Decoder d`
                | KernelFn::DbDecMap3
                // `optional : String -> Decoder a -> a -> Decoder (a->b) -> Decoder b`
                | KernelFn::DbDecOptional
                // ── Server arity-4 (M6) ───────────────────────────────────────
                // `Middleware.withRateLimit : String -> Int -> Int -> Handler -> Handler`
                | KernelFn::MiddlewareWithRateLimit
                // `RateLimit.allow : String -> String -> Int -> Int -> Bool`
                | KernelFn::RateLimitAllow,
            ) => Ok(4),
            // ── JsonDec arity-5 (M4h) ─────────────────────────────────────────
            Callee::Kernel(
                KernelFn::JsonDecMap4
                // ── Db arity-5 (M5b-db) ───────────────────────────────────────
                // `DbInsertFieldsReturning : Db -> String -> List (String, SqlField) -> String -> Decoder a -> Task Error (List a)`
                | KernelFn::DbInsertFieldsReturning
                // `map4 : (a->b->c->d->e) -> Da -> Db -> Dc -> Dd -> De`
                | KernelFn::DbDecMap4
                // ── Result/Maybe map4 — arity 5 (#88) ────────────────────────
                | KernelFn::ResultMap4
                | KernelFn::MaybeMap4,
            ) => Ok(5),
            // ── Result/Maybe map5 — arity 6 (#88) ────────────────────────────
            Callee::Kernel(KernelFn::ResultMap5 | KernelFn::MaybeMap5) => Ok(6),
            // ── M7: Std.Ui / Std.Html render kernels ─────────────────────────
            // Arity 0: nullary constants — no arguments.
            Callee::Kernel(
                // `Ui.none : Element msg`
                KernelFn::UiNone
                // `Ui.fill : Length`
                | KernelFn::UiFill
                // `Ui.content : Length`
                | KernelFn::UiContent
                // `Ui.shrink : Length`
                | KernelFn::UiShrink
                // `Ui.white : Color`
                | KernelFn::UiWhite
                // `Ui.black : Color`
                | KernelFn::UiBlack
                // `Ui.transparent : Color`
                | KernelFn::UiTransparent
                // `Ui.centerX : Attribute msg`
                | KernelFn::UiCenterX
                // `Ui.centerY : Attribute msg`
                | KernelFn::UiCenterY
                // `Ui.alignLeft : Attribute msg`
                | KernelFn::UiAlignLeft
                // `Ui.alignRight : Attribute msg`
                | KernelFn::UiAlignRight
                // `Ui.alignTop : Attribute msg`
                | KernelFn::UiAlignTop
                // `Ui.alignBottom : Attribute msg`
                | KernelFn::UiAlignBottom
                // `Ui.pointer : Attribute msg`
                | KernelFn::UiPointer
                // `Ui.clip : Attribute msg`
                | KernelFn::UiClip
                // `Ui.scrollbars : Attribute msg`
                | KernelFn::UiScrollbars
                // `Font.bold : Attribute msg`
                | KernelFn::FontBold
                // `Font.italic : Attribute msg`
                | KernelFn::FontItalic
                // `Attr.noAttr : Attribute msg` (#76)
                | KernelFn::HtmlNoAttr
                // ── #76 Tier 1 — nullary attrs ────────────────────────────────
                | KernelFn::UiSquare
                | KernelFn::UiWidescreen
                | KernelFn::BorderSolid
                | KernelFn::BorderDashed
                | KernelFn::BorderDotted
                | KernelFn::FontSemiBold
                | KernelFn::FontRegular
                | KernelFn::FontLight
                | KernelFn::FontExtraBold
                | KernelFn::FontBlack
                | KernelFn::FontUnderline
                | KernelFn::FontNoDecoration
                | KernelFn::FontAlignLeft
                | KernelFn::FontAlignRight
                | KernelFn::FontCenter
                | KernelFn::FontJustify
                // Font string constants (nullary, return String)
                | KernelFn::FontSansSerif
                | KernelFn::FontSerif
                | KernelFn::FontMonospace,
            ) => Ok(0),
            // Arity 1: single-argument pure serialisation / escape helpers.
            Callee::Kernel(
                // `Html.render : Html msg -> String`
                KernelFn::HtmlRender
                // `Html.escapeText : String -> String`
                | KernelFn::HtmlEscapeText
                // `Html.escapeAttr : String -> String`
                | KernelFn::HtmlEscapeAttr
                // `Html.attrToString : Attribute msg -> String`
                | KernelFn::HtmlAttrToString
                // ── M7: Ui element builders — arity 1 ────────────────────────
                // `Ui.text : String -> Element msg`
                | KernelFn::UiText
                // `Ui.html : Html msg -> Element msg`
                | KernelFn::UiHtml
                // ── M7: Ui attribute builders — arity 1 ──────────────────────
                // `Ui.spacing : Int -> Attribute msg`
                | KernelFn::UiSpacing
                // `Ui.padding : Int -> Attribute msg`
                | KernelFn::UiPadding
                // `Ui.width : Length -> Attribute msg`
                | KernelFn::UiWidth
                // `Ui.height : Length -> Attribute msg`
                | KernelFn::UiHeight
                // `Ui.gridColumns : Int -> Attribute msg`
                | KernelFn::UiGridColumns
                // ── M7: Ui Length builders — arity 1 ─────────────────────────
                // `Ui.px : Int -> Length`
                | KernelFn::UiPx
                // `Ui.fillPortion : Int -> Length`
                | KernelFn::UiFillPortion
                // `Ui.vh : Int -> Length`
                | KernelFn::UiVh
                // `Ui.vw : Int -> Length`
                | KernelFn::UiVw
                // ── M7: Background — arity 1 ─────────────────────────────────
                // `Background.color : Color -> Attribute msg`
                | KernelFn::BackgroundColor
                // `Background.image : String -> Attribute msg`
                | KernelFn::BackgroundImage
                // ── M7: Border — arity 1 ─────────────────────────────────────
                // `Border.width : Int -> Attribute msg`
                | KernelFn::BorderWidth
                // `Border.rounded : Int -> Attribute msg`
                | KernelFn::BorderRounded
                // `Border.color : Color -> Attribute msg`
                | KernelFn::BorderColor
                // ── M7: Font — arity 1 ───────────────────────────────────────
                // `Font.size : Int -> Attribute msg`
                | KernelFn::FontSize
                // `Font.color : Color -> Attribute msg`
                | KernelFn::FontColor
                // `Font.family : List String -> Attribute msg`
                | KernelFn::FontFamily
                // ── M7: Html element builders — arity 1 ──────────────────────
                // `Html.text : String -> Html msg`
                | KernelFn::HtmlTextNode
                // `Html.raw : String -> Html msg`
                | KernelFn::HtmlRawNode
                // `Html.input : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlInput
                // `Html.img : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlImg
                // ── #76 batch 2: Std.Html void element builders — arity 1 ────
                // `Html.br : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlBr
                // `Html.hr : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlHr
                // `Html.meta : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlMeta
                // `Html.link : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlLink
                // `Html.area : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlArea
                // `Html.base : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlBase
                // `Html.col : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlCol
                // `Html.embed : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlEmbed
                // `Html.source : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlSource
                // `Html.track : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlTrack
                // `Html.wbr : List (Attribute msg) -> Html msg` (void element)
                | KernelFn::HtmlWbr
                // ── M7: Phase-1a event-attribute builders — arity 1 ──────────
                // `Ui.onClick : msg -> Attribute msg`
                | KernelFn::UiOnClick
                // `Ui.onFocus : msg -> Attribute msg`
                | KernelFn::UiOnFocus
                // `Ui.onBlur : msg -> Attribute msg`
                | KernelFn::UiOnBlur
                // `Ui.onMouseOver : msg -> Attribute msg`
                | KernelFn::UiOnMouseOver
                // `Ui.onMouseOut : msg -> Attribute msg`
                | KernelFn::UiOnMouseOut
                // `Ui.onInput : (String -> msg) -> Attribute msg`
                | KernelFn::UiOnInput
                // `Ui.onChange : (String -> msg) -> Attribute msg`
                | KernelFn::UiOnChange
                // `Ui.onKeyDown : (String -> msg) -> Attribute msg`
                | KernelFn::UiOnKeyDown
                // `Ui.onKeyUp : (String -> msg) -> Attribute msg`
                | KernelFn::UiOnKeyUp
                // `Event.onBool : (Bool -> msg) -> Attribute msg`
                | KernelFn::UiOnBool
                // ── #107: Std.Html.Events builders — arity 1 (all shapes) ────
                | KernelFn::HtmlOnClick
                | KernelFn::HtmlOnFocus
                | KernelFn::HtmlOnBlur
                | KernelFn::HtmlOnMouseOver
                | KernelFn::HtmlOnMouseOut
                | KernelFn::HtmlOnSubmit
                | KernelFn::HtmlOnInput
                | KernelFn::HtmlOnChange
                | KernelFn::HtmlOnKeyDown
                | KernelFn::HtmlOnKeyUp
                | KernelFn::HtmlOnBool
                // ── M7: app-entry stubs — arity 1 ────────────────────────────
                // `Live.app : LiveAppCfg model msg -> Task Error ()`
                | KernelFn::LiveApp
                // `Live.appRouted : LiveAppCfg model msg -> Task Error ()`
                | KernelFn::LiveAppRouted
                // `Tui.program : TuiCfg model msg -> Task Error ()`
                | KernelFn::TuiProgram
                // `Tui.app : TuiCfg model msg -> Task Error ()`
                | KernelFn::TuiApp
                // `Webview.app : WebviewCfg model msg -> Task Error ()`
                | KernelFn::WebviewApp
                // #76: Std.Html.Attributes fixed-key builders (`String`/`Bool`
                // -> Attribute msg).
                | KernelFn::HtmlAttrClass
                | KernelFn::HtmlAttrId
                | KernelFn::HtmlAttrHref
                | KernelFn::HtmlAttrSrc
                | KernelFn::HtmlAttrAlt
                | KernelFn::HtmlAttrValue
                | KernelFn::HtmlAttrName
                | KernelFn::HtmlAttrPlaceholder
                | KernelFn::HtmlAttrType
                | KernelFn::HtmlAttrFor
                | KernelFn::HtmlAttrStyle
                | KernelFn::HtmlAttrTitle
                | KernelFn::HtmlAttrChecked
                | KernelFn::HtmlAttrDisabled
                | KernelFn::HtmlAttrReadonly
                | KernelFn::HtmlAttrRequired
                | KernelFn::HtmlAttrMultiple
                | KernelFn::HtmlAttrSelected
                | KernelFn::HtmlAttrAutofocus
                // ── #76 Tier 1 — arity 1 ────────────────────────────────────
                | KernelFn::UiAspectRatio
                | KernelFn::BackgroundHoverColor
                | KernelFn::BackgroundFocusColor
                | KernelFn::BackgroundActiveColor
                | KernelFn::BackgroundDisabledColor
                | KernelFn::BorderHoverColor
                | KernelFn::BorderFocusColor
                | KernelFn::BorderActiveColor
                | KernelFn::BorderHoverWidth
                | KernelFn::BorderHoverRounded
                | KernelFn::FontWeight
                | KernelFn::FontLetterSpacing
                | KernelFn::FontWordSpacing
                | KernelFn::FontHoverColor
                | KernelFn::FontFocusColor
                | KernelFn::FontActiveColor
                | KernelFn::FontDisabledColor
                | KernelFn::FontHoverSize
                | KernelFn::HtmlAttrTabindex,
            ) => Ok(1),
            // Arity 2: `Ui.layout attrs elem`, `Ui.layoutWith cfg elem`,
            //          `Live.route path ctor`, `Live.renderStatic cfg path`.
            Callee::Kernel(
                // `Ui.layout : List (Attribute msg) -> Element msg -> Html msg`
                KernelFn::UiLayout
                // `Ui.layoutWith : { wrapperAttrs, rootAttrs } -> Element msg -> Html msg`
                | KernelFn::UiLayoutWith
                // ── M7: Ui element builders — arity 2 ────────────────────────
                // `Ui.el : List (Attribute msg) -> Element msg -> Element msg`
                | KernelFn::UiEl
                // `Ui.row : List (Attribute msg) -> List (Element msg) -> Element msg`
                | KernelFn::UiRow
                // `Ui.column : List (Attribute msg) -> List (Element msg) -> Element msg`
                | KernelFn::UiColumn
                // `Ui.wrappedRow : List (Attribute msg) -> List (Element msg) -> Element msg`
                | KernelFn::UiWrappedRow
                // `Ui.grid : List (Attribute msg) -> List (Element msg) -> Element msg`
                | KernelFn::UiGrid
                // `Ui.paddingXY : Int -> Int -> Attribute msg`
                | KernelFn::UiPaddingXY
                // `Ui.minimum : Int -> Length -> Length`
                | KernelFn::UiMinimum
                // `Ui.maximum : Int -> Length -> Length`
                | KernelFn::UiMaximum
                // ── M7: Html element builders — arity 2 ──────────────────────
                // `Html.styleNode : List (Attribute msg) -> String -> Html msg`
                | KernelFn::HtmlStyleNode
                // `Html.div : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlDiv
                // `Html.span : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlSpan
                // `Html.a : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlA
                // `Html.button : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlButton
                // `Html.p : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlP
                // ── #76 batch 2: Std.Html container element builders — arity 2 ─
                // `Html.h1 : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlH1
                // `Html.h2 : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlH2
                // `Html.h3 : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlH3
                // `Html.h4 : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlH4
                // `Html.h5 : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlH5
                // `Html.h6 : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlH6
                // `Html.nav : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlNav
                // `Html.section : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlSection
                // `Html.article : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlArticle
                // `Html.header : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlHeader
                // `Html.footer : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlFooter
                // `Html.main : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlMain
                // `Html.aside : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlAside
                // `Html.ul : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlUl
                // `Html.ol : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlOl
                // `Html.li : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlLi
                // `Html.table : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTable
                // `Html.thead : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlThead
                // `Html.tbody : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTbody
                // `Html.tfoot : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTfoot
                // `Html.tr : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTr
                // `Html.th : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTh
                // `Html.td : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTd
                // `Html.textarea : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTextarea
                // `Html.select : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlSelect
                // `Html.option : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlOption
                // `Html.label : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlLabel
                // `Html.form : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlForm
                // `Html.fieldset : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlFieldset
                // `Html.legend : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlLegend
                // `Html.pre : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlPre
                // `Html.code : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlCode
                // `Html.strong : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlStrong
                // `Html.em : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlEm
                // `Html.small : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlSmall
                // `Html.blockquote : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlBlockquote
                // `Html.figure : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlFigure
                // `Html.figcaption : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlFigcaption
                // `Html.details : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlDetails
                // `Html.summary : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlSummary
                // `Html.dialog : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlDialog
                // `Html.video : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlVideo
                // `Html.audio : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlAudio
                // `Html.canvas : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlCanvas
                // `Html.iframe : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlIframe
                // `Html.progress : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlProgress
                // `Html.meter : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlMeter
                // `Html.script : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlScript
                // `Html.body : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlBody
                // `Live.route : String -> page -> LiveRoute` (#106: `page` is a
                // bare polymorphic value — nullary ctor OR `String -> Page`)
                | KernelFn::LiveRoute
                // `Live.renderStatic : LiveAppCfg model msg -> String -> Task Error String`
                | KernelFn::LiveRenderStatic
                // #76: generic `Attr.attribute k v` / `Attr.boolAttribute k b`.
                | KernelFn::HtmlAttribute
                | KernelFn::HtmlBoolAttribute
                // ── #76 Tier 1 — arity 2 ────────────────────────────────────
                | KernelFn::UiAspectRatioWH
                | KernelFn::UiHtmlAttribute,
            ) => Ok(2),
            // Arity 3: `Ui.rgb r g b`, `Html.node tag attrs children`.
            Callee::Kernel(
                // `Ui.rgb : Int -> Int -> Int -> Color`
                KernelFn::UiRgb
                // `Html.node : String -> List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlNode,
            ) => Ok(3),
            // Arity 4: `Ui.rgba r g b a`.
            Callee::Kernel(
                // `Ui.rgba : Int -> Int -> Int -> Float -> Color`
                KernelFn::UiRgba,
            ) => Ok(4),
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
    fn ctor_arity_of(&self, home: &ModPath, name: Symbol) -> DResult<usize> {
        self.ctor_arity
            .get(&(home.clone(), name))
            .copied()
            .ok_or_else(|| bug("sky_lower::ctor_arity_of", "unknown constructor"))
    }

    #[allow(clippy::too_many_lines)] // declarative kernel-name dispatch table
    fn lower_callee(&self, callee: &canon::Expr) -> DResult<Callee> {
        match &callee.value {
            canon::Expr_::VarKernel { id, module, name } => {
                // Phase B fast path: use the pre-resolved id when available.
                // This avoids the ~400-arm string-match dispatch for every
                // registered kernel.  Unregistered entries (id = None) fall
                // through to the legacy string-match below.
                if let Some(sk) = id {
                    return Ok(Callee::Kernel(*sk));
                }
                match (self.resolve(*module)?, self.resolve(*name)?) {
                    ("Log", "println") => Ok(Callee::Kernel(KernelFn::LogPrintln)),
                    ("Log", "info") => Ok(Callee::Kernel(KernelFn::LogInfo)),
                    ("Log", "debug") => Ok(Callee::Kernel(KernelFn::LogDebug)),
                    ("Log", "warn") => Ok(Callee::Kernel(KernelFn::LogWarn)),
                    ("Log", "error") => Ok(Callee::Kernel(KernelFn::LogError)),
                    ("Log", "infoWith") => Ok(Callee::Kernel(KernelFn::LogInfoWith)),
                    ("Log", "debugWith") => Ok(Callee::Kernel(KernelFn::LogDebugWith)),
                    ("Log", "warnWith") => Ok(Callee::Kernel(KernelFn::LogWarnWith)),
                    ("Log", "errorWith") => Ok(Callee::Kernel(KernelFn::LogErrorWith)),
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
                    ("String", "containsIn") => Ok(Callee::Kernel(KernelFn::StringContainsIn)),
                    ("String", "startsWithIn") => Ok(Callee::Kernel(KernelFn::StringStartsWithIn)),
                    ("String", "endsWithIn") => Ok(Callee::Kernel(KernelFn::StringEndsWithIn)),
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
                    ("List", "append") => Ok(Callee::Kernel(KernelFn::ListAppend)),
                    ("List", "concat") => Ok(Callee::Kernel(KernelFn::ListConcat)),
                    ("List", "take") => Ok(Callee::Kernel(KernelFn::ListTake)),
                    ("List", "drop") => Ok(Callee::Kernel(KernelFn::ListDrop)),
                    ("List", "zip") => Ok(Callee::Kernel(KernelFn::ListZip)),
                    ("List", "cons") => Ok(Callee::Kernel(KernelFn::ListCons)),
                    ("List", "isEmpty") => Ok(Callee::Kernel(KernelFn::ListIsEmpty)),
                    ("List", "concatMap") => Ok(Callee::Kernel(KernelFn::ListConcatMap)),
                    ("List", "indexedMap") => Ok(Callee::Kernel(KernelFn::ListIndexedMap)),
                    ("List", "any") => Ok(Callee::Kernel(KernelFn::ListAny)),
                    ("List", "all") => Ok(Callee::Kernel(KernelFn::ListAll)),
                    ("List", "find") => Ok(Callee::Kernel(KernelFn::ListFind)),
                    // ── List batch (#119) ────────────────────────────────────
                    ("List", "filterMap") => Ok(Callee::Kernel(KernelFn::ListFilterMap)),
                    ("List", "sortBy") => Ok(Callee::Kernel(KernelFn::ListSortBy)),
                    ("Basics", "not") => Ok(Callee::Kernel(KernelFn::BasicsNot)),
                    ("Basics", "identity") => Ok(Callee::Kernel(KernelFn::BasicsIdentity)),
                    ("Basics", "always") => Ok(Callee::Kernel(KernelFn::BasicsAlways)),
                    ("Basics", "fst") => Ok(Callee::Kernel(KernelFn::BasicsFst)),
                    ("Basics", "snd") => Ok(Callee::Kernel(KernelFn::BasicsSnd)),
                    ("Basics", "modBy") => Ok(Callee::Kernel(KernelFn::BasicsModBy)),
                    ("Basics", "clamp") => Ok(Callee::Kernel(KernelFn::BasicsClamp)),
                    ("Basics", "toString") => Ok(Callee::Kernel(KernelFn::BasicsToString)),
                    // ── Error kernels (Sky.Core.Error — minimal `Error = String`
                    //    slice, #86) ─────────────────────────────────────────
                    ("Error", "unexpected") => Ok(Callee::Kernel(KernelFn::ErrorUnexpected)),
                    ("Error", "invalidInput") => Ok(Callee::Kernel(KernelFn::ErrorInvalidInput)),
                    ("Error", "io") => Ok(Callee::Kernel(KernelFn::ErrorIo)),
                    ("Error", "network") => Ok(Callee::Kernel(KernelFn::ErrorNetwork)),
                    ("Error", "ffi") => Ok(Callee::Kernel(KernelFn::ErrorFfi)),
                    ("Error", "decode") => Ok(Callee::Kernel(KernelFn::ErrorDecode)),
                    ("Error", "conflict") => Ok(Callee::Kernel(KernelFn::ErrorConflict)),
                    ("Error", "unavailable") => Ok(Callee::Kernel(KernelFn::ErrorUnavailable)),
                    ("Error", "timeout") => Ok(Callee::Kernel(KernelFn::ErrorTimeout)),
                    ("Error", "notFound") => Ok(Callee::Kernel(KernelFn::ErrorNotFound)),
                    ("Error", "permissionDenied") => {
                        Ok(Callee::Kernel(KernelFn::ErrorPermissionDenied))
                    }
                    ("Error", "toString") => Ok(Callee::Kernel(KernelFn::ErrorToString)),
                    ("Error", "withMessage") => Ok(Callee::Kernel(KernelFn::ErrorWithMessage)),
                    // ── CssSafety kernels (Sky.Core.CssSafety — Std.Css leaf
                    //    security kernels, #47) ──────────────────────────────
                    ("CssSafety", "safeValue") => {
                        Ok(Callee::Kernel(KernelFn::CssSafetySafeValue))
                    }
                    ("CssSafety", "safePropName") => {
                        Ok(Callee::Kernel(KernelFn::CssSafetySafePropName))
                    }
                    ("CssSafety", "safeSelector") => {
                        Ok(Callee::Kernel(KernelFn::CssSafetySafeSelector))
                    }
                    ("CssSafety", "stripStyleClose") => {
                        Ok(Callee::Kernel(KernelFn::CssSafetyStripStyleClose))
                    }
                    // ── Maybe kernels ──────────────────────────────────────
                    ("Maybe", "withDefault") => Ok(Callee::Kernel(KernelFn::MaybeWithDefault)),
                    ("Maybe", "map") => Ok(Callee::Kernel(KernelFn::MaybeMap)),
                    ("Maybe", "andThen") => Ok(Callee::Kernel(KernelFn::MaybeAndThen)),
                    ("Maybe", "map2") => Ok(Callee::Kernel(KernelFn::MaybeMap2)),
                    ("Maybe", "map3") => Ok(Callee::Kernel(KernelFn::MaybeMap3)),
                    ("Maybe", "map4") => Ok(Callee::Kernel(KernelFn::MaybeMap4)),
                    ("Maybe", "map5") => Ok(Callee::Kernel(KernelFn::MaybeMap5)),
                    ("Maybe", "andMap") => Ok(Callee::Kernel(KernelFn::MaybeAndMap)),
                    ("Maybe", "combine") => Ok(Callee::Kernel(KernelFn::MaybeCombine)),
                    // ── Result kernels ─────────────────────────────────────
                    ("Result", "withDefault") => Ok(Callee::Kernel(KernelFn::ResultWithDefault)),
                    ("Result", "map") => Ok(Callee::Kernel(KernelFn::ResultMap)),
                    ("Result", "andThen") => Ok(Callee::Kernel(KernelFn::ResultAndThen)),
                    ("Result", "mapError") => Ok(Callee::Kernel(KernelFn::ResultMapError)),
                    ("Result", "map2") => Ok(Callee::Kernel(KernelFn::ResultMap2)),
                    ("Result", "map3") => Ok(Callee::Kernel(KernelFn::ResultMap3)),
                    ("Result", "map4") => Ok(Callee::Kernel(KernelFn::ResultMap4)),
                    ("Result", "map5") => Ok(Callee::Kernel(KernelFn::ResultMap5)),
                    ("Result", "andMap") => Ok(Callee::Kernel(KernelFn::ResultAndMap)),
                    ("Result", "combine") => Ok(Callee::Kernel(KernelFn::ResultCombine)),
                    ("Result", "traverse") => Ok(Callee::Kernel(KernelFn::ResultTraverse)),
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
                    ("Crypto", "rsaSha256Sign") => {
                        Ok(Callee::Kernel(KernelFn::CryptoRsaSha256Sign))
                    }
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
                    // ── Uuid kernels (M5b) ────────────────────────────────────
                    ("Uuid", "v4") => Ok(Callee::Kernel(KernelFn::UuidV4)),
                    ("Uuid", "v7") => Ok(Callee::Kernel(KernelFn::UuidV7)),
                    ("Uuid", "parse") => Ok(Callee::Kernel(KernelFn::UuidParse)),
                    // ── Jwt kernels (M5b) ─────────────────────────────────────
                    ("Jwt", "encodeHs256") => Ok(Callee::Kernel(KernelFn::JwtEncodeHs256)),
                    ("Jwt", "decodeHs256") => Ok(Callee::Kernel(KernelFn::JwtDecodeHs256)),
                    ("Jwt", "encodeRs256") => Ok(Callee::Kernel(KernelFn::JwtEncodeRs256)),
                    ("Jwt", "decodeRs256") => Ok(Callee::Kernel(KernelFn::JwtDecodeRs256)),
                    // ── Task combinators (M5a) ────────────────────────────────
                    ("Task", "succeed") => Ok(Callee::Kernel(KernelFn::TaskSucceed)),
                    ("Task", "fail") => Ok(Callee::Kernel(KernelFn::TaskFail)),
                    ("Task", "map") => Ok(Callee::Kernel(KernelFn::TaskMap)),
                    ("Task", "andThen") => Ok(Callee::Kernel(KernelFn::TaskAndThen)),
                    ("Task", "mapError") => Ok(Callee::Kernel(KernelFn::TaskMapError)),
                    ("Task", "onError") => Ok(Callee::Kernel(KernelFn::TaskOnError)),
                    ("Task", "fromResult") => Ok(Callee::Kernel(KernelFn::TaskFromResult)),
                    ("Task", "andThenResult") => Ok(Callee::Kernel(KernelFn::TaskAndThenResult)),
                    ("Task", "sequence") => Ok(Callee::Kernel(KernelFn::TaskSequence)),
                    ("Task", "parallel") => Ok(Callee::Kernel(KernelFn::TaskParallel)),
                    ("Task", "run") => Ok(Callee::Kernel(KernelFn::TaskRun)),
                    // ── Io kernels (M5a) ──────────────────────────────────────
                    ("Io", "readLine") => Ok(Callee::Kernel(KernelFn::IoReadLine)),
                    ("Io", "writeStdout") => Ok(Callee::Kernel(KernelFn::IoWriteStdout)),
                    ("Io", "writeStderr") => Ok(Callee::Kernel(KernelFn::IoWriteStderr)),
                    // ── Time kernels (M5a) ────────────────────────────────────
                    ("Time", "now") => Ok(Callee::Kernel(KernelFn::TimeNow)),
                    ("Time", "sleep") => Ok(Callee::Kernel(KernelFn::TimeSleep)),
                    ("Time", "unixMillis") => Ok(Callee::Kernel(KernelFn::TimeUnixMillis)),
                    // ── System kernels (M5a) ──────────────────────────────────
                    ("System", "args") => Ok(Callee::Kernel(KernelFn::SystemArgs)),
                    ("System", "getenv") => Ok(Callee::Kernel(KernelFn::SystemGetenv)),
                    ("System", "getenvOr") => Ok(Callee::Kernel(KernelFn::SystemGetenvOr)),
                    ("System", "getArg") => Ok(Callee::Kernel(KernelFn::SystemGetArg)),
                    ("System", "getenvInt") => Ok(Callee::Kernel(KernelFn::SystemGetenvInt)),
                    ("System", "getenvBool") => Ok(Callee::Kernel(KernelFn::SystemGetenvBool)),
                    ("System", "setenv") => Ok(Callee::Kernel(KernelFn::SystemSetenv)),
                    ("System", "unsetenv") => Ok(Callee::Kernel(KernelFn::SystemUnsetenv)),
                    ("System", "cwd") => Ok(Callee::Kernel(KernelFn::SystemCwd)),
                    ("System", "loadEnv") => Ok(Callee::Kernel(KernelFn::SystemLoadEnv)),
                    ("System", "exit") => Ok(Callee::Kernel(KernelFn::SystemExit)),
                    // ── Random kernels (M5a) ──────────────────────────────────
                    ("Random", "int") => Ok(Callee::Kernel(KernelFn::RandomInt)),
                    ("Random", "float") => Ok(Callee::Kernel(KernelFn::RandomFloat)),
                    ("Random", "choice") => Ok(Callee::Kernel(KernelFn::RandomChoice)),
                    // ── File kernels (M5a) ────────────────────────────────────
                    ("File", "readFile") => Ok(Callee::Kernel(KernelFn::FileReadFile)),
                    ("File", "writeFile") => Ok(Callee::Kernel(KernelFn::FileWriteFile)),
                    ("File", "exists") => Ok(Callee::Kernel(KernelFn::FileExists)),
                    ("File", "remove") => Ok(Callee::Kernel(KernelFn::FileRemove)),
                    ("File", "mkdirAll") => Ok(Callee::Kernel(KernelFn::FileMkdirAll)),
                    ("File", "readFileLimit") => Ok(Callee::Kernel(KernelFn::FileReadFileLimit)),
                    ("File", "readFileBytes") => Ok(Callee::Kernel(KernelFn::FileReadFileBytes)),
                    ("File", "append") => Ok(Callee::Kernel(KernelFn::FileAppend)),
                    ("File", "readDir") => Ok(Callee::Kernel(KernelFn::FileReadDir)),
                    ("File", "isDir") => Ok(Callee::Kernel(KernelFn::FileIsDir)),
                    ("File", "tempFile") => Ok(Callee::Kernel(KernelFn::FileTempFile)),
                    ("File", "tempDir") => Ok(Callee::Kernel(KernelFn::FileTempDir)),
                    ("File", "copy") => Ok(Callee::Kernel(KernelFn::FileCopy)),
                    ("File", "rename") => Ok(Callee::Kernel(KernelFn::FileRename)),
                    ("File", "delete") => Ok(Callee::Kernel(KernelFn::FileDelete)),
                    // ── Http kernels (M5b) ────────────────────────────────────
                    ("Http", "get") => Ok(Callee::Kernel(KernelFn::HttpGet)),
                    ("Http", "post") => Ok(Callee::Kernel(KernelFn::HttpPost)),
                    ("Http", "request") => Ok(Callee::Kernel(KernelFn::HttpRequest)),
                    ("Http", "parseQuery") => Ok(Callee::Kernel(KernelFn::HttpParseQuery)),
                    ("Http", "defaultRequest") => Ok(Callee::Kernel(KernelFn::HttpDefaultRequest)),
                    ("Http", "withMethod") => Ok(Callee::Kernel(KernelFn::HttpWithMethod)),
                    ("Http", "withTimeout") => Ok(Callee::Kernel(KernelFn::HttpWithTimeout)),
                    ("Http", "withBody") => Ok(Callee::Kernel(KernelFn::HttpWithBody)),
                    ("Http", "withHeader") => Ok(Callee::Kernel(KernelFn::HttpWithHeader)),
                    // ── Db kernels (M5b-db) ──────────────────────────────────
                    ("Db", "connect") => Ok(Callee::Kernel(KernelFn::DbConnect)),
                    ("Db", "open") => Ok(Callee::Kernel(KernelFn::DbOpen)),
                    ("Db", "close") => Ok(Callee::Kernel(KernelFn::DbClose)),
                    ("Db", "execRaw") => Ok(Callee::Kernel(KernelFn::DbExecRaw)),
                    ("Db", "exec") => Ok(Callee::Kernel(KernelFn::DbExec)),
                    ("Db", "query") => Ok(Callee::Kernel(KernelFn::DbQuery)),
                    ("Db", "queryDecode") => Ok(Callee::Kernel(KernelFn::DbQueryDecode)),
                    ("Db", "getString") => Ok(Callee::Kernel(KernelFn::DbGetString)),
                    ("Db", "getInt") => Ok(Callee::Kernel(KernelFn::DbGetInt)),
                    ("Db", "getBool") => Ok(Callee::Kernel(KernelFn::DbGetBool)),
                    ("Db", "getField") => Ok(Callee::Kernel(KernelFn::DbGetField)),
                    ("Db", "insertRow") => Ok(Callee::Kernel(KernelFn::DbInsertRow)),
                    ("Db", "getById") => Ok(Callee::Kernel(KernelFn::DbGetById)),
                    ("Db", "updateById") => Ok(Callee::Kernel(KernelFn::DbUpdateById)),
                    ("Db", "deleteById") => Ok(Callee::Kernel(KernelFn::DbDeleteById)),
                    ("Db", "findOneByField") => Ok(Callee::Kernel(KernelFn::DbFindOneByField)),
                    ("Db", "findManyByField") => Ok(Callee::Kernel(KernelFn::DbFindManyByField)),
                    ("Db", "findByConditions") => Ok(Callee::Kernel(KernelFn::DbFindByConditions)),
                    ("Db", "unsafeFindWhere") => Ok(Callee::Kernel(KernelFn::DbUnsafeFindWhere)),
                    ("Db", "insertFields") => Ok(Callee::Kernel(KernelFn::DbInsertFields)),
                    ("Db", "updateFields") => Ok(Callee::Kernel(KernelFn::DbUpdateFields)),
                    ("Db", "insertFieldsReturning") => {
                        Ok(Callee::Kernel(KernelFn::DbInsertFieldsReturning))
                    }
                    ("Db", "withTransaction") => Ok(Callee::Kernel(KernelFn::DbWithTransaction)),
                    ("Db", "migrate") => Ok(Callee::Kernel(KernelFn::DbMigrate)),
                    // ── Db.Decode kernels (M5b-db) ────────────────────────────
                    ("Db.Decode", "string") => Ok(Callee::Kernel(KernelFn::DbDecString)),
                    ("Db.Decode", "int") => Ok(Callee::Kernel(KernelFn::DbDecInt)),
                    ("Db.Decode", "float") => Ok(Callee::Kernel(KernelFn::DbDecFloat)),
                    ("Db.Decode", "bool") => Ok(Callee::Kernel(KernelFn::DbDecBool)),
                    ("Db.Decode", "nullable") => Ok(Callee::Kernel(KernelFn::DbDecNullable)),
                    ("Db.Decode", "map") => Ok(Callee::Kernel(KernelFn::DbDecMap)),
                    ("Db.Decode", "andThen") => Ok(Callee::Kernel(KernelFn::DbDecAndThen)),
                    ("Db.Decode", "succeed") => Ok(Callee::Kernel(KernelFn::DbDecSucceed)),
                    ("Db.Decode", "fail") => Ok(Callee::Kernel(KernelFn::DbDecFail)),
                    ("Db.Decode", "map2") => Ok(Callee::Kernel(KernelFn::DbDecMap2)),
                    ("Db.Decode", "map3") => Ok(Callee::Kernel(KernelFn::DbDecMap3)),
                    ("Db.Decode", "map4") => Ok(Callee::Kernel(KernelFn::DbDecMap4)),
                    ("Db.Decode", "required") => Ok(Callee::Kernel(KernelFn::DbDecRequired)),
                    ("Db.Decode", "optional") => Ok(Callee::Kernel(KernelFn::DbDecOptional)),
                    // ── TEA Cmd / Sub / Time kernels (M5c) ───────────────────────
                    ("Cmd", "none") => Ok(Callee::Kernel(KernelFn::CmdNone)),
                    ("Cmd", "batch") => Ok(Callee::Kernel(KernelFn::CmdBatch)),
                    ("Cmd", "perform") => Ok(Callee::Kernel(KernelFn::CmdPerform)),
                    ("Sub", "none") => Ok(Callee::Kernel(KernelFn::SubNone)),
                    ("Sub", "batch") => Ok(Callee::Kernel(KernelFn::SubBatch)),
                    ("Sub", "every") => Ok(Callee::Kernel(KernelFn::SubEvery)),
                    ("Time", "every") => Ok(Callee::Kernel(KernelFn::TimeEvery)),
                    // ── Sky.Http.Server kernels (M6) ─────────────────────────────
                    ("Server", "get") => Ok(Callee::Kernel(KernelFn::ServerGet)),
                    ("Server", "post") => Ok(Callee::Kernel(KernelFn::ServerPost)),
                    ("Server", "put") => Ok(Callee::Kernel(KernelFn::ServerPut)),
                    ("Server", "delete") => Ok(Callee::Kernel(KernelFn::ServerDelete)),
                    ("Server", "any") => Ok(Callee::Kernel(KernelFn::ServerAny)),
                    ("Server", "api") => Ok(Callee::Kernel(KernelFn::ServerApi)),
                    ("Server", "static") => Ok(Callee::Kernel(KernelFn::ServerStatic)),
                    ("Server", "listen") => Ok(Callee::Kernel(KernelFn::ServerListen)),
                    ("Server", "text") => Ok(Callee::Kernel(KernelFn::ServerText)),
                    ("Server", "json") => Ok(Callee::Kernel(KernelFn::ServerJson)),
                    ("Server", "html") => Ok(Callee::Kernel(KernelFn::ServerHtml)),
                    ("Server", "withStatus") => Ok(Callee::Kernel(KernelFn::ServerWithStatus)),
                    ("Server", "withHeader") => Ok(Callee::Kernel(KernelFn::ServerWithHeader)),
                    ("Server", "redirect") => Ok(Callee::Kernel(KernelFn::ServerRedirect)),
                    ("Server", "param") => Ok(Callee::Kernel(KernelFn::ServerParam)),
                    ("Server", "queryParam") => Ok(Callee::Kernel(KernelFn::ServerQueryParam)),
                    ("Server", "header") => Ok(Callee::Kernel(KernelFn::ServerHeader)),
                    ("Server", "getCookie") => Ok(Callee::Kernel(KernelFn::ServerGetCookie)),
                    ("Server", "body") => Ok(Callee::Kernel(KernelFn::ServerBody)),
                    ("Server", "path") => Ok(Callee::Kernel(KernelFn::ServerPath)),
                    ("Server", "method") => Ok(Callee::Kernel(KernelFn::ServerMethod)),
                    ("Server", "cookie") => Ok(Callee::Kernel(KernelFn::ServerCookieNew)),
                    ("Server", "withCookie") => Ok(Callee::Kernel(KernelFn::ServerWithCookie)),
                    ("Middleware", "withCors") => Ok(Callee::Kernel(KernelFn::MiddlewareWithCors)),
                    ("Middleware", "withLogging") => {
                        Ok(Callee::Kernel(KernelFn::MiddlewareWithLogging))
                    }
                    ("Middleware", "withBasicAuth") => {
                        Ok(Callee::Kernel(KernelFn::MiddlewareWithBasicAuth))
                    }
                    ("Middleware", "withRateLimit") => {
                        Ok(Callee::Kernel(KernelFn::MiddlewareWithRateLimit))
                    }
                    ("RateLimit", "allow") => Ok(Callee::Kernel(KernelFn::RateLimitAllow)),
                    // ── M7: Std.Ui / Std.Html render kernels ─────────────────
                    ("Ui", "layout") => Ok(Callee::Kernel(KernelFn::UiLayout)),
                    ("Ui", "layoutWith") => Ok(Callee::Kernel(KernelFn::UiLayoutWith)),
                    ("Html", "render" | "toString") => Ok(Callee::Kernel(KernelFn::HtmlRender)),
                    ("Html", "escapeHtml" | "escapeText") => {
                        Ok(Callee::Kernel(KernelFn::HtmlEscapeText))
                    }
                    ("Html", "escapeAttr") => Ok(Callee::Kernel(KernelFn::HtmlEscapeAttr)),
                    ("Html", "attrToString") => Ok(Callee::Kernel(KernelFn::HtmlAttrToString)),
                    // ── M7: Std.Ui element builders ───────────────────────────
                    ("Ui", "none") => Ok(Callee::Kernel(KernelFn::UiNone)),
                    ("Ui", "text") => Ok(Callee::Kernel(KernelFn::UiText)),
                    ("Ui", "html") => Ok(Callee::Kernel(KernelFn::UiHtml)),
                    ("Ui", "el") => Ok(Callee::Kernel(KernelFn::UiEl)),
                    ("Ui", "row") => Ok(Callee::Kernel(KernelFn::UiRow)),
                    ("Ui", "column") => Ok(Callee::Kernel(KernelFn::UiColumn)),
                    ("Ui", "wrappedRow") => Ok(Callee::Kernel(KernelFn::UiWrappedRow)),
                    ("Ui", "grid") => Ok(Callee::Kernel(KernelFn::UiGrid)),
                    // ── M7: Std.Ui attribute builders ─────────────────────────
                    ("Ui", "spacing") => Ok(Callee::Kernel(KernelFn::UiSpacing)),
                    ("Ui", "padding") => Ok(Callee::Kernel(KernelFn::UiPadding)),
                    ("Ui", "paddingXY") => Ok(Callee::Kernel(KernelFn::UiPaddingXY)),
                    ("Ui", "width") => Ok(Callee::Kernel(KernelFn::UiWidth)),
                    ("Ui", "height") => Ok(Callee::Kernel(KernelFn::UiHeight)),
                    ("Ui", "centerX") => Ok(Callee::Kernel(KernelFn::UiCenterX)),
                    ("Ui", "centerY") => Ok(Callee::Kernel(KernelFn::UiCenterY)),
                    ("Ui", "alignLeft") => Ok(Callee::Kernel(KernelFn::UiAlignLeft)),
                    ("Ui", "alignRight") => Ok(Callee::Kernel(KernelFn::UiAlignRight)),
                    ("Ui", "alignTop") => Ok(Callee::Kernel(KernelFn::UiAlignTop)),
                    ("Ui", "alignBottom") => Ok(Callee::Kernel(KernelFn::UiAlignBottom)),
                    ("Ui", "pointer") => Ok(Callee::Kernel(KernelFn::UiPointer)),
                    ("Ui", "clip" | "clipX" | "clipY") => Ok(Callee::Kernel(KernelFn::UiClip)),
                    ("Ui", "scrollbars" | "scrollbarX" | "scrollbarY") => {
                        Ok(Callee::Kernel(KernelFn::UiScrollbars))
                    }
                    ("Ui", "gridColumns") => Ok(Callee::Kernel(KernelFn::UiGridColumns)),
                    // ── M7: Std.Ui Length builders ────────────────────────────
                    ("Ui", "px") => Ok(Callee::Kernel(KernelFn::UiPx)),
                    ("Ui", "fill") => Ok(Callee::Kernel(KernelFn::UiFill)),
                    ("Ui", "content") => Ok(Callee::Kernel(KernelFn::UiContent)),
                    ("Ui", "shrink") => Ok(Callee::Kernel(KernelFn::UiShrink)),
                    ("Ui", "fillPortion") => Ok(Callee::Kernel(KernelFn::UiFillPortion)),
                    ("Ui", "vh") => Ok(Callee::Kernel(KernelFn::UiVh)),
                    ("Ui", "vw") => Ok(Callee::Kernel(KernelFn::UiVw)),
                    ("Ui", "minimum") => Ok(Callee::Kernel(KernelFn::UiMinimum)),
                    ("Ui", "maximum") => Ok(Callee::Kernel(KernelFn::UiMaximum)),
                    // ── M7: Std.Ui Color builders ─────────────────────────────
                    ("Ui", "rgb") => Ok(Callee::Kernel(KernelFn::UiRgb)),
                    ("Ui", "rgba") => Ok(Callee::Kernel(KernelFn::UiRgba)),
                    ("Ui", "white") => Ok(Callee::Kernel(KernelFn::UiWhite)),
                    ("Ui", "black") => Ok(Callee::Kernel(KernelFn::UiBlack)),
                    ("Ui", "transparent") => Ok(Callee::Kernel(KernelFn::UiTransparent)),
                    // ── M7: Background sub-module ─────────────────────────────
                    ("Background", "color") => Ok(Callee::Kernel(KernelFn::BackgroundColor)),
                    ("Background", "image") => Ok(Callee::Kernel(KernelFn::BackgroundImage)),
                    // ── M7: Border sub-module ─────────────────────────────────
                    ("Border", "width") => Ok(Callee::Kernel(KernelFn::BorderWidth)),
                    ("Border", "rounded") => Ok(Callee::Kernel(KernelFn::BorderRounded)),
                    ("Border", "color") => Ok(Callee::Kernel(KernelFn::BorderColor)),
                    // ── M7: Font sub-module ───────────────────────────────────
                    ("Font", "size") => Ok(Callee::Kernel(KernelFn::FontSize)),
                    ("Font", "color") => Ok(Callee::Kernel(KernelFn::FontColor)),
                    ("Font", "family") => Ok(Callee::Kernel(KernelFn::FontFamily)),
                    ("Font", "bold") => Ok(Callee::Kernel(KernelFn::FontBold)),
                    ("Font", "italic") => Ok(Callee::Kernel(KernelFn::FontItalic)),
                    // ── M7: Html element builders ─────────────────────────────
                    ("Html", "text") => Ok(Callee::Kernel(KernelFn::HtmlTextNode)),
                    ("Html", "raw") => Ok(Callee::Kernel(KernelFn::HtmlRawNode)),
                    // `styleNode : List Attr -> String -> Html msg` is arity-2 —
                    // its own kernel, NOT folded into the arity-3 `HtmlNode`. The
                    // dedicated kernel close-tag-neutralises the CSS body (F7).
                    ("Html", "styleNode") => Ok(Callee::Kernel(KernelFn::HtmlStyleNode)),
                    (
                        "Html",
                        "node" | "voidNode" | "doctype" | "titleNode" | "htmlNode" | "headNode"
                        | "title",
                    ) => Ok(Callee::Kernel(KernelFn::HtmlNode)),
                    ("Html", "div") => Ok(Callee::Kernel(KernelFn::HtmlDiv)),
                    ("Html", "span") => Ok(Callee::Kernel(KernelFn::HtmlSpan)),
                    ("Html", "a") => Ok(Callee::Kernel(KernelFn::HtmlA)),
                    ("Html", "button") => Ok(Callee::Kernel(KernelFn::HtmlButton)),
                    ("Html", "p") => Ok(Callee::Kernel(KernelFn::HtmlP)),
                    ("Html", "input") => Ok(Callee::Kernel(KernelFn::HtmlInput)),
                    ("Html", "img") => Ok(Callee::Kernel(KernelFn::HtmlImg)),
                    // ── #76 batch 2: Std.Html element builders (canonical tag →
                    //    dedicated variant; the emit arm bakes the wire tag via
                    //    `html_element_tag`). Replaces the old wrong-render fold
                    //    (nav→<p>, h1→<p>, br→<img>, header→<div>, link→<a>). ──
                    ("Html", "h1") => Ok(Callee::Kernel(KernelFn::HtmlH1)),
                    ("Html", "h2") => Ok(Callee::Kernel(KernelFn::HtmlH2)),
                    ("Html", "h3") => Ok(Callee::Kernel(KernelFn::HtmlH3)),
                    ("Html", "h4") => Ok(Callee::Kernel(KernelFn::HtmlH4)),
                    ("Html", "h5") => Ok(Callee::Kernel(KernelFn::HtmlH5)),
                    ("Html", "h6") => Ok(Callee::Kernel(KernelFn::HtmlH6)),
                    ("Html", "nav") => Ok(Callee::Kernel(KernelFn::HtmlNav)),
                    ("Html", "section") => Ok(Callee::Kernel(KernelFn::HtmlSection)),
                    ("Html", "article") => Ok(Callee::Kernel(KernelFn::HtmlArticle)),
                    ("Html", "header") => Ok(Callee::Kernel(KernelFn::HtmlHeader)),
                    ("Html", "footer") => Ok(Callee::Kernel(KernelFn::HtmlFooter)),
                    ("Html", "main") => Ok(Callee::Kernel(KernelFn::HtmlMain)),
                    ("Html", "aside") => Ok(Callee::Kernel(KernelFn::HtmlAside)),
                    ("Html", "ul") => Ok(Callee::Kernel(KernelFn::HtmlUl)),
                    ("Html", "ol") => Ok(Callee::Kernel(KernelFn::HtmlOl)),
                    ("Html", "li") => Ok(Callee::Kernel(KernelFn::HtmlLi)),
                    ("Html", "table") => Ok(Callee::Kernel(KernelFn::HtmlTable)),
                    ("Html", "thead") => Ok(Callee::Kernel(KernelFn::HtmlThead)),
                    ("Html", "tbody") => Ok(Callee::Kernel(KernelFn::HtmlTbody)),
                    ("Html", "tfoot") => Ok(Callee::Kernel(KernelFn::HtmlTfoot)),
                    ("Html", "tr") => Ok(Callee::Kernel(KernelFn::HtmlTr)),
                    ("Html", "th") => Ok(Callee::Kernel(KernelFn::HtmlTh)),
                    ("Html", "td") => Ok(Callee::Kernel(KernelFn::HtmlTd)),
                    ("Html", "textarea") => Ok(Callee::Kernel(KernelFn::HtmlTextarea)),
                    ("Html", "select") => Ok(Callee::Kernel(KernelFn::HtmlSelect)),
                    ("Html", "option") => Ok(Callee::Kernel(KernelFn::HtmlOption)),
                    ("Html", "label") => Ok(Callee::Kernel(KernelFn::HtmlLabel)),
                    ("Html", "form") => Ok(Callee::Kernel(KernelFn::HtmlForm)),
                    ("Html", "fieldset") => Ok(Callee::Kernel(KernelFn::HtmlFieldset)),
                    ("Html", "legend") => Ok(Callee::Kernel(KernelFn::HtmlLegend)),
                    ("Html", "pre") => Ok(Callee::Kernel(KernelFn::HtmlPre)),
                    ("Html", "code") => Ok(Callee::Kernel(KernelFn::HtmlCode)),
                    ("Html", "strong") => Ok(Callee::Kernel(KernelFn::HtmlStrong)),
                    ("Html", "em") => Ok(Callee::Kernel(KernelFn::HtmlEm)),
                    ("Html", "small") => Ok(Callee::Kernel(KernelFn::HtmlSmall)),
                    ("Html", "blockquote") => Ok(Callee::Kernel(KernelFn::HtmlBlockquote)),
                    ("Html", "figure") => Ok(Callee::Kernel(KernelFn::HtmlFigure)),
                    ("Html", "figcaption") => Ok(Callee::Kernel(KernelFn::HtmlFigcaption)),
                    ("Html", "details") => Ok(Callee::Kernel(KernelFn::HtmlDetails)),
                    ("Html", "summary") => Ok(Callee::Kernel(KernelFn::HtmlSummary)),
                    ("Html", "dialog") => Ok(Callee::Kernel(KernelFn::HtmlDialog)),
                    ("Html", "video") => Ok(Callee::Kernel(KernelFn::HtmlVideo)),
                    ("Html", "audio") => Ok(Callee::Kernel(KernelFn::HtmlAudio)),
                    ("Html", "canvas") => Ok(Callee::Kernel(KernelFn::HtmlCanvas)),
                    ("Html", "iframe") => Ok(Callee::Kernel(KernelFn::HtmlIframe)),
                    ("Html", "progress") => Ok(Callee::Kernel(KernelFn::HtmlProgress)),
                    ("Html", "meter") => Ok(Callee::Kernel(KernelFn::HtmlMeter)),
                    ("Html", "script") => Ok(Callee::Kernel(KernelFn::HtmlScript)),
                    ("Html", "body") => Ok(Callee::Kernel(KernelFn::HtmlBody)),
                    ("Html", "br") => Ok(Callee::Kernel(KernelFn::HtmlBr)),
                    ("Html", "hr") => Ok(Callee::Kernel(KernelFn::HtmlHr)),
                    ("Html", "meta") => Ok(Callee::Kernel(KernelFn::HtmlMeta)),
                    ("Html", "link") => Ok(Callee::Kernel(KernelFn::HtmlLink)),
                    ("Html", "area") => Ok(Callee::Kernel(KernelFn::HtmlArea)),
                    ("Html", "base") => Ok(Callee::Kernel(KernelFn::HtmlBase)),
                    ("Html", "col") => Ok(Callee::Kernel(KernelFn::HtmlCol)),
                    ("Html", "embed") => Ok(Callee::Kernel(KernelFn::HtmlEmbed)),
                    ("Html", "source") => Ok(Callee::Kernel(KernelFn::HtmlSource)),
                    ("Html", "track") => Ok(Callee::Kernel(KernelFn::HtmlTrack)),
                    ("Html", "wbr") => Ok(Callee::Kernel(KernelFn::HtmlWbr)),
                    // ── #76: Std.Html.Attributes builders (legacy arm; the
                    //    id-fast-path handles these in practice, this arm keeps
                    //    decl() ⇔ legacy parity per `decl_equiv_legacy_match`). ──
                    ("Attr", "class") => Ok(Callee::Kernel(KernelFn::HtmlAttrClass)),
                    ("Attr", "id") => Ok(Callee::Kernel(KernelFn::HtmlAttrId)),
                    ("Attr", "href") => Ok(Callee::Kernel(KernelFn::HtmlAttrHref)),
                    ("Attr", "src") => Ok(Callee::Kernel(KernelFn::HtmlAttrSrc)),
                    ("Attr", "alt") => Ok(Callee::Kernel(KernelFn::HtmlAttrAlt)),
                    ("Attr", "value") => Ok(Callee::Kernel(KernelFn::HtmlAttrValue)),
                    ("Attr", "name") => Ok(Callee::Kernel(KernelFn::HtmlAttrName)),
                    ("Attr", "placeholder") => Ok(Callee::Kernel(KernelFn::HtmlAttrPlaceholder)),
                    ("Attr", "type_") => Ok(Callee::Kernel(KernelFn::HtmlAttrType)),
                    ("Attr", "for_") => Ok(Callee::Kernel(KernelFn::HtmlAttrFor)),
                    ("Attr", "style") => Ok(Callee::Kernel(KernelFn::HtmlAttrStyle)),
                    ("Attr", "title") => Ok(Callee::Kernel(KernelFn::HtmlAttrTitle)),
                    ("Attr", "checked") => Ok(Callee::Kernel(KernelFn::HtmlAttrChecked)),
                    ("Attr", "disabled") => Ok(Callee::Kernel(KernelFn::HtmlAttrDisabled)),
                    ("Attr", "readonly") => Ok(Callee::Kernel(KernelFn::HtmlAttrReadonly)),
                    ("Attr", "required") => Ok(Callee::Kernel(KernelFn::HtmlAttrRequired)),
                    ("Attr", "multiple") => Ok(Callee::Kernel(KernelFn::HtmlAttrMultiple)),
                    ("Attr", "selected") => Ok(Callee::Kernel(KernelFn::HtmlAttrSelected)),
                    ("Attr", "autofocus") => Ok(Callee::Kernel(KernelFn::HtmlAttrAutofocus)),
                    ("Attr", "attribute") => Ok(Callee::Kernel(KernelFn::HtmlAttribute)),
                    ("Attr", "boolAttribute") => Ok(Callee::Kernel(KernelFn::HtmlBoolAttribute)),
                    ("Attr", "noAttr") => Ok(Callee::Kernel(KernelFn::HtmlNoAttr)),
                    // ── M7: Phase-1a event-attribute builders (Std.Ui qualifier) ──
                    // `Ui.onClick` etc. produce the `Std.Ui.Attribute` variant.
                    // NB: the primary resolution path is the id fast-path above
                    // (env.rs threads the pre-resolved kernel id); these string
                    // arms are the legacy fallback for an `id = None` VarKernel.
                    ("Ui", "onClick" | "onMsg") => Ok(Callee::Kernel(KernelFn::UiOnClick)),
                    ("Ui", "onFocus") => Ok(Callee::Kernel(KernelFn::UiOnFocus)),
                    ("Ui", "onBlur") => Ok(Callee::Kernel(KernelFn::UiOnBlur)),
                    ("Ui", "onMouseOver") => Ok(Callee::Kernel(KernelFn::UiOnMouseOver)),
                    ("Ui", "onMouseOut") => Ok(Callee::Kernel(KernelFn::UiOnMouseOut)),
                    ("Ui", "onInput") => Ok(Callee::Kernel(KernelFn::UiOnInput)),
                    ("Ui", "onChange") => Ok(Callee::Kernel(KernelFn::UiOnChange)),
                    ("Ui", "onKeyDown") => Ok(Callee::Kernel(KernelFn::UiOnKeyDown)),
                    ("Ui", "onKeyUp") => Ok(Callee::Kernel(KernelFn::UiOnKeyUp)),
                    ("Ui", "onBool") => {
                        // onBool : (Bool -> msg) -> Attribute msg — Bool-carrying closure
                        Ok(Callee::Kernel(KernelFn::UiOnBool))
                    }
                    // ── #107: Std.Html.Events builders (Event qualifier) — produce
                    // the `Std.Html.Attribute` variant so they compose with the
                    // Std.Html element + attribute builders. Same fallback note
                    // as the `Ui` arms above (id fast-path is primary).
                    ("Event", "onClick" | "onMsg") => Ok(Callee::Kernel(KernelFn::HtmlOnClick)),
                    ("Event", "onFocus") => Ok(Callee::Kernel(KernelFn::HtmlOnFocus)),
                    ("Event", "onBlur") => Ok(Callee::Kernel(KernelFn::HtmlOnBlur)),
                    ("Event", "onMouseOver") => Ok(Callee::Kernel(KernelFn::HtmlOnMouseOver)),
                    ("Event", "onMouseOut") => Ok(Callee::Kernel(KernelFn::HtmlOnMouseOut)),
                    ("Event", "onSubmit") => Ok(Callee::Kernel(KernelFn::HtmlOnSubmit)),
                    ("Event", "onInput") => Ok(Callee::Kernel(KernelFn::HtmlOnInput)),
                    ("Event", "onChange") => Ok(Callee::Kernel(KernelFn::HtmlOnChange)),
                    ("Event", "onKeyDown") => Ok(Callee::Kernel(KernelFn::HtmlOnKeyDown)),
                    ("Event", "onKeyUp") => Ok(Callee::Kernel(KernelFn::HtmlOnKeyUp)),
                    ("Event", "onBool") => Ok(Callee::Kernel(KernelFn::HtmlOnBool)),
                    // ── #76 Tier 1: extended Std.Ui attribute builders ────────
                    ("Ui", "square") => Ok(Callee::Kernel(KernelFn::UiSquare)),
                    ("Ui", "widescreen") => Ok(Callee::Kernel(KernelFn::UiWidescreen)),
                    ("Ui", "aspectRatio") => Ok(Callee::Kernel(KernelFn::UiAspectRatio)),
                    ("Ui", "aspectRatioWH") => Ok(Callee::Kernel(KernelFn::UiAspectRatioWH)),
                    ("Ui", "htmlAttribute") => Ok(Callee::Kernel(KernelFn::UiHtmlAttribute)),
                    ("Background", "hoverColor") => {
                        Ok(Callee::Kernel(KernelFn::BackgroundHoverColor))
                    }
                    ("Background", "focusColor") => {
                        Ok(Callee::Kernel(KernelFn::BackgroundFocusColor))
                    }
                    ("Background", "activeColor") => {
                        Ok(Callee::Kernel(KernelFn::BackgroundActiveColor))
                    }
                    ("Background", "disabledColor") => {
                        Ok(Callee::Kernel(KernelFn::BackgroundDisabledColor))
                    }
                    ("Border", "solid") => Ok(Callee::Kernel(KernelFn::BorderSolid)),
                    ("Border", "dashed") => Ok(Callee::Kernel(KernelFn::BorderDashed)),
                    ("Border", "dotted") => Ok(Callee::Kernel(KernelFn::BorderDotted)),
                    ("Border", "hoverColor") => Ok(Callee::Kernel(KernelFn::BorderHoverColor)),
                    ("Border", "focusColor") => Ok(Callee::Kernel(KernelFn::BorderFocusColor)),
                    ("Border", "activeColor") => Ok(Callee::Kernel(KernelFn::BorderActiveColor)),
                    ("Border", "hoverWidth") => Ok(Callee::Kernel(KernelFn::BorderHoverWidth)),
                    ("Border", "hoverRounded") => {
                        Ok(Callee::Kernel(KernelFn::BorderHoverRounded))
                    }
                    ("Font", "weight") => Ok(Callee::Kernel(KernelFn::FontWeight)),
                    ("Font", "semiBold") => Ok(Callee::Kernel(KernelFn::FontSemiBold)),
                    ("Font", "regular") => Ok(Callee::Kernel(KernelFn::FontRegular)),
                    ("Font", "light") => Ok(Callee::Kernel(KernelFn::FontLight)),
                    ("Font", "extraBold") => Ok(Callee::Kernel(KernelFn::FontExtraBold)),
                    ("Font", "black") => Ok(Callee::Kernel(KernelFn::FontBlack)),
                    ("Font", "underline") => Ok(Callee::Kernel(KernelFn::FontUnderline)),
                    ("Font", "noDecoration") => Ok(Callee::Kernel(KernelFn::FontNoDecoration)),
                    ("Font", "letterSpacing") => Ok(Callee::Kernel(KernelFn::FontLetterSpacing)),
                    ("Font", "wordSpacing") => Ok(Callee::Kernel(KernelFn::FontWordSpacing)),
                    ("Font", "alignLeft") => Ok(Callee::Kernel(KernelFn::FontAlignLeft)),
                    ("Font", "alignRight") => Ok(Callee::Kernel(KernelFn::FontAlignRight)),
                    ("Font", "center") => Ok(Callee::Kernel(KernelFn::FontCenter)),
                    ("Font", "justify") => Ok(Callee::Kernel(KernelFn::FontJustify)),
                    ("Font", "sansSerif") => Ok(Callee::Kernel(KernelFn::FontSansSerif)),
                    ("Font", "serif") => Ok(Callee::Kernel(KernelFn::FontSerif)),
                    ("Font", "monospace") => Ok(Callee::Kernel(KernelFn::FontMonospace)),
                    ("Font", "hoverColor") => Ok(Callee::Kernel(KernelFn::FontHoverColor)),
                    ("Font", "focusColor") => Ok(Callee::Kernel(KernelFn::FontFocusColor)),
                    ("Font", "activeColor") => Ok(Callee::Kernel(KernelFn::FontActiveColor)),
                    ("Font", "disabledColor") => Ok(Callee::Kernel(KernelFn::FontDisabledColor)),
                    ("Font", "hoverSize") => Ok(Callee::Kernel(KernelFn::FontHoverSize)),
                    ("Attr", "tabindex") => Ok(Callee::Kernel(KernelFn::HtmlAttrTabindex)),
                    // ── M7: Std.Live / Sky.Live app-entry kernels ─────────────
                    ("Live", "app") => Ok(Callee::Kernel(KernelFn::LiveApp)),
                    ("Live", "appRouted") => Ok(Callee::Kernel(KernelFn::LiveAppRouted)),
                    ("Live", "route") => Ok(Callee::Kernel(KernelFn::LiveRoute)),
                    ("Live", "renderStatic") => Ok(Callee::Kernel(KernelFn::LiveRenderStatic)),
                    // ── M7: Std.Tui / Sky.Tui app-entry kernels ──────────────
                    ("Tui", "program") => Ok(Callee::Kernel(KernelFn::TuiProgram)),
                    ("Tui", "app") => Ok(Callee::Kernel(KernelFn::TuiApp)),
                    // ── M7: Std.Webview / Sky.Webview app-entry kernel ────────
                    ("Webview", "app") => Ok(Callee::Kernel(KernelFn::WebviewApp)),
                    // A kernel beyond the wired set.
                    // [SKY-L0108, feature: kernels]
                    (_, _) => Err(unsupported(callee.span, Feature::Kernels)),
                }
            }
            canon::Expr_::VarTopLevel { module, name } => {
                // Every `VarTopLevel` carries the defining module's path (set by
                // name resolution), and func_ids is keyed by (home_path, name)
                // so same-named defs from different modules are distinct.  A miss
                // is a violated invariant — the canonicaliser guarantees every
                // VarTopLevel references a known binding.
                let id = *self
                    .func_ids
                    .get(&(module.clone(), *name))
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
            // `/` is float-only (fdiv) — raw Rust `/` on `f64` is total
            // (x/0.0 = ±∞, never panics), so BinOp::Div stays.
            "fdiv" => Ok(BinOp::Div),
            // `//` is integer-only (idiv). Raw Rust `/` on i64 panics on
            // b==0 (DivisionByZero) AND on i64::MIN/-1 (signed overflow).
            // BinOp::IntDiv routes through the total helper
            // `sky_runtime::math::sky_int_div`, making the panicking i64-/
            // unrepresentable in the IR.
            "idiv" => Ok(BinOp::IntDiv),
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
                home,
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
                    home: ModPath(home.clone()),
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
    /// Return `true` if the expression at `span` has a `Task` type according to
    /// the HM solver's region table. Used by [`lower_let`] to decide whether a
    /// wildcard binding (`let _ = expr`) should auto-force the task via
    /// [`Expr::TaskSeq`] rather than silently dropping the unawaited future (F1).
    fn is_task_typed(&self, span: Span) -> bool {
        matches!(
            self.types.regions.get(&span),
            Some(Ty::Con { name, .. })
                if self.interner.resolve(*name).is_some_and(|n| n == "Task")
        )
    }

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
                // F1 (auto-force): `let _ = <task>` — if the discarded value is
                // Task-typed, sequence it via `TaskSeq` so the future is awaited
                // rather than silently dropped. Non-Task wildcards keep the plain
                // `Destructure(Wildcard, …)` form (which lowers to `let _ = …;`).
                canon::Pattern_::PAnything => {
                    if self.is_task_typed(b.body.span) {
                        Expr::TaskSeq {
                            effect: Box::new(value),
                            rest: Box::new(acc),
                        }
                    } else {
                        Expr::Destructure {
                            binder: self.lower_binder_pat(&b.pat, &b.body)?,
                            value: Box::new(value),
                            body: Box::new(acc),
                        }
                    }
                }
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
            let canon::Pattern_::PCtor {
                home, type_name, ..
            } = &first.pat.value
            else {
                return Err(bug(
                    "sky_lower::lower_case",
                    "all-ctor case without a ctor head",
                ));
            };
            let variants = self
                .enum_variants
                .get(&(ModPath(home.clone()), *type_name))
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
                home,
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
                    home: ModPath(home.clone()),
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use sky_canon::ast as canon;
    use sky_diagnostics::{Located, Span};
    use sky_intern::Interner;
    use sky_ir::{Callee, KernelFn};
    use sky_types::SolvedTypes;

    use super::{BuiltinCtors, Lowerer};

    // ── Registry-only allowlist ──────────────────────────────────────────────
    //
    // These variants appear in `KernelFn::ALL` (and are therefore present in
    // `stdlib_index`) but have NO legacy arm in `lower_callee`.  Passing them
    // with `id = None` hits the SKY-L0108 fallthrough → `Err(Diagnostic::Lower)`;
    // they cannot be covered by the decl-equiv-legacy test.
    //
    // EMITTABILITY VERDICT (sky_backend_rust/src/emit_expr.rs, `emit_tea_call`):
    //
    //   KernelFn::PubSubPublish       → Err(Diagnostic::CompilerBug)  [NOT emittable]
    //   KernelFn::PubSubPublishNoEcho → Err(Diagnostic::CompilerBug)  [NOT emittable]
    //
    // LOUD FINDING: PubSubPublish and PubSubPublishNoEcho are in ALL (and hence
    // in stdlib_index) but the qualifier "PubSub" is absent from QUALIFIERS in
    // env.rs, so no VarKernel node with module="PubSub" can be produced from
    // user programs.  The Phase B fast path (id = Some) CANNOT fire for them
    // in practice.  If it somehow did fire, the backend returns Err(CompilerBug)
    // — a loud failure, not silent exit-0.  Both are M6-reserved TEA primitives
    // awaiting a dedicated lowering + emission path before they are safe to move
    // to the covered set.
    const REGISTRY_ONLY_ALLOWLIST: &[KernelFn] =
        &[KernelFn::PubSubPublish, KernelFn::PubSubPublishNoEcho];

    /// Verifies that for every non-excluded variant in `KernelFn::ALL`, the
    /// legacy string-match arm in `lower_callee` returns `Callee::Kernel(sk)`
    /// when called with `id = None` (i.e. the Phase B fast path disabled).
    ///
    /// Forcing `id = None` makes the test NON-VACUOUS:
    ///
    /// * A transposed `decl()` (e.g. `HtmlRender` declares `("Html", "foo")`
    ///   instead of `("Html", "render")`) produces the wrong lookup key →
    ///   either the arm doesn't match (SKY-L0108 Err) or the wrong variant
    ///   returns (`assert_eq` fails).
    ///
    /// * A wrong legacy arm (e.g. `("Html", "render") => Callee::Kernel(Other)`)
    ///   returns the wrong `Callee::Kernel` variant → `assert_eq` fails.
    ///
    /// MECHANICAL: test keys come from `KernelFn::decl()` on the same variant,
    /// so any mismatch between `decl()` and the legacy match arm is caught
    /// automatically, with no manual list to maintain.
    #[test]
    #[allow(clippy::too_many_lines)] // exhaustive per-variant setup + loop
    fn decl_equiv_legacy_match() {
        // ── Build a minimal Lowerer ──────────────────────────────────────────
        //
        // `lower_callee` uses only `self.interner` (via `self.resolve()`).
        // All other Lowerer fields are irrelevant for this test.
        //
        // Lifetime constraint: `Lowerer::new` takes `&Interner` (immutable),
        // but `Interner::intern` requires `&mut Interner`.  Pre-intern every
        // needed string BEFORE creating the Lowerer, then take the immutable
        // borrow.

        let mut interner = Interner::new();

        // BuiltinCtor names (required by Lowerer::new to seed enum_variants).
        let maybe = interner.intern("Maybe").unwrap();
        let result = interner.intern("Result").unwrap();
        let just = interner.intern("Just").unwrap();
        let nothing = interner.intern("Nothing").unwrap();
        let ok = interner.intern("Ok").unwrap();
        let err = interner.intern("Err").unwrap();
        let sqlvalue = interner.intern("SqlValue").unwrap();
        let sqlfield = interner.intern("SqlField").unwrap();
        let sql_string = interner.intern("SqlString").unwrap();
        let sql_int = interner.intern("SqlInt").unwrap();
        let sql_float = interner.intern("SqlFloat").unwrap();
        let sql_bool = interner.intern("SqlBool").unwrap();
        let sql_bytes = interner.intern("SqlBytes").unwrap();
        let sql_time = interner.intern("SqlTime").unwrap();
        let sql_decimal = interner.intern("SqlDecimal").unwrap();
        let sql_money = interner.intern("SqlMoney").unwrap();
        let sql_null = interner.intern("SqlNull").unwrap();
        let set_field = interner.intern("SetField").unwrap();
        let omit_field = interner.intern("OmitField").unwrap();

        // Pre-intern all kernel (qualifier, name) strings in ALL order.
        // Must happen before Lowerer borrows interner immutably.
        let kern_syms: Vec<(sky_intern::Symbol, sky_intern::Symbol)> = KernelFn::ALL
            .iter()
            .map(|sk| {
                let d = sk.decl();
                let q = interner.intern(d.qualifier).unwrap();
                let n = interner.intern(d.name).unwrap();
                (q, n)
            })
            .collect();

        let builtins = BuiltinCtors {
            maybe,
            result,
            just,
            nothing,
            ok,
            err,
            sqlvalue,
            sqlfield,
            sql_string,
            sql_int,
            sql_float,
            sql_bool,
            sql_bytes,
            sql_time,
            sql_decimal,
            sql_money,
            sql_null,
            set_field,
            omit_field,
        };
        let module = canon::Module {
            name: vec![],
            unions: vec![],
            defs: vec![],
        };
        let types = SolvedTypes {
            env: BTreeMap::new(),
            regions: BTreeMap::new(),
            bounds: BTreeMap::new(),
        };

        // Immutable borrow of interner starts here — no more intern() calls.
        let lowerer = Lowerer::new(&module, &types, &interner, vec![], vec![], &builtins);

        // ── Test loop ────────────────────────────────────────────────────────
        let mut covered: usize = 0;
        let mut skipped_internal: usize = 0;
        let allowlisted: usize = REGISTRY_ONLY_ALLOWLIST.len();

        // Iterate `ALL` and its pre-interned (qualifier, name) symbols in
        // lockstep via `zip` — no raw indexing (the project bans
        // `clippy::indexing_slicing`, including in the gate itself).
        for (&sk, &(qual_sym, name_sym)) in KernelFn::ALL.iter().zip(kern_syms.iter()) {
            let decl = sk.decl();

            // Skip internal variants (qualifier starts with '_').
            if decl.qualifier.starts_with('_') {
                skipped_internal += 1;
                continue;
            }

            // Skip registry-only variants — they have no legacy arm.
            if REGISTRY_ONLY_ALLOWLIST.contains(&sk) {
                continue;
            }

            // Force the legacy path by setting id = None.
            let node = Located::new(
                Span::DUMMY,
                canon::Expr_::VarKernel {
                    id: None,
                    module: qual_sym,
                    name: name_sym,
                },
            );

            // A single `assert_eq!` on the `Result` (via `.ok()`) catches BOTH
            // failure modes without `panic!`/`unwrap`:
            //   * Err (missing legacy arm / transposed decl) → `None` != `Some(..)`
            //   * wrong variant returned                     → `Some(other)` != `Some(sk)`
            let got = lowerer.lower_callee(&node).ok();
            assert_eq!(
                got,
                Some(Callee::Kernel(sk)),
                "lower_callee(id=None, qualifier={:?}, name={:?}) returned {got:?}; \
                 expected Some(Callee::Kernel(KernelFn::{sk:?})). Either the legacy \
                 arm is missing / maps to the wrong variant, or decl() returned the \
                 wrong canonical (qualifier, name) for this variant.",
                decl.qualifier,
                decl.name,
            );

            covered += 1;
        }

        // Sanity: every variant must be accounted for.
        let total = KernelFn::ALL.len();
        assert_eq!(
            covered + allowlisted + skipped_internal,
            total,
            "variant accounting mismatch: \
             covered={covered} + allowlisted={allowlisted} + \
             skipped_internal={skipped_internal} != total={total}",
        );
    }
}
