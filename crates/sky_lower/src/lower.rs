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
    is_dispatch_free, is_irrefutable, Arm, BinOp, BoundSet, Callee, EnumDef, Expr, Func, FuncId,
    IrType, KernelFn, Match, ModPath, Module, Pat, Program, TypeDef, UiCtor, UiPlain, Variant,
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
        Ty::Record(fields, _) => fields.values().any(ty_contains_var),
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
        Ty::Record(fields, _) => fields.values().any(ty_contains_fun),
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

/// The built-in COLLECTION type constructors (`List`/`Dict`/`Set`), whose Rust
/// rendering (`Vec<T>` / `HashMap<K,V>` / `BTreeSet<T>`) is a container the
/// kernels (`DictGet`, `ListMap`, …) blanket-`.clone()` their element/value
/// argument (#90 design doc §2 hazard table: "collections of functions" stays
/// a real gap). A function type argument here is NOT the sound
/// enum-constructor-payload shape [`is_enum_like_con_head`] exempts — kept
/// gated (`ty_contains_fun`) by [`embeds_nonderivable_function`]'s fallback arm.
fn is_builtin_collection(interner: &Interner, name: Symbol) -> bool {
    matches!(interner.resolve(name), Some("List" | "Dict" | "Set"))
}

/// Is this `Ty::Con` head an ENUM-LIKE constructor — the built-in `Maybe` /
/// `Result` or a user-declared union — as opposed to a builtin COLLECTION
/// (`List`/`Dict`/`Set`) or an opaque boxed wrapper?
///
/// #90 (SKY-L0114 narrowing): `Ok f` / `Just f` construct the RUNTIME
/// `SkyResult`/`SkyMaybe` enums, whose derives are generic-bounded
/// (`impl<T: Clone> Clone for SkyMaybe<T>`, `runtime/src/sky_runtime/core.rs`)
/// — the TYPE `SkyMaybe<Box<dyn Fn(..)->R>>` compiles regardless of whether
/// `T` satisfies the bound; only *using* `.clone()`/`==`/stringify on it would
/// fail, and each such use is independently gated (type-checker's
/// `ty_is_equatable`, the #91 Model gate, #93's serde-derive gate). A
/// user-declared union enjoys the same shape after the #87 derive-demotion
/// fixpoint (`enum_is_derivable` drops the auto-derive when a payload embeds a
/// function). So a function argument directly under an enum-like head is
/// SOUND to lower — [`is_opaque_boxed_wrapper`] callers already exempt the
/// truly-opaque carriers; this exempts the enum-shaped ones too.
///
/// A COLLECTION head (`List (a -> b)`, …) is excluded: the emitted `Vec<T>` /
/// `HashMap<K,V>` / `BTreeSet<T>` element type is real Rust generic
/// instantiation, and several collection kernels blanket-`.clone()` their
/// element (`DictGet`, `emit_expr.rs`) — E0599 on a non-`Clone`
/// `Box<dyn Fn>` element. Kept gated (Stage 2 territory, not #90).
fn is_enum_like_con_head(interner: &Interner, name: Symbol) -> bool {
    !is_opaque_boxed_wrapper(interner, name) && !is_builtin_collection(interner, name)
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
        // #90: an ENUM-LIKE head (built-in `Maybe`/`Result` or a user union) —
        // the runtime/derive machinery already tolerates a function argument
        // directly under it (see `is_enum_like_con_head`); only recurse for a
        // NESTED non-derivable carrier under the argument (e.g. a `List (a->b)`
        // buried inside `Maybe (List (Int -> Int))`), never flag a bare
        // function argument itself.
        Ty::Con { name, args, .. } if is_enum_like_con_head(interner, *name) => args
            .iter()
            .any(|a| embeds_nonderivable_function(interner, a)),
        // A builtin COLLECTION head (`List`/`Dict`/`Set`): unchanged blanket
        // check — a function element/value type is still the real gap (#90
        // design doc §2, "collections of functions").
        Ty::Con { args, .. } => args
            .iter()
            .any(|a| ty_contains_fun(a) || embeds_nonderivable_function(interner, a)),
        Ty::Record(fields, _) => {
            // Exempt the anonymous `RetryPolicy e` record — a kernel-managed type
            // whose emitter writes a dedicated non-derivable Rust struct.  Identified
            // by the presence of a `shouldRetry` key: no other stdlib or user record
            // carries that name.  A user who literally writes `{ shouldRetry = \_ ->
            // True, … }` still trips `reject_function_valued_field` at the field-value
            // level before reaching this path (record literals skip this gate).
            if fields
                .keys()
                .any(|k| interner.resolve(*k) == Some("shouldRetry"))
            {
                return false;
            }
            fields
                .values()
                .any(|f| ty_contains_fun(f) || embeds_nonderivable_function(interner, f))
        }
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
        // `StreamWriter` is an opaque stream handle — not a function type.
        | IrType::StreamWriter
        // `HttpRequest` is an opaque handle — not a function type.
        | IrType::HttpRequest
        // #127: `WsHandle` / `WsServerCfg` are opaque handles — not function types.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        | IrType::Generic(_)
        // M7: nullary plain types (`Length`, `Color`, etc.) trivially contain no
        // functions.  `LiveReq` is an opaque handle with no `Fn` fields.
        | IrType::UiPlain(_)
        | IrType::LiveReq
        // `Order` (LT/EQ/GT) is a primitive leaf — no embedded function.
        // `Decimal` is a Copy newtype — no embedded function.
        // `ErrorKind`/`Error`/`ErrorDetails` and the nominal error-payload
        // leaves (`ErrorInfo`/`PanicInfo`/`TypeInfo`, SEAL fix 2026-07-11)
        // are leaves — no embedded function.
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        // `SqlFragment` is an opaque query-building value — no embedded function.
        // `Secret` is an opaque sealed string wrapper — no embedded function.
        | IrType::SqlFragment
        | IrType::Secret => false,
        // `LiveRoute page` carries the page type it builds — recurse (the
        // route's own builder closure is runtime-internal, not a Sky `Fn`).
        IrType::LiveRoute(page) => ir_contains_fun(page),
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

// ── Capture-clone classification (#121) ──────────────────────────────────────
//
// Classifies an `IrType` for the capture-clone rewrite that makes closures
// `Fn` (not `FnOnce`). Rules:
//   CopyLeaf  — scalar types that are `Copy`; reads are bare moves (copies).
//   CloneOk   — types that derive `Clone` in the runtime; reads inside closures
//               must use `{name}.clone()` so the closure is re-callable.
//   NonClone  — types that do NOT implement `Clone` (functions, tasks, decoders,
//               server opaques, …); capturing one in a non-callee position is
//               a SKY-L0125 diagnostic.
//
// This is conservative: when unsure → `NonClone` (fail-closed, never a
// silent cargo failure).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CloneClass {
    CopyLeaf,
    CloneOk,
    NonClone,
}

fn clone_class(t: &IrType) -> CloneClass {
    match t {
        // Scalars — primitive Copy types.
        // `Decimal` is `#[derive(Copy)]` — treat as CopyLeaf.
        IrType::Int | IrType::Float | IrType::Bool | IrType::Char | IrType::Unit | IrType::Order | IrType::Decimal | IrType::ErrorKind => {
            CloneClass::CopyLeaf
        }
        // Runtime-verified Clone types.
        // Str(String), Bytes(Vec<u8>), Json(serde_json::Value), Db(Arc-backed),
        // UiPlain (element.rs derives Clone), LiveReq (req.rs derives Clone).
        // Error: SkyError derives Clone (not Copy — carries a heap `String`).
        // ErrorDetails: SkyErrorDetails derives Clone (not Copy — carries
        // heap-allocated `String`/`Vec<String>` payloads; backlog #85
        // follow-up).
        // `SqlFragment` is `#[derive(Clone, PartialEq)]` (no Copy — carries a
        // heap-allocated `String` + `Vec<SqlParam>`).
        // `Secret` is `#[derive(Clone)]` (no Copy — carries a heap-allocated
        // `String`; hand-written `PartialEq`, not derived — see its own doc).
        // The nominal error-payload types derive Clone (not Copy — each
        // carries heap-allocated `String`s; SEAL fix 2026-07-11).
        IrType::Str | IrType::Bytes | IrType::Json | IrType::Db | IrType::UiPlain(_) | IrType::LiveReq | IrType::Error | IrType::ErrorDetails | IrType::ErrorInfo | IrType::PanicInfo | IrType::TypeInfo | IrType::SqlFragment | IrType::Secret => {
            CloneClass::CloneOk
        }
        // Runtime-verified Clone server/http opaques (audited 2026-07-05):
        // ServerRequest/ServerResponse/ServerCookie (server.rs:33/50/59),
        // ServerRoute (server.rs:136), HttpRequest (http_client.rs:64) all
        // `#[derive(Clone, …)]`.
        // #127: `WsServerCfg` holds Arc<dyn Fn> callbacks — Clone via Arc.
        IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        | IrType::HttpRequest
        | IrType::WebSocketServerCfg => CloneClass::CloneOk,
        // StreamWriter is `#[derive(Clone, Copy)]` — an i64 id wrapper
        // (server_stream.rs:38). Bare capture is sound.
        // #127: `WsHandle` is `#[derive(Clone, Copy)]` — an i64 id wrapper.
        IrType::StreamWriter | IrType::WebSocketServer => CloneClass::CopyLeaf,
        // Non-Clone: function-typed, task, decoder, Cmd, Sub.
        // Also Generic(_) until T5 (which injects `T: Clone`).
        IrType::Fun(_, _)
        | IrType::Task(_)
        | IrType::Decoder(_)
        | IrType::Cmd(_)
        | IrType::Sub(_)
        | IrType::Generic(_) => CloneClass::NonClone,
        // Composite: CloneOk iff all components CloneOk (no NonClone part).
        // `Maybe`, `List`, `Set`, `Result`, `Dict` are NAMED Rust types
        // (`SkyMaybe<T>`, `Vec<T>`, `BTreeSet<T>`, `SkyResult<E,A>`,
        // `HashMap<K,V>`) — they never implement `Copy` even when every element
        // is `Copy`. Use `clone_class_named_composite` to floor `CopyLeaf` → `CloneOk`
        // so T5 inserts `.clone()` for multi-use bindings (e.g. `Vec<i64>`).
        IrType::Maybe(elem) | IrType::List(elem) | IrType::Set(elem) => {
            clone_class_named_composite(std::iter::once(elem.as_ref()))
        }
        IrType::Result(e, a) | IrType::Dict(e, a) => {
            clone_class_named_composite([e.as_ref(), a.as_ref()].into_iter())
        }
        IrType::Tuple(elems) => clone_class_composite(elems.iter()),
        // Named types: emitted Rust struct/enum derives `Clone` but NOT `Copy`.
        // A CopyLeaf payload (e.g. all-Int record, no-arg enum) does NOT make the
        // wrapper `Copy` — bare capture would move it on first closure call → E0525.
        // Floor to CloneOk so the rewrite inserts `.clone()` per call.
        IrType::Record(fields) => clone_class_named_composite(fields.values()),
        IrType::Enum { args, .. } => clone_class_named_composite(args.iter()),
        // Ui{msg} / LiveRoute(page) — recurse on the message/page type-param.
        IrType::Ui { msg, .. } => clone_class_composite(std::iter::once(msg.as_ref())),
        IrType::LiveRoute(page) => clone_class_composite(std::iter::once(page.as_ref())),
    }
}

fn clone_class_composite<'a>(parts: impl Iterator<Item = &'a IrType>) -> CloneClass {
    let mut any_clone_ok = false;
    for p in parts {
        match clone_class(p) {
            CloneClass::NonClone => return CloneClass::NonClone,
            CloneClass::CloneOk => any_clone_ok = true,
            CloneClass::CopyLeaf => {}
        }
    }
    if any_clone_ok {
        CloneClass::CloneOk
    } else {
        CloneClass::CopyLeaf
    }
}

/// Like [`clone_class_composite`] but floors `CopyLeaf` to `CloneOk`.
///
/// Use for **named Rust types** (emitted `struct` / `enum`) that derive `Clone`
/// but **not** `Copy`.  A payload of all-scalar fields makes
/// `clone_class_composite` return `CopyLeaf`, falsely claiming the wrapper is
/// `Copy`.  Bare capture of such a type inside a `move` closure moves the value
/// on first call, causing E0525 on any subsequent call.  Flooring to `CloneOk`
/// ensures the rewrite inserts `.clone()` per call — safe because the wrapper
/// derives `Clone`.
fn clone_class_named_composite<'a>(parts: impl Iterator<Item = &'a IrType>) -> CloneClass {
    match clone_class_composite(parts) {
        CloneClass::NonClone => CloneClass::NonClone,
        // CopyLeaf is only valid for Rust primitive types that implement `Copy`.
        // Named structs and enums never derive `Copy` (derive macro doesn't emit it),
        // so the weakest safe class here is `CloneOk`.
        CloneClass::CloneOk | CloneClass::CopyLeaf => CloneClass::CloneOk,
    }
}

// ── Capture-clone rewrite helpers, T3 (#121) ─────────────────────────────────
//
// Three helpers drive the capture-clone rewrite that makes closures `Fn`
// (not `FnOnce`):
//
//   (a) `canon_collect_pat_binds` — collect all VarLocal-binding symbols from
//       a canon pattern (to build the outer-bound set before the free-variable
//       walk).
//   (b) `canon_collect_free_locals` — recursively walk a canon expression
//       collecting VarLocal occurrences free relative to `bound`; shadows from
//       let / case / inner-lambda binders are handled.
//   (c) `rewrite_captured_clones` — given a lowered IR body and the classified
//       capture sets, replace `Var` reads of `CloneOk` captures with
//       `CloneVar` (`.clone()`) and return SKY-L0125 for `NonClone` captures
//       outside direct callee position.
//
// `Lowerer::captured_locals` drives (a)+(b) and returns the classified
// capture list consumed by (c).

/// Collect all symbols bound by `pat` into `bound`.
fn canon_collect_pat_binds(pat: &canon::Pattern, bound: &mut BTreeSet<Symbol>) {
    match &pat.value {
        canon::Pattern_::PVar(s) => {
            bound.insert(*s);
        }
        canon::Pattern_::PAnything
        | canon::Pattern_::PInt(_)
        | canon::Pattern_::PBool(_)
        | canon::Pattern_::PChar(_)
        | canon::Pattern_::PStr(_) => {}
        canon::Pattern_::PCtor { args, .. } => {
            for a in args {
                canon_collect_pat_binds(a, bound);
            }
        }
        canon::Pattern_::PTuple(elems) | canon::Pattern_::PList(elems) => {
            for e in elems {
                canon_collect_pat_binds(e, bound);
            }
        }
        canon::Pattern_::PRecord(fields) => {
            for f in fields {
                bound.insert(f.value);
            }
        }
        canon::Pattern_::PAlias(inner, alias) => {
            canon_collect_pat_binds(inner, bound);
            bound.insert(alias.value);
        }
        canon::Pattern_::PCons(head, tail) => {
            canon_collect_pat_binds(head, bound);
            canon_collect_pat_binds(tail, bound);
        }
    }
}

/// Walk `expr` collecting `VarLocal` symbols free relative to `bound`.
/// Records each free symbol's first-seen use-site span (for region-type
/// lookup by [`Lowerer::captured_locals`]).
///
/// Shadow discipline: `Let` bindings accumulate names sequentially before the
/// continuation body; `Case` arm patterns shadow inside that arm; inner
/// `Lambda` params shadow inside that lambda body.
fn canon_collect_free_locals(
    free: &mut BTreeMap<Symbol, Span>,
    bound: &BTreeSet<Symbol>,
    expr: &canon::Expr,
) {
    match &expr.value {
        canon::Expr_::VarLocal(s) => {
            if !bound.contains(s) {
                free.entry(*s).or_insert(expr.span);
            }
        }
        canon::Expr_::Let(bindings, body) => {
            let mut inner = bound.clone();
            for b in bindings {
                canon_collect_free_locals(free, &inner, &b.body);
                canon_collect_pat_binds(&b.pat, &mut inner);
            }
            canon_collect_free_locals(free, &inner, body);
        }
        canon::Expr_::Lambda(params, body) => {
            let mut inner = bound.clone();
            for p in params {
                canon_collect_pat_binds(p, &mut inner);
            }
            canon_collect_free_locals(free, &inner, body);
        }
        canon::Expr_::Case(scrut, arms) => {
            canon_collect_free_locals(free, bound, scrut);
            for arm in arms {
                let mut arm_bound = bound.clone();
                canon_collect_pat_binds(&arm.pat, &mut arm_bound);
                canon_collect_free_locals(free, &arm_bound, &arm.body);
            }
        }
        canon::Expr_::Call(callee, args) => {
            canon_collect_free_locals(free, bound, callee);
            for a in args {
                canon_collect_free_locals(free, bound, a);
            }
        }
        canon::Expr_::Binop { lhs, rhs, .. } => {
            canon_collect_free_locals(free, bound, lhs);
            canon_collect_free_locals(free, bound, rhs);
        }
        canon::Expr_::If(branches, else_) => {
            for (cond, then) in branches {
                canon_collect_free_locals(free, bound, cond);
                canon_collect_free_locals(free, bound, then);
            }
            canon_collect_free_locals(free, bound, else_);
        }
        canon::Expr_::Tuple(elems) | canon::Expr_::List(elems) => {
            for e in elems {
                canon_collect_free_locals(free, bound, e);
            }
        }
        canon::Expr_::Cons(h, t) => {
            canon_collect_free_locals(free, bound, h);
            canon_collect_free_locals(free, bound, t);
        }
        canon::Expr_::Record(fields) => {
            for (_, v) in fields {
                canon_collect_free_locals(free, bound, v);
            }
        }
        canon::Expr_::Access(rec, _) => canon_collect_free_locals(free, bound, rec),
        canon::Expr_::Update(base, fields) => {
            canon_collect_free_locals(free, bound, base);
            for (_, v) in fields {
                canon_collect_free_locals(free, bound, v);
            }
        }
        // Non-local-variable leaves — no free locals.
        canon::Expr_::VarTopLevel { .. }
        | canon::Expr_::VarKernel { .. }
        | canon::Expr_::VarCtor { .. }
        | canon::Expr_::Int(_)
        | canon::Expr_::Float(_)
        | canon::Expr_::Str(_)
        | canon::Expr_::Char(_)
        | canon::Expr_::Unit => {}
    }
}

/// Does `pat` bind ANY symbol in `set`?
#[inline]
fn pat_binds_any_in(pat: &Pat, set: &BTreeSet<Symbol>) -> bool {
    set.iter().any(|&s| pat_binds_symbol(pat, s))
}

/// Rewrite a lowered IR expression — the body of a `move` closure — to make
/// the closure `Fn` (not `FnOnce`) by inserting `.clone()` calls on captures
/// that are not `Copy`:
///
/// * `Var(s)` where `s ∈ clone_set` → `CloneVar(s)` (runtime `.clone()`)
/// * `Var(s)` where `s ∈ noncl_set` AND `s` is the DIRECT callee of an
///   `Apply` → kept bare (`Fn::call` borrows the receiver — verified green)
/// * `Var(s)` where `s ∈ noncl_set` elsewhere → `Err(SKY-L0125)`
/// * all others → unchanged (not captured, or `CopyLeaf`)
///
/// Shadow discipline mirrors [`rewrite_var_to_apply`]: `Let` / `Destructure`
/// / `Lambda` / `Match`-arm patterns rebind and remove the symbol from the
/// active sets inside the shadowed sub-expression.
#[allow(clippy::too_many_lines)]
// `depth`: closure-nesting depth relative to the outermost lambda being
// processed.  Used to gate the NonClone callee-position exemption: at depth 0
// a `Var(f)` in direct `Apply.func` position is allowed bare (Rust borrows
// `&self` for `Fn::call`).  At depth > 0 the symbol is captured by an inner
// `move` closure which steals it from the outer env on the first call →
// outer closure becomes `FnOnce` (E0525).  The exemption is therefore only
// sound at depth 0.
fn rewrite_captured_clones(
    clone_set: &BTreeSet<Symbol>,
    noncl_set: &BTreeSet<Symbol>,
    lambda_span: Span,
    expr: Expr,
    depth: u32,
) -> DResult<Expr> {
    if clone_set.is_empty() && noncl_set.is_empty() {
        return Ok(expr);
    }
    match expr {
        Expr::Var(s) => {
            if clone_set.contains(&s) {
                Ok(Expr::CloneVar(s))
            } else if noncl_set.contains(&s) {
                Err(unsupported(lambda_span, Feature::NonCloneCapture))
            } else {
                Ok(Expr::Var(s))
            }
        }
        // Leaves that are never local captures.
        Expr::CloneVar(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => Ok(expr),
        // Apply: a `Var(s)` in DIRECT func position where `s ∈ noncl_set`
        // is allowed bare ONLY at depth 0.  At depth 0, Rust's `Fn::call`
        // borrows `&self`, so re-calling the closure is safe.
        // At depth > 0 the symbol lives inside an inner `move` closure: the
        // inner closure would move it out of the outer env on the first call,
        // making the outer closure `FnOnce` (E0525).
        //
        // Args discipline (#151): a lambda that appears as a CALLBACK ARGUMENT
        // (e.g. `task_and_then(task, \ts -> insertRow db ts)`) has already been
        // fully processed by its own `lower_lambda` pass at depth 0, including
        // the callee-position exemption for NonClone symbols.  Propagating
        // `noncl_set` into arg-position lambdas here would re-examine already-
        // handled callee sites at depth+1, where the exemption does NOT fire,
        // spuriously emitting L0126.
        //
        // Lambdas in FUNC position (immediately-invoked pattern
        // `(\x -> f x) p`) are NOT cleared: the inner lambda creation moves a
        // NonClone value out of the outer env on every call → outer closure
        // becomes FnOnce against a `Box<dyn Fn>` return annotation → Rust E0277
        // (i130 c14 gate).  Those still propagate `noncl_set` via the normal
        // `other` path into the `Lambda` arm.
        Expr::Apply { func, args } => {
            let new_func = Box::new(match *func {
                Expr::Var(s) if noncl_set.contains(&s) && depth == 0 => Expr::Var(s),
                other => rewrite_captured_clones(clone_set, noncl_set, lambda_span, other, depth)?,
            });
            let new_args = args
                .into_iter()
                .map(|a| {
                    // Clear `noncl_set` for lambda arguments — they are
                    // already self-consistent from their own `lower_lambda`
                    // pass.  Non-lambda expressions keep the full `noncl_set`
                    // so forwarding a NonClone value in arg position (e.g.
                    // `applyTwice f x` where `f` is non-callee) still fires
                    // L0126 as expected.
                    if matches!(&a, Expr::Lambda { .. }) {
                        let empty = BTreeSet::new();
                        rewrite_captured_clones(clone_set, &empty, lambda_span, a, depth)
                    } else {
                        rewrite_captured_clones(clone_set, noncl_set, lambda_span, a, depth)
                    }
                })
                .collect::<DResult<Vec<_>>>()?;
            Ok(Expr::Apply { func: new_func, args: new_args })
        }
        Expr::BinOp { op, lhs, rhs } => Ok(Expr::BinOp {
            op,
            lhs: Box::new(rewrite_captured_clones(clone_set, noncl_set, lambda_span, *lhs, depth)?),
            rhs: Box::new(rewrite_captured_clones(clone_set, noncl_set, lambda_span, *rhs, depth)?),
        }),
        Expr::Let { name, value, body } => {
            let new_value =
                Box::new(rewrite_captured_clones(clone_set, noncl_set, lambda_span, *value, depth)?);
            if clone_set.contains(&name) || noncl_set.contains(&name) {
                let inner_clone: BTreeSet<Symbol> =
                    clone_set.iter().copied().filter(|&s| s != name).collect();
                let inner_noncl: BTreeSet<Symbol> =
                    noncl_set.iter().copied().filter(|&s| s != name).collect();
                Ok(Expr::Let {
                    name,
                    value: new_value,
                    body: Box::new(rewrite_captured_clones(
                        &inner_clone,
                        &inner_noncl,
                        lambda_span,
                        *body,
                        depth,
                    )?),
                })
            } else {
                Ok(Expr::Let {
                    name,
                    value: new_value,
                    body: Box::new(rewrite_captured_clones(
                        clone_set,
                        noncl_set,
                        lambda_span,
                        *body,
                        depth,
                    )?),
                })
            }
        }
        Expr::Destructure { binder, value, body } => {
            let new_value =
                Box::new(rewrite_captured_clones(clone_set, noncl_set, lambda_span, *value, depth)?);
            if pat_binds_any_in(&binder, clone_set) || pat_binds_any_in(&binder, noncl_set) {
                let inner_clone: BTreeSet<Symbol> = clone_set
                    .iter()
                    .copied()
                    .filter(|&s| !pat_binds_symbol(&binder, s))
                    .collect();
                let inner_noncl: BTreeSet<Symbol> = noncl_set
                    .iter()
                    .copied()
                    .filter(|&s| !pat_binds_symbol(&binder, s))
                    .collect();
                Ok(Expr::Destructure {
                    binder,
                    value: new_value,
                    body: Box::new(rewrite_captured_clones(
                        &inner_clone,
                        &inner_noncl,
                        lambda_span,
                        *body,
                        depth,
                    )?),
                })
            } else {
                Ok(Expr::Destructure {
                    binder,
                    value: new_value,
                    body: Box::new(rewrite_captured_clones(
                        clone_set,
                        noncl_set,
                        lambda_span,
                        *body,
                        depth,
                    )?),
                })
            }
        }
        // Lambda: its own params shadow for the body.
        //
        // `noncl_set` IS propagated into inner lambda bodies (at depth+1) so
        // that the depth > 0 gate can fire for the immediately-invoked pattern
        // `(\x -> f x) p` (i130 c14): that inner `\x -> f x` is in `Apply.func`
        // position and reaches this arm via the normal `other` path.  At depth 1
        // the callee-position exemption (`depth == 0`) does NOT fire, so
        // `Var(f)` inside the inner body triggers L0126 — correctly preventing
        // a `FnOnce` closure from being boxed as `Box<dyn Fn>`.
        //
        // The companion case — lambdas in ARGUMENT position such as
        // `task_and_then(task, \ts -> insertRow db ts)` — is handled one level
        // up in the `Apply` arm: arg-position lambdas receive an empty
        // `noncl_set` before entering this arm, so `inner_noncl` below is
        // already empty and no spurious L0126 is emitted (#151).
        Expr::Lambda { params, ret, body } => {
            let param_names: BTreeSet<Symbol> = params.iter().map(|(s, _)| *s).collect();
            let inner_clone: BTreeSet<Symbol> =
                clone_set.iter().copied().filter(|s| !param_names.contains(s)).collect();
            let inner_noncl: BTreeSet<Symbol> =
                noncl_set.iter().copied().filter(|s| !param_names.contains(s)).collect();
            Ok(Expr::Lambda {
                params,
                ret,
                body: Box::new(rewrite_captured_clones(
                    &inner_clone,
                    &inner_noncl,
                    lambda_span,
                    *body,
                    depth + 1,
                )?),
            })
        }
        Expr::Match(m) => {
            let (scrutinee, arms) = m.into_parts();
            let new_scrutinee = Box::new(rewrite_captured_clones(
                clone_set,
                noncl_set,
                lambda_span,
                *scrutinee,
                depth,
            )?);
            let new_arms = arms
                .into_iter()
                .map(|arm| {
                    let new_body =
                        if pat_binds_any_in(&arm.pat, clone_set)
                            || pat_binds_any_in(&arm.pat, noncl_set)
                        {
                            let inner_clone: BTreeSet<Symbol> = clone_set
                                .iter()
                                .copied()
                                .filter(|&s| !pat_binds_symbol(&arm.pat, s))
                                .collect();
                            let inner_noncl: BTreeSet<Symbol> = noncl_set
                                .iter()
                                .copied()
                                .filter(|&s| !pat_binds_symbol(&arm.pat, s))
                                .collect();
                            rewrite_captured_clones(
                                &inner_clone,
                                &inner_noncl,
                                lambda_span,
                                arm.body,
                                depth,
                            )?
                        } else {
                            rewrite_captured_clones(
                                clone_set,
                                noncl_set,
                                lambda_span,
                                arm.body,
                                depth,
                            )?
                        };
                    Ok(Arm { pat: arm.pat, body: new_body, guard: arm.guard })
                })
                .collect::<DResult<Vec<_>>>()?;
            Ok(Expr::Match(Match::from_parts_unchecked(new_scrutinee, new_arms)))
        }
        Expr::If { cond, then_, else_ } => Ok(Expr::If {
            cond: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *cond, depth,
            )?),
            then_: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *then_, depth,
            )?),
            else_: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *else_, depth,
            )?),
        }),
        // Call: kernel / top-level function application.
        //
        // Same Lambda-in-args discipline as `Expr::Apply` (#151): a lambda
        // passed as a callback to a kernel (e.g. `List.map (\m -> f m) xs` or
        // `task_and_then(task, \ts -> insertRow db ts)`) is already fully
        // processed by its own `lower_lambda` pass at depth 0.  Propagating
        // `noncl_set` into it here would fire spurious L0126 at depth+1.
        //
        // Non-lambda args keep the full `noncl_set` so forwarding a NonClone
        // value in arg position (e.g. `applyTwice f x` where `f` is non-callee)
        // is still rejected.
        Expr::Call { callee, args } => Ok(Expr::Call {
            callee,
            args: args
                .into_iter()
                .map(|a| {
                    if matches!(&a, Expr::Lambda { .. }) {
                        let empty = BTreeSet::new();
                        rewrite_captured_clones(clone_set, &empty, lambda_span, a, depth)
                    } else {
                        rewrite_captured_clones(clone_set, noncl_set, lambda_span, a, depth)
                    }
                })
                .collect::<DResult<Vec<_>>>()?,
        }),
        Expr::Tuple(items) => Ok(Expr::Tuple(
            items
                .into_iter()
                .map(|e| rewrite_captured_clones(clone_set, noncl_set, lambda_span, e, depth))
                .collect::<DResult<Vec<_>>>()?,
        )),
        Expr::List { elem, items } => Ok(Expr::List {
            elem,
            items: items
                .into_iter()
                .map(|e| rewrite_captured_clones(clone_set, noncl_set, lambda_span, e, depth))
                .collect::<DResult<Vec<_>>>()?,
        }),
        Expr::Cons { head, tail } => Ok(Expr::Cons {
            head: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *head, depth,
            )?),
            tail: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *tail, depth,
            )?),
        }),
        Expr::ListIndexClone { list, index } => Ok(Expr::ListIndexClone {
            list: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *list, depth,
            )?),
            index,
        }),
        Expr::ListLenCheck { list, len, exact } => Ok(Expr::ListLenCheck {
            list: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *list, depth,
            )?),
            len,
            exact,
        }),
        Expr::Record(fields) => Ok(Expr::Record(
            fields
                .into_iter()
                .map(|(sym, e)| {
                    rewrite_captured_clones(clone_set, noncl_set, lambda_span, e, depth)
                        .map(|e| (sym, e))
                })
                .collect::<DResult<Vec<_>>>()?,
        )),
        Expr::Access { record, field } => Ok(Expr::Access {
            record: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *record, depth,
            )?),
            field,
        }),
        Expr::Update { record, fields } => Ok(Expr::Update {
            record: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *record, depth,
            )?),
            fields: fields
                .into_iter()
                .map(|(sym, e)| {
                    rewrite_captured_clones(clone_set, noncl_set, lambda_span, e, depth)
                        .map(|e| (sym, e))
                })
                .collect::<DResult<Vec<_>>>()?,
        }),
        Expr::TaskSeq { effect, rest } => Ok(Expr::TaskSeq {
            effect: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *effect, depth,
            )?),
            rest: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *rest, depth,
            )?),
        }),
        Expr::TaskSeqSync { effect, rest } => Ok(Expr::TaskSeqSync {
            effect: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *effect, depth,
            )?),
            rest: Box::new(rewrite_captured_clones(
                clone_set, noncl_set, lambda_span, *rest, depth,
            )?),
        }),
        Expr::Ctor { home, ty, variant, args } => Ok(Expr::Ctor {
            home,
            ty,
            variant,
            args: args
                .into_iter()
                .map(|a| rewrite_captured_clones(clone_set, noncl_set, lambda_span, a, depth))
                .collect::<DResult<Vec<_>>>()?,
        }),
        // TailLoop/TailRecur are produced by a post-lower TCO pass that runs
        // AFTER lower_lambda — they cannot appear inside a lambda body at this
        // point. Handle defensively: TailLoop params shadow; TailRecur recurse.
        // TailLoop is NOT a new closure scope — do NOT increment depth here.
        Expr::TailLoop { params, body } => {
            let param_names: BTreeSet<Symbol> = params.iter().map(|(s, _)| *s).collect();
            let inner_clone: BTreeSet<Symbol> =
                clone_set.iter().copied().filter(|s| !param_names.contains(s)).collect();
            let inner_noncl: BTreeSet<Symbol> =
                noncl_set.iter().copied().filter(|s| !param_names.contains(s)).collect();
            Ok(Expr::TailLoop {
                params,
                body: Box::new(rewrite_captured_clones(
                    &inner_clone,
                    &inner_noncl,
                    lambda_span,
                    *body,
                    depth,
                )?),
            })
        }
        Expr::TailRecur { args } => Ok(Expr::TailRecur {
            args: args
                .into_iter()
                .map(|a| rewrite_captured_clones(clone_set, noncl_set, lambda_span, a, depth))
                .collect::<DResult<Vec<_>>>()?,
        }),
    }
}

// ── Multi-use-clone rewrite, T5 (#104 / #112) ────────────────────────────────
//
// A `CloneOk` local used more than once in a BY-VALUE consuming position causes
// the Rust backend to emit bare identifier moves for each occurrence.  In Rust,
// only the FIRST move is valid; subsequent reads of a moved value are E0382.
//
// Fix: when a `let`-binding or function parameter of `CloneOk` type is used
// N > 1 times in its scope, insert `.clone()` on all but the syntactically LAST
// occurrence (DFS left-to-right order).  The last occurrence stays bare — it
// is the "real" final consume.  Over-cloning is acceptable (conservatism);
// a precision pass can follow once the correctness seal holds.
//
// Sub-class #112 — Lambda captures that are also used after the closure:
// A `move` closure captures a `CloneOk` local by value (moving it) even when
// the lambda body only ever reads a `.clone()` of it (T3).  If the same local
// is referenced again AFTER the lambda creation, Rust sees E0382 because the
// move already happened.
//
// Fix: when a Lambda expression that move-captures `sym` is NOT the last use,
// pre-clone `sym` before it:
//   `let sym = sym.clone() in Lambda { body still using CloneVar(sym) }`
// The inner rebinding shadows the outer `sym`; the `move` closure captures the
// CLONE (inner sym); the outer `sym` remains alive for subsequent uses.
//
// Three helpers implement this:
//   `lambda_body_refs_sym` — does a Lambda body reference `sym` (directly or
//       via nested lambdas, respecting inner-lambda-param shadowing)?
//   `count_var_uses`       — count consuming occurrences of `sym` in expr,
//       counting a Lambda whose body refs sym as one occurrence.
//   `rewrite_multiuse_clones` — DFS rewrite; `remaining` starts at the count,
//       decrements per occurrence; last occurrence (remaining==1) stays bare.

/// Returns `true` when `expr` references `sym` as a live (non-shadowed) local
/// at any depth, treating nested lambdas as transparent — descending into their
/// bodies unless one of their parameters re-binds `sym`.
///
/// Used to decide whether a `move` closure whose body is `expr` will capture
/// `sym` from the enclosing scope: if this returns `true`, the outer `move`
/// closure moves `sym` in.
fn lambda_body_refs_sym(sym: Symbol, expr: &Expr) -> bool {
    match expr {
        Expr::Var(s) | Expr::CloneVar(s) => *s == sym,
        // Nested lambda: descend unless it shadows `sym` via a parameter.
        // A nested `move` closure that captures `sym` causes the outer closure
        // to capture `sym` too.
        Expr::Lambda { params, body, .. } => {
            if params.iter().any(|(s, _)| *s == sym) {
                false // sym shadowed by inner lambda param
            } else {
                lambda_body_refs_sym(sym, body)
            }
        }
        Expr::Let { name, value, body } => {
            lambda_body_refs_sym(sym, value)
                || if *name == sym { false } else { lambda_body_refs_sym(sym, body) }
        }
        Expr::Destructure { binder, value, body } => {
            lambda_body_refs_sym(sym, value)
                || if pat_binds_symbol(binder, sym) {
                    false
                } else {
                    lambda_body_refs_sym(sym, body)
                }
        }
        Expr::Match(m) => {
            lambda_body_refs_sym(sym, m.scrutinee())
                || m.arms().iter().any(|arm| {
                    !pat_binds_symbol(&arm.pat, sym)
                        && lambda_body_refs_sym(sym, &arm.body)
                })
        }
        Expr::TailLoop { params, body } => {
            if params.iter().any(|(s, _)| *s == sym) {
                false
            } else {
                lambda_body_refs_sym(sym, body)
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            lambda_body_refs_sym(sym, lhs) || lambda_body_refs_sym(sym, rhs)
        }
        Expr::If { cond, then_, else_ } => {
            lambda_body_refs_sym(sym, cond)
                || lambda_body_refs_sym(sym, then_)
                || lambda_body_refs_sym(sym, else_)
        }
        Expr::Call { args, .. } => args.iter().any(|a| lambda_body_refs_sym(sym, a)),
        Expr::Apply { func, args } => {
            lambda_body_refs_sym(sym, func)
                || args.iter().any(|a| lambda_body_refs_sym(sym, a))
        }
        Expr::Tuple(items) => items.iter().any(|e| lambda_body_refs_sym(sym, e)),
        Expr::List { items, .. } => items.iter().any(|e| lambda_body_refs_sym(sym, e)),
        Expr::Cons { head, tail } => {
            lambda_body_refs_sym(sym, head) || lambda_body_refs_sym(sym, tail)
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            lambda_body_refs_sym(sym, list)
        }
        Expr::Record(fields) => fields.iter().any(|(_, e)| lambda_body_refs_sym(sym, e)),
        // Update.record is wrapped in `.clone()` by emit_update (borrow, not move).
        // Only the field value expressions are consuming captures.
        Expr::Update { fields, .. } => {
            fields.iter().any(|(_, e)| lambda_body_refs_sym(sym, e))
        }
        Expr::Ctor { args, .. } => args.iter().any(|a| lambda_body_refs_sym(sym, a)),
        Expr::TaskSeq { effect, rest } | Expr::TaskSeqSync { effect, rest } => {
            lambda_body_refs_sym(sym, effect) || lambda_body_refs_sym(sym, rest)
        }
        Expr::TailRecur { args } => args.iter().any(|a| lambda_body_refs_sym(sym, a)),
        // `Access.record` is borrowed by the emitted `(record).field.clone()` —
        // a borrow of `record`, not a move.  The lambda still needs to capture
        // `sym` if `sym` is the record expression, so we recurse.
        Expr::Access { record, .. } => lambda_body_refs_sym(sym, record),
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => false,
    }
}

// ── T5 helpers for case-arm bound variables ──────────────────────────────────

/// Collect every [`Symbol`] bound by a `case` arm's canon [`Pattern_`] at any
/// depth.  These symbols become locally-owned (after the backend's
/// `list_binder_rebinds` prologue or a `PCtor` destructure) and may need `.clone()`
/// if they appear multiple times in the arm body.
fn collect_arm_pat_pvars(pat: &canon::Pattern_) -> Vec<Symbol> {
    let mut out = Vec::new();
    collect_pvars_inner(pat, &mut out);
    out
}

fn collect_pvars_inner(pat: &canon::Pattern_, out: &mut Vec<Symbol>) {
    match pat {
        canon::Pattern_::PVar(s) => out.push(*s),
        canon::Pattern_::PAlias(inner, name) => {
            out.push(name.value);
            collect_pvars_inner(&inner.value, out);
        }
        canon::Pattern_::PCons(head, tail) => {
            collect_pvars_inner(&head.value, out);
            collect_pvars_inner(&tail.value, out);
        }
        canon::Pattern_::PList(pats) | canon::Pattern_::PTuple(pats) => {
            for p in pats {
                collect_pvars_inner(&p.value, out);
            }
        }
        canon::Pattern_::PCtor { args, .. } => {
            for p in args {
                collect_pvars_inner(&p.value, out);
            }
        }
        canon::Pattern_::PRecord(fields) => {
            for f in fields {
                out.push(f.value);
            }
        }
        // Leaf patterns: no bindings.
        canon::Pattern_::PAnything
        | canon::Pattern_::PInt(_)
        | canon::Pattern_::PBool(_)
        | canon::Pattern_::PChar(_)
        | canon::Pattern_::PStr(_) => {}
    }
}

/// Walk a canon expression tree in pre-order and return the span of the FIRST
/// [`canon::Expr_::VarLocal`] occurrence of `sym`.  Returns `None` if `sym` is
/// not referenced.  The returned span can be keyed into [`Lowerer::region_ty`]
/// to recover the HM type that the solver assigned to that use site, which in
/// turn drives the T5 clone-class decision for arm-bound variables.
fn find_first_varlocal_span(sym: Symbol, body: &canon::Expr) -> Option<Span> {
    match &body.value {
        canon::Expr_::VarLocal(s) if *s == sym => Some(body.span),
        // Atomic leaves — no sub-expressions.
        canon::Expr_::VarLocal(_)
        | canon::Expr_::VarTopLevel { .. }
        | canon::Expr_::VarKernel { .. }
        | canon::Expr_::VarCtor { .. }
        | canon::Expr_::Int(_)
        | canon::Expr_::Float(_)
        | canon::Expr_::Str(_)
        | canon::Expr_::Char(_)
        | canon::Expr_::Unit => None,
        // Compound forms — recurse left-to-right.
        canon::Expr_::Call(f, args) => find_first_varlocal_span(sym, f)
            .or_else(|| args.iter().find_map(|a| find_first_varlocal_span(sym, a))),
        canon::Expr_::Binop { lhs, rhs, .. } => find_first_varlocal_span(sym, lhs)
            .or_else(|| find_first_varlocal_span(sym, rhs)),
        canon::Expr_::Let(bindings, body) => {
            for lb in bindings {
                if let Some(s) = find_first_varlocal_span(sym, &lb.body) {
                    return Some(s);
                }
            }
            find_first_varlocal_span(sym, body)
        }
        canon::Expr_::If(branches, else_) => branches
            .iter()
            .find_map(|(c, t)| {
                find_first_varlocal_span(sym, c)
                    .or_else(|| find_first_varlocal_span(sym, t))
            })
            .or_else(|| find_first_varlocal_span(sym, else_)),
        canon::Expr_::Case(scrut, arms) => find_first_varlocal_span(sym, scrut)
            .or_else(|| arms.iter().find_map(|a| find_first_varlocal_span(sym, &a.body))),
        canon::Expr_::Tuple(items) | canon::Expr_::List(items) => {
            items.iter().find_map(|e| find_first_varlocal_span(sym, e))
        }
        canon::Expr_::Cons(h, t) => find_first_varlocal_span(sym, h)
            .or_else(|| find_first_varlocal_span(sym, t)),
        canon::Expr_::Lambda(_, e) | canon::Expr_::Access(e, _) => {
            find_first_varlocal_span(sym, e)
        }
        canon::Expr_::Record(fields) => {
            fields.iter().find_map(|(_, e)| find_first_varlocal_span(sym, e))
        }
        canon::Expr_::Update(base, fields) => find_first_varlocal_span(sym, base)
            .or_else(|| fields.iter().find_map(|(_, e)| find_first_varlocal_span(sym, e))),
    }
}

/// Count the number of times `sym` is consumed (moved in emitted Rust) by
/// `expr`.  A `Lambda` whose body references `sym` (via `lambda_body_refs_sym`)
/// counts as one consuming occurrence — the `move` closure captures `sym` by
/// value at creation time regardless of how many times the body clones it
/// internally.  Direct `Var(sym)` / `CloneVar(sym)` reads outside lambdas each
/// count once.
///
/// Shadow discipline:
/// * `Let { name == sym, value, body }` — recurse `value` (outer scope), skip
///   `body` (shadowed).
/// * `Destructure { binder binds sym, body }` — skip `body`.
/// * `Match` arm whose pattern binds `sym` — skip that arm's body.
/// * `TailLoop` whose params include `sym` — skip `body`.
/// * `Lambda` — counted as 0 or 1 via `lambda_body_refs_sym`; do NOT recurse
///   further (inner uses are the Lambda's own business).
fn count_var_uses(sym: Symbol, expr: &Expr) -> usize {
    match expr {
        // A pre-pass `CloneVar` at the outer scope counts like a bare `Var`.
        Expr::Var(s) | Expr::CloneVar(s) => usize::from(*s == sym),
        Expr::Lambda { body, .. } => usize::from(lambda_body_refs_sym(sym, body)),
        Expr::Let { name, value, body } => {
            let in_value = count_var_uses(sym, value);
            let in_body =
                if *name == sym { 0 } else { count_var_uses(sym, body) };
            in_value + in_body
        }
        Expr::Destructure { binder, value, body } => {
            let in_value = count_var_uses(sym, value);
            let in_body =
                if pat_binds_symbol(binder, sym) { 0 } else { count_var_uses(sym, body) };
            in_value + in_body
        }
        Expr::If { cond, then_, else_ } => {
            count_var_uses(sym, cond)
                + count_var_uses(sym, then_)
                + count_var_uses(sym, else_)
        }
        Expr::Match(m) => {
            let in_scrut = count_var_uses(sym, m.scrutinee());
            let in_arms: usize = m
                .arms()
                .iter()
                .map(|arm| {
                    if pat_binds_symbol(&arm.pat, sym) {
                        0
                    } else {
                        count_var_uses(sym, &arm.body)
                    }
                })
                .sum();
            in_scrut + in_arms
        }
        Expr::BinOp { lhs, rhs, .. } => {
            count_var_uses(sym, lhs) + count_var_uses(sym, rhs)
        }
        Expr::Call { args, .. } => args.iter().map(|a| count_var_uses(sym, a)).sum(),
        Expr::Apply { func, args } => {
            count_var_uses(sym, func)
                + args.iter().map(|a| count_var_uses(sym, a)).sum::<usize>()
        }
        Expr::Tuple(items) => items.iter().map(|e| count_var_uses(sym, e)).sum(),
        Expr::List { items, .. } => items.iter().map(|e| count_var_uses(sym, e)).sum(),
        Expr::Cons { head, tail } => {
            count_var_uses(sym, head) + count_var_uses(sym, tail)
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            count_var_uses(sym, list)
        }
        Expr::Record(fields) => {
            fields.iter().map(|(_, e)| count_var_uses(sym, e)).sum()
        }
        // `Update.record` — `emit_update` wraps it as `(record).clone()`, which
        // BORROWS the record (`.clone()` takes `&self`).  `sym` is NOT moved here.
        // Only the new FIELD VALUES (fields.values) are consuming positions.
        Expr::Update { fields, .. } => {
            fields.iter().map(|(_, e)| count_var_uses(sym, e)).sum::<usize>()
        }
        Expr::Ctor { args, .. } => args.iter().map(|a| count_var_uses(sym, a)).sum(),
        Expr::TaskSeq { effect, rest } | Expr::TaskSeqSync { effect, rest } => {
            count_var_uses(sym, effect) + count_var_uses(sym, rest)
        }
        Expr::TailLoop { params, body } => {
            if params.iter().any(|(s, _)| *s == sym) {
                0
            } else {
                count_var_uses(sym, body)
            }
        }
        Expr::TailRecur { args } => args.iter().map(|a| count_var_uses(sym, a)).sum(),
        // `Access.record` emits as `(record).field.clone()` — a borrow of
        // `record`, not a move.  We still COUNT the use so that T5 knows a
        // consuming sibling (bare Var) is not the final use and must be
        // cloned.  The rewrite pass handles the Access arm separately (the
        // inner VarLocal is left as a borrow-position, not turned into a move).
        Expr::Access { record, .. } => count_var_uses(sym, record),
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => 0,
    }
}

// ── Fn-value reuse gate, T4 (#90) ─────────────────────────────────────────────
//
// A binding whose type embeds a function (`IrType::Fun`, or a `Maybe`/
// `Result`/user-union carrying one) renders as (or contains) `Box<dyn Fn(..)
// -> R + Send + 'static>`, which is NOT `Clone`. Unlike the multi-use-clone
// rewrite above (only applied to `CloneClass::CloneOk` bindings, which get
// `.clone()` inserted), a `CloneClass::NonClone` fn-carrying binding used more
// than once in a CONSUMING position has no sound rewrite available — it is
// rejected with SKY-L0127 ([`Feature::FunctionValueReuse`]) instead.
//
// [`count_fn_value_uses`] mirrors [`count_var_uses`] with exactly one
// difference: an [`Expr::Apply`] whose `func` is DIRECTLY `sym` is a call —
// `Box<dyn Fn>` implements `Fn`, so `Fn::call` borrows (`&self`), never
// moves — so that occurrence is NOT counted. Every other position (an
// argument, a nested capture, a second forwarding) is counted exactly as
// `count_var_uses` would.

/// Count the number of times `sym` is CONSUMED (moved, in emitted Rust) by
/// `expr`, treating a direct-callee `Expr::Apply` position as non-consuming
/// (a `Box<dyn Fn>` call borrows via `Fn::call(&self, ..)`).
///
/// Used only for the fn-value reuse gate (T4, #90) — never for the
/// multi-use-clone rewrite, which has different call-position semantics for
/// `CloneOk` types (those are not directly callable, so the distinction never
/// mattered there).
fn count_fn_value_uses(sym: Symbol, expr: &Expr) -> usize {
    match expr {
        Expr::Var(s) | Expr::CloneVar(s) => usize::from(*s == sym),
        Expr::Lambda { body, .. } => usize::from(lambda_body_refs_sym(sym, body)),
        Expr::Let { name, value, body } => {
            let in_value = count_fn_value_uses(sym, value);
            let in_body = if *name == sym {
                0
            } else {
                count_fn_value_uses(sym, body)
            };
            in_value + in_body
        }
        Expr::Destructure { binder, value, body } => {
            let in_value = count_fn_value_uses(sym, value);
            let in_body = if pat_binds_symbol(binder, sym) {
                0
            } else {
                count_fn_value_uses(sym, body)
            };
            in_value + in_body
        }
        Expr::If { cond, then_, else_ } => {
            count_fn_value_uses(sym, cond)
                + count_fn_value_uses(sym, then_)
                + count_fn_value_uses(sym, else_)
        }
        Expr::Match(m) => {
            let in_scrut = count_fn_value_uses(sym, m.scrutinee());
            let in_arms: usize = m
                .arms()
                .iter()
                .map(|arm| {
                    if pat_binds_symbol(&arm.pat, sym) {
                        0
                    } else {
                        count_fn_value_uses(sym, &arm.body)
                    }
                })
                .sum();
            in_scrut + in_arms
        }
        Expr::BinOp { lhs, rhs, .. } => {
            count_fn_value_uses(sym, lhs) + count_fn_value_uses(sym, rhs)
        }
        Expr::Call { args, .. } => args.iter().map(|a| count_fn_value_uses(sym, a)).sum(),
        // The one arm that differs from `count_var_uses`: a direct-callee
        // `Apply { func: Var(sym) | CloneVar(sym), .. }` borrows, not moves.
        Expr::Apply { func, args } => {
            let func_uses = if matches!(func.as_ref(), Expr::Var(s) | Expr::CloneVar(s) if *s == sym)
            {
                0
            } else {
                count_fn_value_uses(sym, func)
            };
            func_uses
                + args
                    .iter()
                    .map(|a| count_fn_value_uses(sym, a))
                    .sum::<usize>()
        }
        Expr::Tuple(items) => items.iter().map(|e| count_fn_value_uses(sym, e)).sum(),
        Expr::List { items, .. } => items.iter().map(|e| count_fn_value_uses(sym, e)).sum(),
        Expr::Cons { head, tail } => {
            count_fn_value_uses(sym, head) + count_fn_value_uses(sym, tail)
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            count_fn_value_uses(sym, list)
        }
        Expr::Record(fields) => fields.iter().map(|(_, e)| count_fn_value_uses(sym, e)).sum(),
        Expr::Update { fields, .. } => {
            fields.iter().map(|(_, e)| count_fn_value_uses(sym, e)).sum::<usize>()
        }
        Expr::Ctor { args, .. } => args.iter().map(|a| count_fn_value_uses(sym, a)).sum(),
        Expr::TaskSeq { effect, rest } | Expr::TaskSeqSync { effect, rest } => {
            count_fn_value_uses(sym, effect) + count_fn_value_uses(sym, rest)
        }
        Expr::TailLoop { params, body } => {
            if params.iter().any(|(s, _)| *s == sym) {
                0
            } else {
                count_fn_value_uses(sym, body)
            }
        }
        Expr::TailRecur { args } => args.iter().map(|a| count_fn_value_uses(sym, a)).sum(),
        Expr::Access { record, .. } => count_fn_value_uses(sym, record),
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => 0,
    }
}

/// T4 (#90): fail closed with [`Feature::FunctionValueReuse`] (SKY-L0127) if
/// `sym` — a binding whose IR type embeds a function ([`ir_contains_fun`])
/// and does not derive `Clone` ([`CloneClass::NonClone`]) — is CONSUMED more
/// than once in `body`.
///
/// Self-guarding: a no-op `Ok(())` for any OTHER type (a `CopyLeaf` like
/// `Int`, or a `CloneOk`/opaque `NonClone` carrier with no embedded
/// function — e.g. a bare `Task`/`Decoder`, which #90 does not touch and
/// which the multi-use-clone rewrite already handles for `CloneOk`), so
/// callers may invoke it unconditionally wherever that rewrite does not
/// apply. See the "Fn-value reuse gate, T4 (#90)" module doc block above
/// [`count_fn_value_uses`] for why a direct-callee use is exempt.
fn reject_fn_value_reuse(sym: Symbol, ir_ty: &IrType, body: &Expr, span: Span) -> DResult<()> {
    if !ir_contains_fun(ir_ty) || !matches!(clone_class(ir_ty), CloneClass::NonClone) {
        return Ok(());
    }
    if count_fn_value_uses(sym, body) > 1 {
        return Err(unsupported(span, Feature::FunctionValueReuse));
    }
    Ok(())
}

/// Rewrite `Var(sym)` / `Lambda`-captures of `sym` in DFS left-to-right order
/// so that all but the syntactically last occurrence are `.clone()`d.
///
/// `remaining` starts at `count_var_uses(sym, expr)`.  Each consuming
/// occurrence decrements it; when `remaining > 1` the occurrence is non-last
/// and is rewritten:
/// * bare `Var(sym)` → `CloneVar(sym)`;
/// * Lambda whose body refs sym → `Let { name: sym, value: CloneVar(sym), body: Lambda }`.
///   The pre-clone rebinding captures the CLONE into the closure while the
///   OUTER `sym` remains alive for subsequent uses.
///
/// When `remaining == 1` (the last occurrence), the node is kept bare.
///
/// Shadow discipline and Lambda-body descent mirror `count_var_uses`.
#[allow(clippy::too_many_lines)]
fn rewrite_multiuse_clones(sym: Symbol, remaining: &mut usize, expr: Expr) -> Expr {
    if *remaining == 0 {
        return expr;
    }
    match expr {
        Expr::Var(s) if s == sym => {
            if *remaining > 1 {
                *remaining -= 1;
                Expr::CloneVar(s)
            } else {
                *remaining -= 1;
                Expr::Var(s)
            }
        }
        // Pre-existing CloneVar at the outer scope: treat like Var — always
        // leave as CloneVar (it already borrows; no further action needed).
        Expr::CloneVar(s) if s == sym => {
            *remaining -= 1;
            Expr::CloneVar(s)
        }
        // Lambda: if it move-captures `sym`, consume one `remaining` slot.
        // When NOT the last use, wrap in a pre-clone Let so the closure
        // captures the clone and the outer `sym` stays alive.
        Expr::Lambda { params, ret, body } => {
            if lambda_body_refs_sym(sym, &body) {
                if *remaining > 1 {
                    *remaining -= 1;
                    // Pre-clone: `let sym = sym.clone() in Lambda { … }`.
                    // The inner rebinding `sym` shadows the outer; the `move`
                    // closure inside captures the inner sym (the clone).
                    Expr::Let {
                        name: sym,
                        value: Box::new(Expr::CloneVar(sym)),
                        body: Box::new(Expr::Lambda { params, ret, body }),
                    }
                } else {
                    *remaining -= 1;
                    Expr::Lambda { params, ret, body }
                }
            } else {
                Expr::Lambda { params, ret, body }
            }
        }
        // Non-`sym` Var / CloneVar and all atomic leaves — pass through.
        Expr::Var(_)
        | Expr::CloneVar(_)
        | Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => expr,
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(rewrite_multiuse_clones(sym, remaining, *lhs)),
            rhs: Box::new(rewrite_multiuse_clones(sym, remaining, *rhs)),
        },
        Expr::If { cond, then_, else_ } => Expr::If {
            cond: Box::new(rewrite_multiuse_clones(sym, remaining, *cond)),
            then_: Box::new(rewrite_multiuse_clones(sym, remaining, *then_)),
            else_: Box::new(rewrite_multiuse_clones(sym, remaining, *else_)),
        },
        // `value` is in the outer scope; `body` is shadowed if `name == sym`.
        Expr::Let { name, value, body } => {
            let new_value = Box::new(rewrite_multiuse_clones(sym, remaining, *value));
            let new_body = if name == sym {
                body
            } else {
                Box::new(rewrite_multiuse_clones(sym, remaining, *body))
            };
            Expr::Let { name, value: new_value, body: new_body }
        }
        Expr::Destructure { binder, value, body } => {
            let new_value = Box::new(rewrite_multiuse_clones(sym, remaining, *value));
            let new_body = if pat_binds_symbol(&binder, sym) {
                body
            } else {
                Box::new(rewrite_multiuse_clones(sym, remaining, *body))
            };
            Expr::Destructure { binder, value: new_value, body: new_body }
        }
        Expr::Match(m) => {
            let (scrutinee, arms) = m.into_parts();
            let new_scrutinee =
                Box::new(rewrite_multiuse_clones(sym, remaining, *scrutinee));
            let new_arms = arms
                .into_iter()
                .map(|arm| {
                    let new_body = if pat_binds_symbol(&arm.pat, sym) {
                        arm.body
                    } else {
                        rewrite_multiuse_clones(sym, remaining, arm.body)
                    };
                    Arm { pat: arm.pat, body: new_body, guard: arm.guard }
                })
                .collect();
            Expr::Match(Match::from_parts_unchecked(new_scrutinee, new_arms))
        }
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args
                .into_iter()
                .map(|a| rewrite_multiuse_clones(sym, remaining, a))
                .collect(),
        },
        Expr::Apply { func, args } => {
            let new_func = Box::new(rewrite_multiuse_clones(sym, remaining, *func));
            let new_args = args
                .into_iter()
                .map(|a| rewrite_multiuse_clones(sym, remaining, a))
                .collect();
            Expr::Apply { func: new_func, args: new_args }
        }
        Expr::Tuple(items) => Expr::Tuple(
            items
                .into_iter()
                .map(|e| rewrite_multiuse_clones(sym, remaining, e))
                .collect(),
        ),
        Expr::List { elem, items } => Expr::List {
            elem,
            items: items
                .into_iter()
                .map(|e| rewrite_multiuse_clones(sym, remaining, e))
                .collect(),
        },
        Expr::Cons { head, tail } => Expr::Cons {
            head: Box::new(rewrite_multiuse_clones(sym, remaining, *head)),
            tail: Box::new(rewrite_multiuse_clones(sym, remaining, *tail)),
        },
        Expr::ListIndexClone { list, index } => Expr::ListIndexClone {
            list: Box::new(rewrite_multiuse_clones(sym, remaining, *list)),
            index,
        },
        Expr::ListLenCheck { list, len, exact } => Expr::ListLenCheck {
            list: Box::new(rewrite_multiuse_clones(sym, remaining, *list)),
            len,
            exact,
        },
        Expr::Record(fields) => Expr::Record(
            fields
                .into_iter()
                .map(|(k, v)| (k, rewrite_multiuse_clones(sym, remaining, v)))
                .collect(),
        ),
        // `Access.record` emits as `(record).field.clone()` — the record is
        // BORROWED by the method call, not moved.  We still recurse so that
        // the `remaining` counter advances correctly (count_var_uses counts
        // Access-under-record uses).  A VarLocal found here becomes CloneVar
        // unless it is the last overall use, in which case it stays bare (the
        // borrow keeps the original value alive for subsequent uses).
        Expr::Access { record, field } => Expr::Access {
            record: Box::new(rewrite_multiuse_clones(sym, remaining, *record)),
            field,
        },
        // `Update.record` is always wrapped in `(record).clone()` by `emit_update`
        // (a borrow via `Clone::clone(&self)`).  Only the field VALUES are consuming
        // positions; leave `record` bare.
        Expr::Update { record, fields } => Expr::Update {
            record,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, rewrite_multiuse_clones(sym, remaining, v)))
                .collect(),
        },
        Expr::Ctor { home, ty, variant, args } => Expr::Ctor {
            home,
            ty,
            variant,
            args: args
                .into_iter()
                .map(|a| rewrite_multiuse_clones(sym, remaining, a))
                .collect(),
        },
        Expr::TaskSeq { effect, rest } => Expr::TaskSeq {
            effect: Box::new(rewrite_multiuse_clones(sym, remaining, *effect)),
            rest: Box::new(rewrite_multiuse_clones(sym, remaining, *rest)),
        },
        Expr::TaskSeqSync { effect, rest } => Expr::TaskSeqSync {
            effect: Box::new(rewrite_multiuse_clones(sym, remaining, *effect)),
            rest: Box::new(rewrite_multiuse_clones(sym, remaining, *rest)),
        },
        Expr::TailLoop { params, body } => {
            if params.iter().any(|(s, _)| *s == sym) {
                Expr::TailLoop { params, body } // sym shadowed by loop var
            } else {
                Expr::TailLoop {
                    params,
                    body: Box::new(rewrite_multiuse_clones(sym, remaining, *body)),
                }
            }
        }
        Expr::TailRecur { args } => Expr::TailRecur {
            args: args
                .into_iter()
                .map(|a| rewrite_multiuse_clones(sym, remaining, a))
                .collect(),
        },
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
/// One binder slot in a nested list pattern — a named variable or a wildcard.
#[derive(Clone, Copy)]
enum NestedBinder {
    Named(Symbol),
    Wildcard,
}

/// The open-tail shape of a nested list pattern: a `[a, b]` literal (or a cons
/// chain ending in `[]`) is `Closed` (exact-length match); an open cons chain
/// carries a `Named` / `Wildcard` rest binder.
#[derive(Clone, Copy)]
enum NestedTail {
    Closed,
    Rest(NestedBinder),
}

/// The flattened shape of a SUPPORTABLE list / cons sub-pattern nested inside a
/// constructor payload, for the Class 4 item C2 (#158) desugaring. `prefix`
/// holds one binder per leading element; `tail` records whether the pattern is
/// closed (exact length) or open (a rest binder).
struct FlatNestedList {
    prefix: Vec<NestedBinder>,
    tail: NestedTail,
}

impl FlatNestedList {
    /// Is this pattern CLOSED (a `[a, b]` literal / a cons chain ending in
    /// `[]`)? A closed pattern lowers to an exact-length `.len() == N` guard; an
    /// open one to `.len() >= N`.
    const fn closed(&self) -> bool {
        matches!(self.tail, NestedTail::Closed)
    }

    /// Does this nested list BIND at least one value (a named head element or a
    /// named open-tail binder)? A fully-wildcard shape (`Just [_, _]`) binds
    /// nothing, so it needs no element-`Clone` bound and skips the polymorphism
    /// gate.
    fn binds_a_value(&self) -> bool {
        self.prefix
            .iter()
            .any(|b| matches!(b, NestedBinder::Named(_)))
            || matches!(self.tail, NestedTail::Rest(NestedBinder::Named(_)))
    }
}

/// Classify a single list-element / tail sub-pattern as a plain binder for the
/// #158 C2 desugaring: a `PVar` / `PAnything`. Any refutable sub-pattern is
/// `None` (the whole nested list is then not desugarable).
const fn nested_simple_binder(p: &canon::Pattern) -> Option<NestedBinder> {
    match &p.value {
        canon::Pattern_::PVar(s) => Some(NestedBinder::Named(*s)),
        canon::Pattern_::PAnything => Some(NestedBinder::Wildcard),
        _ => None,
    }
}

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
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            count_self_calls(self_id, arity, list, false, tail, non_tail);
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
        Expr::TaskSeq { effect, rest } | Expr::TaskSeqSync { effect, rest } => {
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
        | Expr::Var(_)
        | Expr::CloneVar(_) => {}
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
                    guard: arm.guard.clone(),
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

// ── Kernel-family presence detection (one traversal) ────────────────────────

/// Outcome of [`Lowerer::intercept_live_kernel_call`].
enum Intercepted {
    /// The call was intercepted and fully lowered.
    Done(Expr),
    /// Not intercepted — continue on [`Lowerer::lower_call_uniform`]. When the
    /// callee was a `VarKernel`/`VarTopLevel`, the already-resolved [`Callee`]
    /// is carried so the uniform path doesn't re-run the `lower_callee`
    /// dispatch (efficiency-audit §3 medium).
    Fallthrough(Option<Callee>),
}

/// Per-family kernel-usage flags, collected in ONE traversal over every
/// function body (efficiency-audit §3 medium: [`Lowerer::run`] previously
/// walked every body once per family — nine independent full-AST
/// `expr_uses_<family>_kernel` passes).
///
/// Each flag is the OR of the same [`sky_ir::KernelFn`] family predicate the
/// former per-family walkers applied to `Call` / `FuncValue` callees, over the
/// same traversal shape — so the nine booleans are identical to the
/// nine-pass form.
// Nine genuinely independent presence flags (same shape as `ir::Module`'s
// `uses_*` fields, which carry the same allow) — not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
struct KernelUsage {
    /// Any `Db*` (including `DbDec*`) kernel — gates the synthetic
    /// `SqlValue`/`SqlField` enum injection and the db-enabled backend files.
    db: bool,
    /// Any TEA (`Cmd*` / `Sub*` / `TimeEvery`) kernel (M5c).
    tea: bool,
    /// Any Sky.Http.Server kernel (M6).
    server: bool,
    /// Any Std.Ui render kernel (M7).
    ui: bool,
    /// Any Std.Css (Sky.Core.CssSafety) leaf kernel (#47) — independent of
    /// `ui`: a pure-Std.Css program uses no render kernel.
    css: bool,
    /// Any Std.Auth kernel (#111).
    auth: bool,
    /// Any Std.Live kernel (M7).
    live: bool,
    /// Any Std.Tui kernel (M7).
    tui: bool,
    /// Any Std.Webview kernel (M7).
    webview: bool,
}

impl KernelUsage {
    /// Every flag already set — nothing left to learn, traversal can stop.
    const fn all_set(&self) -> bool {
        self.db
            && self.tea
            && self.server
            && self.ui
            && self.css
            && self.auth
            && self.live
            && self.tui
            && self.webview
    }

    /// OR in the family flags for one kernel callee.
    const fn record(&mut self, k: KernelFn) {
        self.db |= k.is_db();
        self.tea |= k.is_tea();
        self.server |= k.is_server();
        self.ui |= k.is_ui();
        self.css |= k.is_css();
        self.auth |= k.is_auth();
        self.live |= k.is_live();
        self.tui |= k.is_tui();
        self.webview |= k.is_webview();
    }
}

/// Record every kernel callee reachable from `expr` into `usage`.
///
/// Traversal shape mirrors the former per-family walkers exactly: `Call` /
/// `FuncValue` callees are inspected (a `FuncValue` reifies a callee as a
/// first-class value — not a direct call — but a kernel callee still implies
/// usage); every sub-expression is visited; leaves cannot contain a kernel
/// call. A `TailLoop` (a TCO'd body) recurses into its tail body exactly as
/// the pre-TCO body would; a `TailRecur` (a TCO jump) carries its
/// next-iteration args like a `Ctor`. Early-exits once every flag is set.
fn scan_kernel_usage(expr: &Expr, usage: &mut KernelUsage) {
    if usage.all_set() {
        return;
    }
    match expr {
        Expr::Call { callee, args } => {
            if let Callee::Kernel(k) = callee {
                usage.record(*k);
            }
            for a in args {
                scan_kernel_usage(a, usage);
            }
        }
        Expr::FuncValue { callee, .. } => {
            if let Callee::Kernel(k) = callee {
                usage.record(*k);
            }
        }
        Expr::Apply { func, args } => {
            scan_kernel_usage(func, usage);
            for a in args {
                scan_kernel_usage(a, usage);
            }
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            scan_kernel_usage(value, usage);
            scan_kernel_usage(body, usage);
        }
        Expr::If { cond, then_, else_ } => {
            scan_kernel_usage(cond, usage);
            scan_kernel_usage(then_, usage);
            scan_kernel_usage(else_, usage);
        }
        Expr::Match(m) => {
            scan_kernel_usage(m.scrutinee(), usage);
            for arm in m.arms() {
                scan_kernel_usage(&arm.body, usage);
            }
        }
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => {
            scan_kernel_usage(body, usage);
        }
        Expr::Cons { head, tail } => {
            scan_kernel_usage(head, usage);
            scan_kernel_usage(tail, usage);
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            scan_kernel_usage(list, usage);
        }
        Expr::Tuple(elems) => {
            for e in elems {
                scan_kernel_usage(e, usage);
            }
        }
        Expr::List { items, .. } => {
            for e in items {
                scan_kernel_usage(e, usage);
            }
        }
        Expr::Record(fields) => {
            for (_, v) in fields {
                scan_kernel_usage(v, usage);
            }
        }
        Expr::Access { record, .. } => scan_kernel_usage(record, usage),
        Expr::Update { record, fields } => {
            scan_kernel_usage(record, usage);
            for (_, v) in fields {
                scan_kernel_usage(v, usage);
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            scan_kernel_usage(lhs, usage);
            scan_kernel_usage(rhs, usage);
        }
        Expr::TaskSeq { effect, rest } | Expr::TaskSeqSync { effect, rest } => {
            scan_kernel_usage(effect, usage);
            scan_kernel_usage(rest, usage);
        }
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => {
            for a in args {
                scan_kernel_usage(a, usage);
            }
        }
        // Leaf expressions that cannot contain a kernel call.
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::CloneVar(_)
        | Expr::Var(_) => {}
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

/// Does this pattern bind the symbol `target`?
///
/// Used by [`rewrite_var_to_apply`] to detect shadow bindings — when a
/// pattern in a `let`-destructure / `case` arm / lambda rebinds `target`,
/// `Var(target)` reads inside that scope are NOT references to the outer
/// thunk binding and must NOT be rewritten.
fn pat_binds_symbol(pat: &Pat, target: Symbol) -> bool {
    match pat {
        Pat::Var(s) => *s == target,
        Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_) => false,
        Pat::Alias(inner, s) => *s == target || pat_binds_symbol(inner, target),
        Pat::Ctor { args, .. } => args.iter().any(|p| pat_binds_symbol(p, target)),
        Pat::Tuple(elems) => elems.iter().any(|p| pat_binds_symbol(p, target)),
        Pat::Record(fields) => fields.iter().any(|(_, p)| pat_binds_symbol(p, target)),
        Pat::Slice { prefix, rest } => {
            prefix.iter().any(|p| pat_binds_symbol(p, target))
                || rest.as_deref().is_some_and(|p| pat_binds_symbol(p, target))
        }
    }
}

/// Rewrite every FREE `Expr::Var(target)` in `expr` to `on_hit(target)` —
/// the shared shadow-aware tree walk behind [`rewrite_var_to_apply`] (#89 F2)
/// and [`rewrite_destructure_read`] (#125). Factoring the walk out keeps the
/// two rewrites' shadow handling provably identical instead of two chances
/// to drift (spec §2.5,
/// `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md`).
///
/// Shadow-safe: stops rewriting into any scope where `target` is rebound by:
/// * `Expr::Let { name, … }` — `name == target` shadows in `body`
/// * `Expr::Destructure { binder, … }` — `binder` binds `target`
/// * `Expr::Lambda { params, … }` — a param named `target`
/// * `Expr::Match` arms — an arm's `pat` binds `target`
///
/// `value` is never rewritten inside a `Let` (it is evaluated in the outer
/// scope, matching the `let` scoping rule), and a thunk's own body (the
/// decoder expression) is also in the outer scope and is already correct.
#[allow(clippy::too_many_lines)] // A recursive tree-walk over a large enum — necessarily long.
fn rewrite_var_free_occurrences(
    target: Symbol,
    expr: Expr,
    on_hit: &impl Fn(Symbol) -> Expr,
) -> Expr {
    match expr {
        Expr::Var(s) if s == target => on_hit(s),
        Expr::Var(_)
        | Expr::CloneVar(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => expr,
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(rewrite_var_free_occurrences(target, *lhs, on_hit)),
            rhs: Box::new(rewrite_var_free_occurrences(target, *rhs, on_hit)),
        },
        // `let name = value in body` — `value` is outer-scope; `body` is
        // inner-scope but the shadow check applies.
        Expr::Let { name, value, body } => {
            let new_value = Box::new(rewrite_var_free_occurrences(target, *value, on_hit));
            let new_body = if name == target {
                // `target` is shadowed; reads in body are the new binding.
                body
            } else {
                Box::new(rewrite_var_free_occurrences(target, *body, on_hit))
            };
            Expr::Let {
                name,
                value: new_value,
                body: new_body,
            }
        }
        Expr::Destructure { binder, value, body } => {
            let new_value = Box::new(rewrite_var_free_occurrences(target, *value, on_hit));
            let new_body = if pat_binds_symbol(&binder, target) {
                body
            } else {
                Box::new(rewrite_var_free_occurrences(target, *body, on_hit))
            };
            Expr::Destructure {
                binder,
                value: new_value,
                body: new_body,
            }
        }
        Expr::If { cond, then_, else_ } => Expr::If {
            cond: Box::new(rewrite_var_free_occurrences(target, *cond, on_hit)),
            then_: Box::new(rewrite_var_free_occurrences(target, *then_, on_hit)),
            else_: Box::new(rewrite_var_free_occurrences(target, *else_, on_hit)),
        },
        Expr::Match(m) => {
            let (scrutinee, arms) = m.into_parts();
            let new_scrutinee = Box::new(rewrite_var_free_occurrences(target, *scrutinee, on_hit));
            let new_arms = arms
                .into_iter()
                .map(|arm| {
                    let binds = pat_binds_symbol(&arm.pat, target);
                    let new_body = if binds {
                        arm.body
                    } else {
                        rewrite_var_free_occurrences(target, arm.body, on_hit)
                    };
                    // A C2 arm guard is evaluated in the same scope as its body;
                    // rewrite free occurrences of `target` there too when the arm
                    // pattern does not shadow it (guards over a fresh length-check
                    // var normally reference no outer capture, but staying uniform
                    // keeps this rewrite total for any future guard shape).
                    let new_guard = arm.guard.map(|g| {
                        if binds {
                            g
                        } else {
                            rewrite_var_free_occurrences(target, g, on_hit)
                        }
                    });
                    Arm {
                        pat: arm.pat,
                        body: new_body,
                        guard: new_guard,
                    }
                })
                .collect();
            Expr::Match(Match::from_parts_unchecked(new_scrutinee, new_arms))
        }
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args
                .into_iter()
                .map(|a| rewrite_var_free_occurrences(target, a, on_hit))
                .collect(),
        },
        Expr::Tuple(items) => {
            Expr::Tuple(items.into_iter().map(|e| rewrite_var_free_occurrences(target, e, on_hit)).collect())
        }
        Expr::List { elem, items } => Expr::List {
            elem,
            items: items.into_iter().map(|e| rewrite_var_free_occurrences(target, e, on_hit)).collect(),
        },
        Expr::Cons { head, tail } => Expr::Cons {
            head: Box::new(rewrite_var_free_occurrences(target, *head, on_hit)),
            tail: Box::new(rewrite_var_free_occurrences(target, *tail, on_hit)),
        },
        Expr::ListIndexClone { list, index } => Expr::ListIndexClone {
            list: Box::new(rewrite_var_free_occurrences(target, *list, on_hit)),
            index,
        },
        Expr::ListLenCheck { list, len, exact } => Expr::ListLenCheck {
            list: Box::new(rewrite_var_free_occurrences(target, *list, on_hit)),
            len,
            exact,
        },
        Expr::Record(fields) => Expr::Record(
            fields
                .into_iter()
                .map(|(sym, e)| (sym, rewrite_var_free_occurrences(target, e, on_hit)))
                .collect(),
        ),
        Expr::Access { record, field } => Expr::Access {
            record: Box::new(rewrite_var_free_occurrences(target, *record, on_hit)),
            field,
        },
        Expr::Update { record, fields } => Expr::Update {
            record: Box::new(rewrite_var_free_occurrences(target, *record, on_hit)),
            fields: fields
                .into_iter()
                .map(|(sym, e)| (sym, rewrite_var_free_occurrences(target, e, on_hit)))
                .collect(),
        },
        Expr::Lambda { params, ret, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(rewrite_var_free_occurrences(target, *body, on_hit))
            };
            Expr::Lambda { params, ret, body: new_body }
        }
        Expr::Apply { func, args } => Expr::Apply {
            func: Box::new(rewrite_var_free_occurrences(target, *func, on_hit)),
            args: args.into_iter().map(|a| rewrite_var_free_occurrences(target, a, on_hit)).collect(),
        },
        Expr::TaskSeq { effect, rest } => Expr::TaskSeq {
            effect: Box::new(rewrite_var_free_occurrences(target, *effect, on_hit)),
            rest: Box::new(rewrite_var_free_occurrences(target, *rest, on_hit)),
        },
        Expr::TaskSeqSync { effect, rest } => Expr::TaskSeqSync {
            effect: Box::new(rewrite_var_free_occurrences(target, *effect, on_hit)),
            rest: Box::new(rewrite_var_free_occurrences(target, *rest, on_hit)),
        },
        Expr::Ctor { home, ty, variant, args } => Expr::Ctor {
            home,
            ty,
            variant,
            args: args.into_iter().map(|a| rewrite_var_free_occurrences(target, a, on_hit)).collect(),
        },
        // TailLoop/TailRecur are produced by a separate TCO pass that runs
        // AFTER lower_let, so they never appear in the IR at the point this
        // rewrite runs. The `_ => expr` below keeps the match exhaustive-
        // compatible with future IR additions (they're also leaf-like).
        Expr::TailLoop { params, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(rewrite_var_free_occurrences(target, *body, on_hit))
            };
            Expr::TailLoop { params, body: new_body }
        }
        Expr::TailRecur { args } => Expr::TailRecur {
            args: args.into_iter().map(|a| rewrite_var_free_occurrences(target, a, on_hit)).collect(),
        },
    }
}

/// Rewrite every free `Expr::Var(target)` in `expr` to
/// `Expr::Apply { func: Var(target), args: [] }` (emitted as `(target)()`).
///
/// This is the read-site half of the Decoder thunk rewrite (#89 F2, design
/// preserved in git history as `seal-jsondecp-design.md` §5.C): after
/// [`Lowerer::lower_let`] wraps a Decoder-typed binding value in a zero-arg
/// lambda, every read of that binding must call the thunk to obtain a fresh
/// `Decoder` value. Thin wrapper over [`rewrite_var_free_occurrences`].
fn rewrite_var_to_apply(target: Symbol, expr: Expr) -> Expr {
    rewrite_var_free_occurrences(target, expr, &|s| Expr::Apply {
        func: Box::new(Expr::Var(s)),
        args: vec![],
    })
}

/// Does `ty` structurally contain [`IrType::Decoder`] anywhere (itself, or
/// nested inside a `Tuple`/`Record`/`Maybe`/`Result`/`List`)? Gates #125's
/// destructure-thunk rewrite: a `Tuple`/`Record` binder whose aggregate
/// type contains a Decoder anywhere needs the WHOLE destructure thunked
/// (spec §2.2) — a Decoder nested inside e.g. `Maybe (Decoder a)` is out of
/// today's realistic reach (Decoders aren't optional in practice) but the
/// predicate stays structurally total rather than special-cased to Tuple/
/// Record only, matching `ir_type_contains_task`'s existing shape in the
/// Rust backend (AUD-04).
fn ir_type_contains_decoder(ty: &IrType) -> bool {
    match ty {
        IrType::Decoder(_) => true,
        IrType::Tuple(elems) => elems.iter().any(ir_type_contains_decoder),
        IrType::Record(fields) => fields.values().any(ir_type_contains_decoder),
        IrType::Maybe(inner) | IrType::List(inner) => ir_type_contains_decoder(inner),
        IrType::Result(e, a) => ir_type_contains_decoder(e) || ir_type_contains_decoder(a),
        _ => false,
    }
}

/// Collect every symbol `pat` binds (recursively) into `out`. Local twin
/// of `sky_backend_rust::pat_bound_symbols` (same shape, same crate-
/// boundary rationale — `sky_lower` and `sky_backend_rust` each keep their
/// own copy rather than share one, since IR flows one-way lower → backend).
fn pat_bound_symbols(pat: &Pat, out: &mut BTreeSet<Symbol>) {
    match pat {
        Pat::Var(s) => {
            out.insert(*s);
        }
        Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => {}
        Pat::Alias(inner, s) => {
            out.insert(*s);
            pat_bound_symbols(inner, out);
        }
        Pat::Ctor { args, .. } | Pat::Tuple(args) => {
            for p in args {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Record(fields) => {
            for (_, p) in fields {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Slice { prefix, rest } => {
            for p in prefix {
                pat_bound_symbols(p, out);
            }
            if let Some(p) = rest {
                pat_bound_symbols(p, out);
            }
        }
    }
}

/// Rebuild `pat` with every bound name EXCEPT `keep` erased to
/// [`Pat::Wildcard`] — an `Alias`'s own name collapses to a bare
/// `Pat::Var(keep)` at that position when `keep` is the alias name itself
/// (dropping the alias wrapper entirely; a single flat name needs no `as`),
/// otherwise the alias erases and recurses into `inner` (its own name is
/// irrelevant to this masked, single-name extraction). Used to build the
/// per-read-site re-destructure pattern for #125 (spec §2.2) — reusing the
/// ORIGINAL pattern's shape (masked) sidesteps needing any tuple-index /
/// record-field EXPRESSION accessor in the IR, since [`Expr::Destructure`]
/// already exists to bind a pattern from a value.
fn mask_pattern_except(pat: &Pat, keep: Symbol) -> Pat {
    match pat {
        Pat::Var(s) => {
            if *s == keep {
                Pat::Var(*s)
            } else {
                Pat::Wildcard
            }
        }
        Pat::Alias(inner, s) => {
            if *s == keep {
                Pat::Var(*s)
            } else {
                mask_pattern_except(inner, keep)
            }
        }
        Pat::Tuple(elems) => {
            Pat::Tuple(elems.iter().map(|p| mask_pattern_except(p, keep)).collect())
        }
        Pat::Record(fields) => Pat::Record(
            fields
                .iter()
                .map(|(n, p)| (*n, mask_pattern_except(p, keep)))
                .collect(),
        ),
        // Binding-free leaves keep their shape. `Ctor` / `Slice` never
        // appear in a #125-eligible binder (the irrefutable destructure
        // grammar forbids them — `lower_destructure_pat`'s own fail-closed
        // arms); kept total via an unreachable-in-practice clone rather
        // than a partial match.
        Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Slice { .. } => pat.clone(),
    }
}

/// Shadow-aware rewrite: replace every FREE `Expr::Var(target)` in `expr`
/// with a fresh, masked re-destructure of `thunk_name`'s call result — the
/// #125 generalization of [`rewrite_var_to_apply`] (which only ever needs a
/// bare `Apply`, since a `PVar` binder has exactly one name that directly
/// names the re-buildable value). A `Tuple`/`Record` binder introduces
/// MULTIPLE names from ONE value, so each read must also RE-PROJECT the
/// right component out of a fresh thunk call:
///
/// ```text
/// -- every free read of `d1` becomes:
/// { let (d1, _) = (__thunk)(); d1 }
/// ```
///
/// Shares [`rewrite_var_free_occurrences`]'s walk, so the shadow rules are
/// provably identical to [`rewrite_var_to_apply`]'s.
fn rewrite_destructure_read(
    target: Symbol,
    root_pat: &Pat,
    thunk_name: Symbol,
    expr: Expr,
) -> Expr {
    rewrite_var_free_occurrences(target, expr, &|s| Expr::Destructure {
        binder: mask_pattern_except(root_pat, s),
        value: Box::new(Expr::Apply {
            func: Box::new(Expr::Var(thunk_name)),
            args: vec![],
        }),
        body: Box::new(Expr::Var(s)),
    })
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
    /// Pre-minted, collision-free names for capturing a supplied argument
    /// expression in an eta-expand-partial hoist (T4/#121). When a supplied arg
    /// is not a literal or bare `Var`, it is hoisted to
    /// `let __sky_cap_i = <arg> in <lambda>` so it evaluates once even though
    /// the lambda is called multiple times. Sized identically to `eta_params`
    /// (widest arity); position `i` names the i-th hoisted capture.
    cap_params: Vec<Symbol>,
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
    /// Fresh symbols for the per-occurrence `any`-in-param-position seal fix
    /// (AUD-01). Sized by [`count_any_param_sites`], pre-interned through the
    /// owned `&mut Interner` before this immutably-borrowed `Lowerer` is
    /// constructed (the interner is frozen by lowering time — a symbol cannot
    /// be minted from inside `&self`). Each bare param-position `any`
    /// occurrence in `split_typed_sig` gets ONE of these instead of sharing the
    /// single interned `"any"` Symbol, so two `any` params pinned by the body
    /// to two DIFFERENT concrete types emit as two DISTINCT Rust generics
    /// (`fn f<T1, T2>(a:T1, b:T2)`) rather than colliding onto one shared `T1`.
    any_param_binders: Vec<Symbol>,
    /// Monotonic cursor into [`Self::any_param_binders`], mirroring
    /// [`Self::param_cursor`]'s shape exactly.
    any_param_cursor: Cell<usize>,
    /// Pre-minted, collision-free names for the #125 destructure-thunk
    /// binding (`let destr_thunk_N = move || <value>; …`) that
    /// [`Self::build_destructure_or_decoder_thunk`] introduces when a
    /// tuple / record / alias destructure binds a value whose type
    /// contains `IrType::Decoder`. Sized by
    /// [`count_destructure_thunk_sites`] (every syntactic destructure
    /// site, regardless of type — the type gate runs post-solve) and
    /// handed out through [`Self::destructure_thunk_cursor`] as a
    /// GLOBALLY-unique supply, mirroring [`Self::param_binders`].
    destructure_thunk_binders: Vec<Symbol>,
    /// Monotonic cursor into [`Self::destructure_thunk_binders`],
    /// mirroring [`Self::param_cursor`]'s shape exactly.
    destructure_thunk_cursor: Cell<usize>,
    /// Pre-minted, collision-free `Vec` binder names for the Class 4 item C2
    /// (#158) nested-cons desugaring — a `PList` / `PCons` sub-pattern nested in
    /// a `PCtor` arm payload (`Just (h :: t)`) lowers to a fresh `Vec` binder in
    /// that ctor-arg slot plus an arm guard, with the named head / tail bindings
    /// recovered in the body prelude. Sized by [`count_nested_cons_payload_sites`]
    /// (an upper bound over every `case`-arm head) and handed out through
    /// [`Self::nested_cons_cursor`] as a GLOBALLY-unique supply, mirroring
    /// [`Self::param_binders`].
    nested_cons_binders: Vec<Symbol>,
    /// Monotonic cursor into [`Self::nested_cons_binders`], mirroring
    /// [`Self::param_cursor`]'s shape exactly.
    nested_cons_cursor: Cell<usize>,
    /// Pre-minted, collision-free `String` binder names for the sibling
    /// nested-string-literal desugaring — a `PStr` sub-pattern DIRECTLY
    /// nested in a `PCtor` arm payload (`Just "live"`) lowers to a fresh
    /// `String` binder in that ctor-arg slot plus an arm guard
    /// (`binder == "live"`), instead of a bare literal pattern. Rust cannot
    /// literal-match a `&str` pattern against an owned `String` ctor FIELD
    /// (`SkyMaybe::Just(String)`) the way it can against a raw `&str`
    /// scrutinee — the same "a Vec/String enum field cannot be pattern-
    /// matched inline" shape [`Self::nested_cons_binders`] documents, applied
    /// to `String` instead of `List`. Sized by
    /// [`count_nested_strlit_payload_sites`] (an upper bound over every
    /// `case`-arm head) and handed out through [`Self::nested_strlit_cursor`]
    /// as a GLOBALLY-unique supply, mirroring [`Self::param_binders`].
    nested_strlit_binders: Vec<Symbol>,
    /// Monotonic cursor into [`Self::nested_strlit_binders`], mirroring
    /// [`Self::param_cursor`]'s shape exactly.
    nested_strlit_cursor: Cell<usize>,
    /// Home module path of the def currently being lowered.  Set at the start of
    /// each [`Self::lower_def`] call; read by [`Self::region_ty`] to key the
    /// region map as `(home, span)` — matching the discriminant the constraint
    /// builder recorded.  Interior mutability so `lower_def` can update it through
    /// the shared `&self` reference that the lowering walk uses.
    current_home: std::cell::RefCell<Vec<Symbol>>,
    /// Reverse map from union-find representative id to annotation variable
    /// symbol for the typed def currently being lowered.  Populated by
    /// [`Self::lower_def`] from [`SolvedTypes::poly_var_map`] before recursing
    /// into the body; cleared (restored to empty) afterward.
    ///
    /// Used by [`Self::ir_type_from_ty_ui_msg`] to distinguish a `Ty::Var` that
    /// represents an enclosing generic type parameter (→ `IrType::Generic(sym)`)
    /// from a `Ty::Var` that is a truly unconstrained, message-free UI subtree
    /// placeholder (→ `IrType::Unit`, which emits `Html<()>` / `Attribute<()>`).
    ///
    /// Without this distinction every `Ty::Var` in a UI msg position was mapped
    /// to `IrType::Unit`, causing E0308 (`Attribute<()>` vs `Attribute<T1>`) in
    /// the Rust emitted for polymorphic functions such as
    /// `view : (Msg -> parentMsg) -> Counter -> Html parentMsg`.
    current_poly_tvars: std::cell::RefCell<BTreeMap<u32, Symbol>>,
    /// Whether the function (def or lambda) currently being lowered has a Task
    /// return type. Set to `true` when `lower_def` / `lower_lambda` detects that
    /// the inferred return type is `IrType::Task(_)`; reset to `false` on entry to
    /// each new def/lambda scope and restored on exit (save/set/restore pattern).
    /// Read by `lower_let`'s `PAnything` arm to choose between `Expr::TaskSeq`
    /// (async context — emit `task_and_then(...)`) and `Expr::TaskSeqSync`
    /// (sync context — emit `{ let _ = task_run(...); rest }`).  Interior
    /// mutability so the lowering walk stays over a shared `&self`.
    fn_is_async: Cell<bool>,
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
    // ── Order ADT (#123) ─────────────────────────────────────────────────────
    pub order: Symbol,
    pub lt: Symbol,
    pub eq: Symbol,
    pub gt: Symbol,
    // ── Error / ErrorKind ADTs (E-12, #152) ──────────────────────────────────
    // `Error` has one constructor also named `Error` (arity 2).
    // `ErrorKind` has 11 nullary constructors.
    pub error: Symbol,
    pub errorkind: Symbol,
    pub ek_io: Symbol,
    pub ek_network: Symbol,
    pub ek_ffi: Symbol,
    pub ek_decode: Symbol,
    pub ek_timeout: Symbol,
    pub ek_not_found: Symbol,
    pub ek_permission_denied: Symbol,
    pub ek_invalid_input: Symbol,
    pub ek_conflict: Symbol,
    pub ek_unavailable: Symbol,
    pub ek_unexpected: Symbol,
    // ── ErrorDetails ADT (backlog #85 follow-up) ──────────────────────────────
    // `ErrorDetails` has 5 constructors, each arity 1.
    pub errordetails: Symbol,
    pub ed_ffi_panic: Symbol,
    pub ed_type_mismatch: Symbol,
    pub ed_http_status: Symbol,
    pub ed_json_decode: Symbol,
    pub ed_custom: Symbol,
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

/// Count every bare `any`-wildcard occurrence in PARAM position across every
/// [`canon::Def::Typed`] annotation in the module — the pre-sizing pass for
/// [`Lowerer::any_param_binders`] (AUD-01 seal fix: each occurrence needs its
/// OWN fresh symbol so it doesn't collapse onto every other `any` occurrence's
/// shared interned Symbol; see [`Lowerer::split_typed_sig`]).
///
/// Only `any` can appear as a bare param-position type variable without being
/// quantified by the def (a genuine type parameter is fine to share — only
/// `any` gets a fresh flex UV per occurrence in the checker). Only walks the
/// PARAM positions of the top-level annotation's arrow chain — the return
/// position is handled separately (the existing region-based return-`any`
/// substitution) and lambdas never carry their own annotation in Sky, so no
/// body/lambda recursion is needed here (unlike
/// [`count_destructure_param_sites`]).
///
/// Over-counting is harmless (a few unused interned symbols); under-counting
/// would let [`Lowerer::fresh_any_param_symbol`]'s cursor overrun, which fails
/// closed as a [`bug`] — never an index panic, never a silent reuse.
pub fn count_any_param_sites(m: &canon::Module, interner: &Interner) -> usize {
    fn is_any_var(t: &canon::Type, interner: &Interner) -> bool {
        matches!(t, canon::Type::Var(v) if interner.resolve(*v) == Some("any"))
    }
    m.defs
        .iter()
        .map(|d| {
            let canon::Def::Typed { ty, .. } = d else {
                return 0;
            };
            let mut cur = ty;
            let mut n = 0;
            while let canon::Type::Lambda(arg, rest) = cur {
                if is_any_var(arg, interner) {
                    n += 1;
                }
                cur = rest.as_ref();
            }
            n
        })
        .sum()
}

/// Count every destructure-binder `let` binding AND single-arm product
/// `case` in the module — one pre-minted symbol needed per site for #125's
/// Decoder-thunk generalization (spec §2.6), REGARDLESS of whether that
/// binding ultimately turns out to be Decoder-typed (the type-dependent
/// gate runs later, once solving has completed; this pass is purely
/// syntactic, like its [`count_destructure_param_sites`] sibling). A `let`
/// binding counts whenever its pattern is neither `PVar` nor `PAnything` —
/// exactly the set that reaches [`Lowerer::lower_let`]'s destructure
/// catch-all; a `case` counts when it has exactly one arm whose head is a
/// product destructure ([`Lowerer::is_destructure_head`]'s shape). Over-
/// counting is harmless; under-counting fails closed as a [`bug`], never an
/// index panic.
pub fn count_destructure_thunk_sites(m: &canon::Module) -> usize {
    const fn is_thunk_countable_binding(pat: &canon::Pattern_) -> bool {
        !matches!(
            pat,
            canon::Pattern_::PVar(_) | canon::Pattern_::PAnything
        )
    }
    fn is_destructure_headed(pat: &canon::Pattern_) -> bool {
        match pat {
            canon::Pattern_::PTuple(_) | canon::Pattern_::PRecord(_) => true,
            canon::Pattern_::PAlias(inner, _) => is_destructure_headed(&inner.value),
            _ => false,
        }
    }
    fn walk_expr(e: &canon::Expr) -> usize {
        match &e.value {
            canon::Expr_::Let(bindings, body) => {
                bindings
                    .iter()
                    .map(|b| {
                        usize::from(is_thunk_countable_binding(&b.pat.value)) + walk_expr(&b.body)
                    })
                    .sum::<usize>()
                    + walk_expr(body)
            }
            canon::Expr_::Case(scrut, branches) => {
                let head = branches.len() == 1
                    && branches
                        .first()
                        .is_some_and(|b| is_destructure_headed(&b.pat.value));
                usize::from(head)
                    + walk_expr(scrut)
                    + branches.iter().map(|b| walk_expr(&b.body)).sum::<usize>()
            }
            // Every other recursive arm mirrors
            // `count_destructure_param_sites`'s `walk_expr` shape.
            canon::Expr_::Lambda(_, body) => walk_expr(body),
            canon::Expr_::Call(callee, args) => {
                walk_expr(callee) + args.iter().map(walk_expr).sum::<usize>()
            }
            canon::Expr_::Binop { lhs, rhs, .. } => walk_expr(lhs) + walk_expr(rhs),
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
            // Leaves host no let / case.
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
            canon::Def::Typed { body, .. } | canon::Def::Untyped { body, .. } => walk_expr(body),
        })
        .sum()
}

/// Count `case`-arm sites that need a fresh payload binder for the Class 4
/// item C2 (#158) nested-cons desugaring: a `PList` / `PCons` sub-pattern that
/// is a DIRECT argument of a `PCtor` arm head (`Just (h :: t)`, `Ok [a, b]`).
/// Each such argument lowers to one fresh `Vec` binder plus an arm guard.
///
/// The count is deliberately an UPPER BOUND — it walks every `case` arm HEAD in
/// the module and counts every nested `PList` / `PCons` argument regardless of
/// whether that arm ultimately takes the C2 path (a same-position wildcard arm,
/// a fully-generic element type, etc. may bail out before minting a binder). An
/// over-count is harmless: unused pool entries are simply never handed out, the
/// same policy [`count_destructure_thunk_sites`] documents.
pub fn count_nested_cons_payload_sites(m: &canon::Module) -> usize {
    fn direct_list_args(pat: &canon::Pattern_) -> usize {
        match pat {
            canon::Pattern_::PCtor { args, .. } => args
                .iter()
                .map(|a| {
                    usize::from(matches!(
                        a.value,
                        canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _)
                    )) + direct_list_args(&a.value)
                })
                .sum(),
            canon::Pattern_::PTuple(elems) => {
                elems.iter().map(|e| direct_list_args(&e.value)).sum()
            }
            canon::Pattern_::PAlias(inner, _) => direct_list_args(&inner.value),
            _ => 0,
        }
    }
    fn walk_expr(e: &canon::Expr) -> usize {
        match &e.value {
            canon::Expr_::Let(bindings, body) => {
                bindings.iter().map(|b| walk_expr(&b.body)).sum::<usize>() + walk_expr(body)
            }
            canon::Expr_::Case(scrut, branches) => {
                walk_expr(scrut)
                    + branches
                        .iter()
                        .map(|b| direct_list_args(&b.pat.value) + walk_expr(&b.body))
                        .sum::<usize>()
            }
            canon::Expr_::Lambda(_, body) => walk_expr(body),
            canon::Expr_::Call(callee, args) => {
                walk_expr(callee) + args.iter().map(walk_expr).sum::<usize>()
            }
            canon::Expr_::Binop { lhs, rhs, .. } => walk_expr(lhs) + walk_expr(rhs),
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
            canon::Def::Typed { body, .. } | canon::Def::Untyped { body, .. } => walk_expr(body),
        })
        .sum()
}

/// Count `case`-arm sites that need a fresh payload binder for the sibling
/// nested-string-literal desugaring: a `PStr` sub-pattern that is a DIRECT
/// argument of a `PCtor` arm head (`Just "live"`, `Ok "done"`). Each such
/// argument lowers to one fresh `String` binder plus an arm guard
/// (`binder == "live"`) — the same "an enum FIELD cannot be inline-pattern-
/// matched" shape [`count_nested_cons_payload_sites`] documents for
/// `PList`/`PCons`, mirrored here for `PStr` (a ctor's `String` field cannot
/// take a bare `&str` literal pattern the way a top-level `String`
/// scrutinee's `.as_str()` coercion allows).
///
/// The count is deliberately an UPPER BOUND, exactly mirroring
/// [`count_nested_cons_payload_sites`]'s policy: it walks every `case` arm
/// HEAD in the module and counts every nested `PStr` argument regardless of
/// whether that arm ultimately takes this desugaring path. An over-count is
/// harmless — unused pool entries are simply never handed out.
pub fn count_nested_strlit_payload_sites(m: &canon::Module) -> usize {
    fn direct_strlit_args(pat: &canon::Pattern_) -> usize {
        match pat {
            canon::Pattern_::PCtor { args, .. } => args
                .iter()
                .map(|a| {
                    usize::from(matches!(a.value, canon::Pattern_::PStr(_)))
                        + direct_strlit_args(&a.value)
                })
                .sum(),
            canon::Pattern_::PTuple(elems) => {
                elems.iter().map(|e| direct_strlit_args(&e.value)).sum()
            }
            canon::Pattern_::PAlias(inner, _) => direct_strlit_args(&inner.value),
            _ => 0,
        }
    }
    fn walk_expr(e: &canon::Expr) -> usize {
        match &e.value {
            canon::Expr_::Let(bindings, body) => {
                bindings.iter().map(|b| walk_expr(&b.body)).sum::<usize>() + walk_expr(body)
            }
            canon::Expr_::Case(scrut, branches) => {
                walk_expr(scrut)
                    + branches
                        .iter()
                        .map(|b| direct_strlit_args(&b.pat.value) + walk_expr(&b.body))
                        .sum::<usize>()
            }
            canon::Expr_::Lambda(_, body) => walk_expr(body),
            canon::Expr_::Call(callee, args) => {
                walk_expr(callee) + args.iter().map(walk_expr).sum::<usize>()
            }
            canon::Expr_::Binop { lhs, rhs, .. } => walk_expr(lhs) + walk_expr(rhs),
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
            canon::Def::Typed { body, .. } | canon::Def::Untyped { body, .. } => walk_expr(body),
        })
        .sum()
}

/// Every pre-minted, collision-free synthetic-symbol pool [`Lowerer::new`]
/// needs — bundled into one argument so the constructor stays under
/// clippy's arg-count ceiling. Each field is documented at its matching
/// [`Lowerer`] struct field (the pools are stored flat there; this type
/// exists only to keep `new`'s signature small, not as an ongoing grouping).
pub struct SymbolPools {
    pub eta_params: Vec<Symbol>,
    pub cap_params: Vec<Symbol>,
    pub param_binders: Vec<Symbol>,
    pub any_param_binders: Vec<Symbol>,
    pub destructure_thunk_binders: Vec<Symbol>,
    pub nested_cons_binders: Vec<Symbol>,
    pub nested_strlit_binders: Vec<Symbol>,
}

/// `(params, prologue, ret, any_syms_minted)` — [`Lowerer::split_typed_sig`]'s
/// return shape, named so the signature stays under clippy's type-complexity
/// ceiling. `any_syms_minted` (AUD-01 seal fix) lists every fresh symbol
/// handed out by [`Lowerer::fresh_any_param_symbol`] for THIS call — the
/// caller must union these into whatever set gates `type_params` (they are
/// NOT in [`canon::Def::Typed::free_vars`], since they didn't exist at canon
/// time), or the backend would reference an undeclared Rust generic.
type TypedSigParts = (Vec<IrParam>, Vec<ParamPrologue>, IrType, Vec<Symbol>);

impl<'a> Lowerer<'a> {
    #[allow(clippy::too_many_lines)] // Error/ErrorKind ADT seeding (E-12/#152) pushed it over 100
    pub fn new(
        m: &'a canon::Module,
        types: &'a SolvedTypes,
        interner: &'a Interner,
        pools: SymbolPools,
        builtins: &'a BuiltinCtors,
    ) -> Self {
        let SymbolPools {
            eta_params,
            cap_params,
            param_binders,
            any_param_binders,
            destructure_thunk_binders,
            nested_cons_binders,
            nested_strlit_binders,
        } = pools;
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
        ctor_arity.insert((prelude_home.clone(), builtins.omit_field), 0);
        // ── Order ADT (#123) ─────────────────────────────────────────────────
        enum_variants.insert(
            (prelude_home.clone(), builtins.order),
            vec![builtins.lt, builtins.eq, builtins.gt],
        );
        ctor_arity.insert((prelude_home.clone(), builtins.lt), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.eq), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.gt), 0);
        // ── Error / ErrorKind ADTs (E-12, #152) ─────────────────────────────────
        // `Error` is a single-constructor ADT: `Error ErrorKind ErrorInfo`.
        // `ErrorKind` has 11 nullary variants.
        // Both are Prelude built-ins — no user `type` declaration in Sky source.
        // Seeding them here lets `case e of Error kind info ->` validate and lower
        // past the `Match::new` enum-cover check, following the same pattern as
        // `Maybe` / `Result` / `SqlValue` / `Order` above.
        enum_variants.insert(
            (prelude_home.clone(), builtins.error),
            vec![builtins.error], // sole constructor has the same name as the type
        );
        ctor_arity.insert((prelude_home.clone(), builtins.error), 2); // Error(ErrorKind, ErrorInfo)
        enum_variants.insert(
            (prelude_home.clone(), builtins.errorkind),
            vec![
                builtins.ek_io,
                builtins.ek_network,
                builtins.ek_ffi,
                builtins.ek_decode,
                builtins.ek_timeout,
                builtins.ek_not_found,
                builtins.ek_permission_denied,
                builtins.ek_invalid_input,
                builtins.ek_conflict,
                builtins.ek_unavailable,
                builtins.ek_unexpected,
            ],
        );
        ctor_arity.insert((prelude_home.clone(), builtins.ek_io), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_network), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_ffi), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_decode), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_timeout), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_not_found), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_permission_denied), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_invalid_input), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_conflict), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_unavailable), 0);
        ctor_arity.insert((prelude_home.clone(), builtins.ek_unexpected), 0);
        // ── ErrorDetails ADT (backlog #85 follow-up) ─────────────────────────────
        // 5-variant enrichment union carried on `ErrorInfo.details`. Same
        // registration recipe as `ErrorKind` above — seeding here lets
        // `case d of FfiPanic info -> …` / `HttpStatus code -> …` validate and
        // lower past the `Match::new` enum-cover check.
        enum_variants.insert(
            (prelude_home.clone(), builtins.errordetails),
            vec![
                builtins.ed_ffi_panic,
                builtins.ed_type_mismatch,
                builtins.ed_http_status,
                builtins.ed_json_decode,
                builtins.ed_custom,
            ],
        );
        ctor_arity.insert((prelude_home.clone(), builtins.ed_ffi_panic), 1); // FfiPanic(PanicInfo)
        ctor_arity.insert((prelude_home.clone(), builtins.ed_type_mismatch), 1); // TypeMismatch(TypeInfo)
        ctor_arity.insert((prelude_home.clone(), builtins.ed_http_status), 1); // HttpStatus(Int)
        ctor_arity.insert((prelude_home.clone(), builtins.ed_json_decode), 1); // JsonDecode(String)
        ctor_arity.insert((prelude_home, builtins.ed_custom), 1); // Custom(String), final move

        Self {
            m,
            types,
            interner,
            builtins,
            func_ids,
            enum_variants,
            ctor_arity,
            eta_params,
            cap_params,
            param_binders,
            param_cursor: Cell::new(0),
            any_param_binders,
            any_param_cursor: Cell::new(0),
            destructure_thunk_binders,
            destructure_thunk_cursor: Cell::new(0),
            nested_cons_binders,
            nested_cons_cursor: Cell::new(0),
            nested_strlit_binders,
            nested_strlit_cursor: Cell::new(0),
            current_home: std::cell::RefCell::new(Vec::new()),
            current_poly_tvars: std::cell::RefCell::new(BTreeMap::new()),
            fn_is_async: Cell::new(false),
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

    /// Hand out the next globally-unique fresh symbol from
    /// [`Self::any_param_binders`] for the per-occurrence `any`-in-param-
    /// position seal fix (AUD-01). Mirrors [`Self::fresh_param_binder`]
    /// exactly; sized by [`count_any_param_sites`], so an overrun is an
    /// internal invariant violation, never an index panic.
    fn fresh_any_param_symbol(&self) -> DResult<Symbol> {
        let i = self.any_param_cursor.get();
        let sym = *self.any_param_binders.get(i).ok_or_else(|| {
            bug(
                "sky_lower::fresh_any_param_symbol",
                "any-param-position fresh-symbol pool exhausted",
            )
        })?;
        self.any_param_cursor.set(i + 1);
        Ok(sym)
    }

    /// Hand out the next globally-unique #125 destructure-thunk binder from
    /// [`Self::destructure_thunk_binders`]. Mirrors
    /// [`Self::fresh_param_binder`] exactly; sized by
    /// [`count_destructure_thunk_sites`], so an overrun is an internal
    /// invariant violation, never an index panic.
    fn fresh_destructure_thunk_symbol(&self) -> DResult<Symbol> {
        let i = self.destructure_thunk_cursor.get();
        let sym = *self.destructure_thunk_binders.get(i).ok_or_else(|| {
            bug(
                "sky_lower::fresh_destructure_thunk_symbol",
                "destructure-thunk-binder pool exhausted",
            )
        })?;
        self.destructure_thunk_cursor.set(i + 1);
        Ok(sym)
    }

    /// Hand out the next globally-unique Class 4 item C2 (#158) nested-cons
    /// payload binder from [`Self::nested_cons_binders`]. Mirrors
    /// [`Self::fresh_param_binder`] exactly; sized by
    /// [`count_nested_cons_payload_sites`] (an upper bound), so an overrun is an
    /// internal invariant violation, never an index panic.
    fn fresh_nested_cons_binder(&self) -> DResult<Symbol> {
        let i = self.nested_cons_cursor.get();
        let sym = *self.nested_cons_binders.get(i).ok_or_else(|| {
            bug(
                "sky_lower::fresh_nested_cons_binder",
                "nested-cons-payload binder pool exhausted",
            )
        })?;
        self.nested_cons_cursor.set(i + 1);
        Ok(sym)
    }

    /// Hand out the next globally-unique nested-string-literal payload binder
    /// from [`Self::nested_strlit_binders`]. Mirrors
    /// [`Self::fresh_nested_cons_binder`] exactly; sized by
    /// [`count_nested_strlit_payload_sites`] (an upper bound), so an overrun
    /// is an internal invariant violation, never an index panic.
    fn fresh_nested_strlit_binder(&self) -> DResult<Symbol> {
        let i = self.nested_strlit_cursor.get();
        let sym = *self.nested_strlit_binders.get(i).ok_or_else(|| {
            bug(
                "sky_lower::fresh_nested_strlit_binder",
                "nested-string-literal-payload binder pool exhausted",
            )
        })?;
        self.nested_strlit_cursor.set(i + 1);
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

    /// Look up the solved type for a source `span` in the current def's home
    /// module.
    ///
    /// The region map is keyed by `(home_module_path, Span)` to prevent
    /// cross-module span collisions after `link::link` merges dep modules.
    /// This helper reads [`Self::current_home`] (set by [`Self::lower_def`])
    /// and constructs the composite key automatically, so callers need only
    /// supply the span.
    #[inline]
    fn region_ty(&self, span: Span) -> Option<&Ty> {
        let home = self.current_home.borrow().clone();
        self.types.regions.get(&(home, span))
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
        for (idx, def) in self.m.defs.iter().enumerate() {
            // Positional id: `func_ids` was assigned from this very
            // enumeration order in `new()` under the unique-`(home, name)`
            // module invariant, so the positional id equals the map-resolved
            // id — passing it spares `lower_def` a throwaway `Vec<Symbol>`
            // key allocation per def (efficiency-audit §3 low).
            let id = FuncId::from_raw(u32::try_from(idx).unwrap_or(u32::MAX));
            let func = self.lower_def(def, id)?;
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
        // All nine kernel-family flags are collected in ONE pass over the
        // function bodies (see [`KernelUsage`]) instead of nine independent
        // full-AST walks (efficiency-audit §3 medium).
        let mut kernel_usage = KernelUsage::default();
        for f in &funcs {
            if kernel_usage.all_set() {
                break;
            }
            scan_kernel_usage(&f.body, &mut kernel_usage);
        }

        if kernel_usage.db {
            types_ir.push(TypeDef::Enum(self.synthetic_sqlvalue_enum()));
            types_ir.push(TypeDef::Enum(self.synthetic_sqlfield_enum()));
        }

        let records = self.collect_record_types()?;

        // M5c: detect whether any TEA kernel call is present. The backend uses
        // this flag to append `pub mod tea; pub use tea::*;` to mod.rs and to
        // add `SkyCmd<M>` / `SkySub<M>` type aliases.
        let uses_tea = kernel_usage.tea;

        // M6: detect whether any Sky.Http.Server kernel call is present. The
        // backend uses this flag to inject the `server` feature in Cargo.toml
        // and append `pub mod server; pub use server::*; pub mod server_stream;
        // pub use server_stream::*;` to mod.rs.
        let uses_server = kernel_usage.server;

        // M7: detect Std.Ui / Std.Html / Std.Live / Std.Tui / Std.Webview usage.
        // TUI runtime files (tui/app.rs, tui/layout.rs, tui/focus.rs) import
        // `super::super::ui` and `super::super::html` unconditionally, so
        // `uses_ui` must be true whenever `uses_tui` is true — even when the
        // Sky source only calls `Ui.column`/`Ui.el`/`Ui.text` (kernels that
        // trigger `uses_tui`) and never calls `Ui.layout`/`Ui.layoutWith`
        // (kernels that trigger `uses_ui`).
        let uses_tui = kernel_usage.tui;
        let uses_ui = kernel_usage.ui || uses_tui;
        let uses_live = kernel_usage.live;
        let uses_webview = kernel_usage.webview;

        // #47: detect Std.Css (Sky.Core.CssSafety) leaf-kernel usage. Independent
        // of `uses_ui` — a pure-Std.Css program uses no render kernel.
        let uses_css = kernel_usage.css;

        // #111: detect Std.Auth kernel usage — any of hashPassword, verifyPassword,
        // signToken, verifyToken, register, login, setRole, and companions.  The
        // backend uses this flag to append `pub mod auth; pub use auth::*;` to
        // the emitted `sky_runtime/mod.rs`.
        let uses_auth = kernel_usage.auth;

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
            uses_auth,
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
    /// One fail-closed gate runs per constructor, surfaced as a span-carrying
    /// [`Diagnostic::Lower`] rather than emitting Rust that cargo rejects: a
    /// field type variable not bound by the union's parameters (`type Foo a =
    /// Bar b`) would have no Rust generic to resolve to — the polymorphism gap
    /// ([`Feature::Polymorphism`]).
    ///
    /// A field whose type embeds a function (`type Retryish e = RetryWhen (e ->
    /// Bool)`) is NOT gated here (#90) — #87's derive-demotion fixpoint keeps
    /// the emitted enum sound (see the field-loop comment below).
    /// Search a canonical constructor-payload type for a mis-arity `Task`
    /// application (any arity other than the internal unary `Task a` or the
    /// canonical `Task Error a`), returning the offending argument count.
    ///
    /// A mis-arity `Task` reached through a constructor FIELD type
    /// (`type J a = J (Task Error a Bool)`) never passes through
    /// `normalize_annotation_ty`, so E1's SKY-T0016 gate does not fire — it
    /// reaches `ir_type_from_canon`'s `"Task"` dispatch directly, which would
    /// otherwise raise a `CompilerBug` ICE. This predicate lets `lower_enum`
    /// fail closed with the SAME clean SKY-T0016 diagnostic before that happens.
    /// A mis-arity `Task` in a constructor field is ALWAYS wrong, never a
    /// legitimate program.
    /// Find a mis-arity async carrier (`Task`/`Cmd`/`Sub`) anywhere in a
    /// canonical type, returning `(carrier_name, found_arity)`. `Task` is
    /// well-formed at arity 1 (internal unary) or 2 (`Task Error a`);
    /// `Cmd`/`Sub` take exactly 1 (`Cmd msg`). Any other arity would trip
    /// `ir_type_from_canon`'s catch-all `CompilerBug` (SKY-I0001), so
    /// `lower_enum`'s Gate 0a fails closed on it with a clean SKY-T0016.
    /// (Cmd/Sub coverage added after the #32 review found the Task-only gate
    /// left the siblings ICE-ing, contrary to the item title.)
    fn task_arity_in_canon(&self, t: &canon::Type) -> Option<(&'static str, usize)> {
        match t {
            canon::Type::Con { name, args, .. } => {
                match self.interner.resolve(*name) {
                    Some("Task") if args.len() != 1 && args.len() != 2 => {
                        return Some(("Task", args.len()));
                    }
                    Some("Cmd") if args.len() != 1 => return Some(("Cmd", args.len())),
                    Some("Sub") if args.len() != 1 => return Some(("Sub", args.len())),
                    _ => {}
                }
                args.iter().find_map(|a| self.task_arity_in_canon(a))
            }
            canon::Type::Lambda(a, b) => self
                .task_arity_in_canon(a)
                .or_else(|| self.task_arity_in_canon(b)),
            canon::Type::Tuple(elems) => elems.iter().find_map(|e| self.task_arity_in_canon(e)),
            canon::Type::Record(fields) => {
                fields.iter().find_map(|(_, ty)| self.task_arity_in_canon(ty))
            }
            canon::Type::Var(_) | canon::Type::Unit => None,
        }
    }

    fn lower_enum(&self, u: &canon::Union) -> DResult<EnumDef> {
        let type_params = u.vars.clone();
        let mut variants = Vec::with_capacity(u.ctors.len());
        for ctor in &u.ctors {
            let mut fields = Vec::with_capacity(ctor.args.len());
            for arg in &ctor.args {
                // Gate 0a: a mis-arity `Task` in a constructor payload
                // (`J (Task Error a Bool)`) would trip `ir_type_from_canon`'s
                // `"Task"` catch-all `CompilerBug`. Fail closed with the clean
                // SKY-T0016 diagnostic (`TypeError::TaskArity`) at the ctor span,
                // matching E1's annotation-path behaviour. A well-formed
                // `Task Error a` (arity 2) is NOT rejected here — it lowers to a
                // `Variant` carrying `IrType::Task`, and #87's derive-demotion
                // fixpoint degrades a non-derivable enum gracefully.
                if let Some((carrier, found)) = self.task_arity_in_canon(arg) {
                    return Err(Diagnostic::Type {
                        span: ctor.span,
                        msg: sky_diagnostics::TypeError::TaskArity { carrier, found },
                    });
                }
                // Gate 1: every field type variable must be one the union
                // quantifies, so it resolves to a Rust generic by position.
                // Exception: `any` wildcard is the pub/sub wire-carrier pin
                // (Dict String String) — excluded from the bound check,
                // mirroring the reference's `(/= "any") freeVars` filter
                // (DeclaredArityHelperSpec.hs:43). `ir_type_from_canon` maps
                // it to the concrete IrType::Dict(Str, Str) below.
                let mut vars = BTreeSet::new();
                collect_type_vars(arg, &mut vars);
                if !vars.iter().all(|v| {
                    type_params.contains(v)
                        || self.interner.resolve(*v).is_some_and(|n| n == "any")
                }) {
                    return Err(unsupported(ctor.span, Feature::Polymorphism));
                }
                let ir = self.ir_type_from_canon(arg, &type_params)?;
                // #90: a function-bearing payload field (`type Retryish e =
                // RetryWhen (e -> Bool)`) is SOUND to declare — #87's
                // derive-demotion fixpoint (`enum_is_derivable`,
                // `sky_backend_rust::emit_types`) drops the enum's
                // `#[derive(Clone, Debug, PartialEq)]` whenever any field
                // (transitively) embeds `IrType::Fun`, and the hand-written
                // `SkyStringify` impl renders a non-derivable field as the
                // `<fn>` placeholder instead of calling a derive. No gate
                // needed at declaration time; see
                // `docs/architecture/ctor-payload-function-design.md`.
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
        // O(1) dedup gate alongside the ordered Vec (efficiency-audit §3
        // medium: the former `out.contains(&ir)` was an O(n²) scan over
        // every region/env record shape). `out` keeps the same ordering and
        // the same element set — the set only gates insertion.
        let mut seen: std::collections::HashSet<IrType> = std::collections::HashSet::new();
        for ty in self.types.regions.values().chain(self.types.env.values()) {
            self.collect_records_in_ty(ty, &mut out, &mut seen)?;
        }
        Ok(out)
    }

    /// Walk a solved [`Ty`], pushing every distinct record shape it contains
    /// (nested records first) into `out`. Non-record shapes recurse into their
    /// children; leaves contribute nothing.
    fn collect_records_in_ty(
        &self,
        ty: &Ty,
        out: &mut Vec<IrType>,
        seen: &mut std::collections::HashSet<IrType>,
    ) -> DResult<()> {
        match ty {
            Ty::Record(fields, _tail) => {
                for field_ty in fields.values() {
                    self.collect_records_in_ty(field_ty, out, seen)?;
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
                    //
                    // EXCEPTION — `RetryPolicy e`: this anonymous record (identified
                    // by a `shouldRetry` field) IS materialised as a runtime value
                    // passed to `task_retry_with`.  Its Rust struct is emitted by
                    // `emit_task_retry_call` and MUST be registered here despite
                    // carrying a function-typed field.  The backend emits the struct
                    // with `shouldRetry: Box<dyn Fn(…) -> …>` and skips the `Clone`
                    // / `PartialEq` derives for that field.
                    let is_retry_policy = fields
                        .keys()
                        .any(|k| self.interner.resolve(*k) == Some("shouldRetry"));
                    if (!ir_contains_fun(&ir) || is_retry_policy) && seen.insert(ir.clone()) {
                        out.push(ir);
                    }
                }
            }
            Ty::Tuple(elems) => {
                for e in elems {
                    self.collect_records_in_ty(e, out, seen)?;
                }
            }
            Ty::Fun(a, b) => {
                self.collect_records_in_ty(a, out, seen)?;
                self.collect_records_in_ty(b, out, seen)?;
            }
            Ty::Con { args, .. } => {
                for a in args {
                    self.collect_records_in_ty(a, out, seen)?;
                }
            }
            Ty::Var(_) | Ty::Unit => {}
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // grew past 100 with the T5 multi-use-clone pre-pass (#104)
    fn lower_def(&self, def: &canon::Def, id: FuncId) -> DResult<Func> {
        // Track the current def's home so every `region_ty(span)` lookup uses the
        // correct `(home, span)` key, matching what the constraint builder wrote.
        *self.current_home.borrow_mut() = def.home().to_vec();

        // `id` is the def's position in `m.defs` — identical to the
        // `func_ids` entry `new()` recorded for `(home, name)` (the map is
        // populated from the same enumeration), passed in to avoid a
        // throwaway `Vec<Symbol>` lookup key per def.
        let name = def.name().value;

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
                //
                // `ir_type_from_canon` now handles every stdlib opaque alias
                // (including `HttpRequest`) directly, so there is no need for a
                // separate value-binding fast path that bypasses it.  Always use
                // `split_typed_sig` — it routes through `ir_type_from_canon` for
                // the annotation type and handles both 0-parameter and N-parameter
                // bindings correctly.
                // `Handler` / `Middleware` are transparent stdlib function
                // aliases (`Handler = Request -> Task Error Response`,
                // `Middleware = Handler -> Handler`). At the annotation level they
                // are a nullary `Con`, so `split_typed_sig` would see ZERO arrows
                // for a handler that binds `req` and raise the "annotation has
                // fewer arrows than parameters" ICE. The type checker has already
                // unfolded the alias — the binding's SOLVED type in `types.env` is
                // the full arrow chain — so split THAT instead (identical to the
                // unannotated path). Only whole-annotation aliases are unfolded
                // here; a `Handler` in argument position (`withCors : … -> Handler
                // -> Handler`) still lowers via `split_typed_sig` unchanged.
                let (params, prologue, ret, any_syms_minted) = if !patterns.is_empty()
                    && self.annotation_is_function_alias(ty)
                {
                    let solved_ty = self
                        .types
                        .env
                        .get(&(def.home().to_vec(), name))
                        .ok_or_else(|| {
                            bug(
                                "sky_lower::lower_def",
                                "no inferred type for function-alias binding",
                            )
                        })?;
                    // The solved-type path never encounters a bare `any`-wildcard
                    // Generic (a solved type is either concrete or a free
                    // `Ty::Var`, never carrying the annotation-only `any` marker)
                    // — nothing minted here.
                    let (p, pr, r) = self.split_unannotated_sig(solved_ty, patterns, sig_span)?;
                    (p, pr, r, Vec::new())
                } else {
                    self.split_typed_sig(ty, patterns, free_vars)?
                };
                // Bug-29 fix: `view : Model -> any` where the body region is a UI
                // type `Html<Ty::Var(uv)>`.  We need to inject `(uv_rep →
                // any_sym)` into the poly_tvars map so that
                // `ir_type_from_ty_ui_msg(Ty::Var(uv))` returns
                // `IrType::Generic(any_sym)` (→ `Html<T0>`) instead of
                // `IrType::Unit` (→ `Html<()>`).  Without this, the emitted
                // function returns `Html<()>` which mismatches `webview_app`'s
                // `FView: Fn(Model) -> Html<Msg>` bound (E0271).
                //
                // Detection: if the annotation return is `IrType::Generic(sym)`
                // where `sym` resolves to "any" AND the body region is a
                // `Ty::Con` with exactly one arg that is a bare `Ty::Var`,
                // record `(uv_rep, any_sym)` for injection in the poly_tvars
                // installation block below.
                //
                // The injection is performed in the poly_tvars installation
                // block (next) so that the `ir_type_from_ty(body_ty)` call in
                // the any-ret fix (after the installation) already runs with the
                // correct current_poly_tvars.
                let any_ui_msg_injection: Option<(u32, Symbol)> =
                    if let IrType::Generic(sym) = &ret {
                        if self.interner.resolve(*sym) == Some("any") {
                            self.types
                                .regions
                                .get(&(def.home().to_vec(), body.span))
                                .and_then(|body_ty| {
                                    let Ty::Con { args, .. } = body_ty else {
                                        return None;
                                    };
                                    let Some(Ty::Var(uv)) = args.first() else {
                                        return None;
                                    };
                                    Some((*uv, *sym))
                                })
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                // Install the binding's generic type-variable map so
                // `ir_type_from_ty_ui_msg` can distinguish a `Ty::Var` that is an
                // enclosing generic (→ `IrType::Generic`) from one that is a
                // message-free UI subtree placeholder (→ `IrType::Unit`).  The map
                // is cleared (restored to empty) after the body is lowered, so
                // nested annotated lambdas only see their own outer binding's map —
                // lambdas are not `Def::Typed`, so there is no inner install that
                // would conflict.
                //
                // Bug-29: `any_ui_msg_injection` may augment the map with a UI
                // msg UV → any_sym mapping so the subsequent `ir_type_from_ty`
                // call produces `IrType::Generic(any_sym)` instead of Unit.
                let poly_key = (def.home().to_vec(), name);
                let saved_poly_tvars = {
                    let mut poly = self
                        .types
                        .poly_var_map
                        .get(&poly_key)
                        .cloned()
                        .unwrap_or_default();
                    if let Some((uv_rep, any_sym)) = any_ui_msg_injection {
                        poly.insert(uv_rep, any_sym);
                    }
                    let mut slot = self.current_poly_tvars.borrow_mut();
                    let saved = slot.clone();
                    *slot = poly;
                    saved
                };
                // Wildcard-`any` return-type fix: `view : Model -> any` makes
                // `any` appear in `free_vars` (the canon free-var collector treats
                // it uniformly alongside genuine type parameters).  But `any` is NOT
                // a real type parameter — it is a return-type wildcard whose concrete
                // type is resolved by the HM solver from the body.  When
                // `split_typed_sig` returns `IrType::Generic(any_sym)` for the
                // return position, substitute the body's solved concrete type from
                // `self.types.regions` instead.  Without this substitution the
                // emitted Rust function gains a spurious `<T1: Clone>` generic
                // parameter and a `-> T1` return type that the body cannot satisfy
                // (E0308).
                //
                // This block runs AFTER the poly_tvars installation above so that
                // `ir_type_from_ty(body_ty)` — specifically its
                // `ir_type_from_ty_ui_msg` sub-call for UI msg slots — already
                // sees the Bug-29-injected mapping and produces
                // `IrType::Generic(any_sym)` instead of `IrType::Unit`.
                //
                // Why regions, not env? `SolvedTypes::env` for TYPED bindings
                // stores the annotation type verbatim (`generated.top_level`), which
                // still has `Ty::Var(any_sym)` in the return position — the any UV
                // was never zonked back there.  `self.types.regions[(home, body.span)]`
                // is the body expression's solved type, which IS the concrete return
                // type after solving.
                //
                // The analogous gate in the Haskell compiler: `Instantiate.fromAnnotation`
                // filters `"any"` out before treating free vars as polymorphic;
                // `buildEnv` gives each `any` occurrence a fresh flex UV that the body
                // constrains to a concrete type.  The Rust port must do the same.
                let ret = if let IrType::Generic(sym) = ret {
                    if self.interner.resolve(sym) == Some("any") {
                        // The body's region type is the concrete return type.
                        let body_ty = self
                            .types
                            .regions
                            .get(&(def.home().to_vec(), body.span))
                            .ok_or_else(|| {
                                bug(
                                    "sky_lower::lower_def",
                                    "no region type for body of `any`-annotated binding",
                                )
                            })?;
                        self.ir_type_from_ty(body_ty, sig_span)?
                    } else {
                        IrType::Generic(sym)
                    }
                } else {
                    ret
                };
                // A tuple-destructuring parameter binds its synthetic name to the
                // tuple, then the body opens it with a `Destructure`. Fold the
                // prologue OUTERMOST-first (reverse) so the first parameter's
                // destructure is the outermost binding, matching source order.
                // Save/set/restore fn_is_async so nested lambdas/defs see the
                // correct async context for their own scope.
                let prev_async = self.fn_is_async.get();
                self.fn_is_async.set(matches!(ret, IrType::Task(_)));
                let mut lowered_body = self.lower_expr(body)?;
                *self.current_poly_tvars.borrow_mut() = saved_poly_tvars;
                self.fn_is_async.set(prev_async);
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
                //
                // Bug-28 fix (`init : any -> (Model, Cmd Msg)`): `any` in PARAM
                // position is a legitimate type parameter — `IrType::Generic(any_sym)`
                // appears in `params` and must be in `type_params` so the backend
                // can map it to a Rust generic `T{n}`.  The old filter
                // (`resolve(v) != "any"`) was correct for RETURN-position `any`
                // (resolved away above) but over-removed `any_sym` when `any` is
                // structurally used in `params`.
                //
                // Principled rule: include `v` in `type_params` iff
                // `IrType::Generic(v)` structurally appears in the RESOLVED
                // `params` or `ret`.  This naturally:
                //   - INCLUDES `any_sym` when `any` is in param position (Generic stays).
                //   - EXCLUDES `any_sym` when `any` is in return position (resolved away).
                //   - INCLUDES `any_sym` when `any` is the injected UI msg generic
                //     (`view : Model -> any`, Bug-29) because `ret = Html<Generic(any_sym)>`.
                let used_generics: BTreeSet<Symbol> = {
                    let mut s = BTreeSet::new();
                    for (_, ty) in &params {
                        collect_ir_generic_syms(ty, &mut s);
                    }
                    collect_ir_generic_syms(&ret, &mut s);
                    s
                };
                // (AUD-05) keyed by (home, name) — see the `bounds` field doc
                // on `SolvedTypes` for why a bare-name lookup is unsound here.
                let var_bounds = self.types.bounds.get(&(def.home().to_vec(), name));
                // AUD-01 seal fix: `any_syms_minted` holds every fresh symbol
                // `split_typed_sig` handed out for a per-occurrence `any`
                // param-position substitution — these are, by construction,
                // NOT in `free_vars` (canon never saw them; they're minted at
                // lowering time) but DO structurally appear in `params`
                // (`used_generics` already contains them). Without this union
                // each would silently drop out of `type_params` while still
                // being referenced in the emitted signature — an undeclared
                // Rust generic, worse than the bug this fix closes. Each is
                // trivially unbounded (`bounds_for` returns `UNBOUNDED` on a
                // missing `var_bounds` entry, which every fresh symbol has).
                let type_params = free_vars
                    .iter()
                    .copied()
                    .filter(|v| used_generics.contains(v))
                    .chain(any_syms_minted.iter().copied())
                    .map(|v| (v, Self::bounds_for(var_bounds, v)))
                    .collect();
                // T5 (#104 / #112): multi-use-clone rewrite for CloneOk params.
                // When a function parameter of `CloneOk` type (e.g. String) is used
                // N > 1 times in the body, all but the syntactically last occurrence
                // must clone — otherwise Rust emits E0382 (use of moved value).
                // Run BEFORE TCO so the loop-rewrite sees already-correct clone nodes.
                for (sym, ir_ty) in &params {
                    if matches!(clone_class(ir_ty), CloneClass::CloneOk) {
                        let n = count_var_uses(*sym, &lowered_body);
                        if n > 1 {
                            let mut remaining = n;
                            lowered_body =
                                rewrite_multiuse_clones(*sym, &mut remaining, lowered_body);
                        }
                    } else {
                        // T4 (#90): a fn-carrying, non-Clone param has no sound
                        // multi-use rewrite — fail closed on reuse instead.
                        reject_fn_value_reuse(*sym, ir_ty, &lowered_body, sig_span)?;
                    }
                }
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
                // Read the HM-solved type once; both the parameterless and the
                // parameterised paths need it.
                let solved_ty = self
                    .types
                    .env
                    .get(&(def.home().to_vec(), name))
                    .ok_or_else(|| {
                        bug("sky_lower::lower_def", "no inferred type for unannotated fn")
                    })?;
                // Boundary Scheme Promotion: if this def generalized at its
                // home module's boundary (a non-empty `untyped_type_params`
                // entry), install its quantified-var map exactly like the
                // Typed arm does — so a `Ty::Var` reachable from `solved_ty`
                // (region-zonked, hence solver-tagged) that IS one of these
                // quantified vars lowers to `IrType::Generic(sym)` instead of
                // hitting `ir_type_from_ty`'s SKY-L0102 fail-closed arm.
                // Absent/empty entry (the common case — most untyped defs stay
                // fully monomorphic): `current_poly_tvars` stays empty, byte-
                // identical to before this feature existed.
                let poly_key = (def.home().to_vec(), name);
                let quantified_syms = self.types.untyped_type_params.get(&poly_key);
                let is_generalized = quantified_syms.is_some_and(|v| !v.is_empty());
                let saved_poly_tvars = if is_generalized {
                    let poly = self.types.poly_var_map.get(&poly_key).cloned().unwrap_or_default();
                    let mut slot = self.current_poly_tvars.borrow_mut();
                    let saved = slot.clone();
                    *slot = poly;
                    Some(saved)
                } else {
                    None
                };
                let var_bounds = self.types.bounds.get(&poly_key);
                // `used_generics` structural-appearance filter, ported from the
                // Typed arm (Bug-28/Bug-29 invariant): a var only belongs in
                // `type_params` if `IrType::Generic(v)` actually appears in the
                // RESOLVED `params`/`ret` after `split_unannotated_sig` runs.
                // `quantified_syms` alone is not enough — a var can be a
                // residual boundary-scheme root that gets pinned to a concrete
                // type by `resolve_deferred` (e.g. a field-access result var
                // narrowly missed by `obligation_roots`) without disappearing
                // from `untyped_type_params`. Declaring it as a Rust generic
                // that appears in neither `params` nor `ret` is exactly the
                // E0283 SEAL violation an independent review caught on a
                // 3-module cross-module field-access getter; this filter is
                // defense-in-depth alongside the `obligation_roots` fix above.
                // Computed per-branch below, once `params`/`ret` are known.
                let compute_type_params = |quantified_syms: Option<&Vec<Symbol>>,
                                           var_bounds: Option<&BTreeMap<Symbol, TyBounds>>,
                                           params: &[(Symbol, IrType)],
                                           ret: &IrType| {
                    let used_generics: BTreeSet<Symbol> = {
                        let mut s = BTreeSet::new();
                        for (_, ty) in params {
                            collect_ir_generic_syms(ty, &mut s);
                        }
                        collect_ir_generic_syms(ret, &mut s);
                        s
                    };
                    quantified_syms
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|v| used_generics.contains(v))
                        .map(|v| (v, Self::bounds_for(var_bounds, v)))
                        .collect::<Vec<(Symbol, BoundSet)>>()
                };

                if !patterns.is_empty() {
                    // An unannotated top-level function: synthesise the typed
                    // parameter/return split from the HM-solved type, mirroring
                    // what `split_typed_sig` does for annotated bindings.
                    // Concrete solved types lower cleanly; a free `Ty::Var` in
                    // a parameter or return position that is NOT one of this
                    // def's own quantified vars surfaces as
                    // `Feature::Polymorphism` (SKY-L0102) rather than emitting
                    // unsound `any`-shaped parameters — fail-closed by design
                    // (Divergence D1: an ambiguous instantiation the reference
                    // erasure-accepts is rejected here, strictly safer).
                    let split_result = self.split_unannotated_sig(solved_ty, patterns, sig_span);
                    let (params, prologue, ret) = match split_result {
                        Ok(v) => v,
                        Err(e) => {
                            if let Some(saved) = saved_poly_tvars {
                                *self.current_poly_tvars.borrow_mut() = saved;
                            }
                            return Err(e);
                        }
                    };
                    // Save/set/restore fn_is_async (same rationale as Typed path).
                    let prev_async = self.fn_is_async.get();
                    self.fn_is_async.set(matches!(ret, IrType::Task(_)));
                    let body_result = self.lower_expr(body);
                    self.fn_is_async.set(prev_async);
                    if let Some(saved) = saved_poly_tvars {
                        *self.current_poly_tvars.borrow_mut() = saved;
                    }
                    let mut lowered_body = body_result?;
                    // Fold destructuring prologues outermost-first (reverse)
                    // so the first parameter's destructure is the outer binding.
                    for (binder_sym, binder_pat) in prologue.into_iter().rev() {
                        lowered_body = Expr::Destructure {
                            binder: binder_pat,
                            value: Box::new(Expr::Var(binder_sym)),
                            body: Box::new(lowered_body),
                        };
                    }
                    // T5 (#104 / #112): same param multi-use-clone pass as the
                    // Typed path above (see comment there for rationale).
                    for (sym, ir_ty) in &params {
                        if matches!(clone_class(ir_ty), CloneClass::CloneOk) {
                            let n = count_var_uses(*sym, &lowered_body);
                            if n > 1 {
                                let mut remaining = n;
                                lowered_body =
                                    rewrite_multiuse_clones(*sym, &mut remaining, lowered_body);
                            }
                        } else {
                            // T4 (#90): see the Typed-path comment above.
                            reject_fn_value_reuse(*sym, ir_ty, &lowered_body, sig_span)?;
                        }
                    }
                    let arity = params.len();
                    if analyze_tail_recursion(id, arity, &lowered_body)
                        == TailRecursion::TailRecursive
                    {
                        lowered_body = rewrite_tail_calls(id, arity, params.clone(), lowered_body);
                    }
                    let type_params =
                        compute_type_params(quantified_syms, var_bounds, &params, &ret);
                    return Ok(Func {
                        id,
                        name,
                        home: ModPath(def.home().to_vec()),
                        type_params,
                        params,
                        ret,
                        body: lowered_body,
                    });
                }
                let ret_result = self.ir_type_from_ty(solved_ty, sig_span);
                let ret = match ret_result {
                    Ok(v) => v,
                    Err(e) => {
                        if let Some(saved) = saved_poly_tvars {
                            *self.current_poly_tvars.borrow_mut() = saved;
                        }
                        return Err(e);
                    }
                };
                // Save/set/restore fn_is_async for the 0-param (value-binding) path.
                let prev_async = self.fn_is_async.get();
                self.fn_is_async.set(matches!(ret, IrType::Task(_)));
                let lowered_body = self.lower_expr(body);
                self.fn_is_async.set(prev_async);
                if let Some(saved) = saved_poly_tvars {
                    *self.current_poly_tvars.borrow_mut() = saved;
                }
                let lowered_body = lowered_body?;
                // Zero-param generalized value bindings (no value restriction,
                // e.g. `empty = []` used at two element types cross-module)
                // take the identical `params: []` path the backend already
                // emits for zero-arg fn calls — no shared mutable cell, no
                // memoization to break.
                let type_params = compute_type_params(quantified_syms, var_bounds, &[], &ret);
                Ok(Func {
                    id,
                    name,
                    home: ModPath(def.home().to_vec()),
                    type_params,
                    params: Vec::new(),
                    ret,
                    body: lowered_body,
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

    /// Like [`split_typed_sig`] but operates on a SOLVED [`Ty`] instead of a
    /// parsed `canon::Type` annotation.  Used for unannotated top-level
    /// functions whose complete type the HM solver has inferred — it peels the
    /// `Ty::Fun` chain one step per parameter pattern and converts each layer
    /// with [`ir_type_from_ty`].
    ///
    /// A free [`Ty::Var`] in a parameter or return position surfaces as
    /// [`Feature::Polymorphism`] (SKY-L0102): the unannotated function is
    /// polymorphic, and the backend has no source-level name with which to
    /// emit a `<T>` generic.  Fail-closed — never emits unsound `any`-shaped
    /// parameters.
    ///
    /// An arity mismatch (fewer arrows than patterns) is an invariant
    /// violation — the type checker rejects this shape first (arity mismatch →
    /// T0001), so reaching it here is a compiler bug.
    fn split_unannotated_sig(
        &self,
        ty: &Ty,
        patterns: &[canon::Pattern],
        span: Span,
    ) -> DResult<(Vec<IrParam>, Vec<ParamPrologue>, IrType)> {
        let mut cur = ty;
        let mut params = Vec::with_capacity(patterns.len());
        let mut prologue = Vec::new();
        for pat in patterns {
            let Ty::Fun(arg, rest) = cur else {
                return Err(bug(
                    "sky_lower::split_unannotated_sig",
                    "solved type has fewer arrows than parameters",
                ));
            };
            let ir_ty = self.ir_type_from_ty(arg, span)?;
            let (param, maybe_prologue) = self.lower_param(pat, ir_ty)?;
            params.push(param);
            if let Some(p) = maybe_prologue {
                prologue.push(p);
            }
            cur = rest.as_ref();
        }
        Ok((params, prologue, self.ir_type_from_ty(cur, span)?))
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
    /// Is `ty` a whole-annotation reference to a transparent stdlib FUNCTION
    /// alias (`Handler` = `Request -> Task Error Response`, `Middleware` =
    /// `Handler -> Handler`)? Such an annotation is a nullary `Con` even though
    /// the aliased type has arrows, so a binding `f : Handler` with parameters
    /// must be split from its (already-unfolded) SOLVED type, not from the
    /// annotation. Non-function opaque aliases (`Session` / `Store` / `VNode`)
    /// are excluded — they never carry a value binding's parameters.
    fn annotation_is_function_alias(&self, ty: &canon::Type) -> bool {
        matches!(
            ty,
            canon::Type::Con { name, args, .. }
                if args.is_empty()
                    && matches!(
                        self.interner.resolve(*name),
                        Some("Handler" | "Middleware")
                    )
        )
    }

    fn split_typed_sig(
        &self,
        ty: &canon::Type,
        patterns: &[canon::Pattern],
        generics: &[Symbol],
    ) -> DResult<TypedSigParts> {
        let mut cur = ty;
        let mut params = Vec::with_capacity(patterns.len());
        let mut prologue = Vec::new();
        let mut any_syms_minted = Vec::new();
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
            let mut ir_ty = self.ir_type_from_canon(arg, generics)?;
            // Per-occurrence `any` seal fix (AUD-01): a bare param-position `any`
            // lowers to `IrType::Generic(any_sym)` above — the SAME interned
            // Symbol for EVERY occurrence, so `f : any -> any -> Int` emits
            // `fn f<T1>(a:T1,b:T1)`, and a well-typed call `f "x" 3` (the checker
            // gives each `any` occurrence its own fresh flex UV, so this program
            // IS well-typed) fails cargo E0308 (exit-0-then-cargo-fail).
            //
            // Fix: give THIS occurrence a distinct fresh symbol from the
            // `any_param_binders` pool (pre-sized by `count_any_param_sites`,
            // pre-interned in `sky_lower::lower` alongside `param_binders` — the
            // interner is frozen by this point in the pipeline, so a symbol
            // cannot be minted here). The backend renders `IrType::Generic` by
            // the variable's POSITION in `Func::type_params`, not by the
            // symbol's spelling (see the `Generic` doc comment in
            // `sky_ir::ir`), so a fresh, distinctly-named symbol per occurrence
            // is sufficient — no concrete-type resolution needed, and each
            // occurrence behaves exactly like genuine independent polymorphism
            // (which is precisely what the checker's fresh-UV-per-occurrence
            // semantics already grants it).
            if let IrType::Generic(sym) = ir_ty
                && self.interner.resolve(sym) == Some("any")
            {
                let fresh = self.fresh_any_param_symbol()?;
                any_syms_minted.push(fresh);
                ir_ty = IrType::Generic(fresh);
            }
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
        Ok((
            params,
            prologue,
            self.ir_type_from_canon(cur, generics)?,
            any_syms_minted,
        ))
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
                let ty = self.region_ty(param_span).ok_or_else(|| {
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
            _ => self.lower_destructure_pat(pat),
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
                // `Order` is the built-in three-way comparison result type (#123).
                // Backed by `sky_runtime::SkyOrder` (repr(u8) enum: LT/EQ/GT).
                "Order" => Ok(IrType::Order),
                // `Decimal` is the Std.Decimal arbitrary-precision type.
                // Backed by `sky_runtime::decimal::Decimal` (rust_decimal newtype).
                "Decimal" => Ok(IrType::Decimal),
                // `SqlFragment` is `Std.Db.Sql`'s opaque WHERE-fragment type
                // (backlog #61). Backed by `sky_runtime::db::SqlFragment`.
                "SqlFragment" => Ok(IrType::SqlFragment),
                // `Secret` is `Sky.Core.Secret`'s opaque sealed secret-string
                // type (backlog #44). Backed by `sky_runtime::secret::Secret`.
                "Secret" => Ok(IrType::Secret),
                "String" => Ok(IrType::Str),
                // `Error` is Sky's fixed error-channel type — backed by the real
                // `sky_runtime::error::SkyError` ADT (backlog #85/#160), no
                // longer merged with `String`. `ErrorKind` mirrors `Order`.
                "Error" => Ok(IrType::Error),
                "ErrorKind" => Ok(IrType::ErrorKind),
                // `ErrorDetails` — the 5-variant enrichment union carried on
                // `ErrorInfo.details : Maybe ErrorDetails` (backlog #85
                // follow-up). Backed by `sky_runtime::error::SkyErrorDetails`.
                "ErrorDetails" => Ok(IrType::ErrorDetails),
                // The NOMINAL error-payload types (SEAL fix 2026-07-11) —
                // annotatable via canon's `EXTRA_BUILTIN_TYPE_NAMES`. Backed
                // by `sky_runtime::error::{SkyErrorInfo, SkyPanicInfo,
                // SkyTypeInfo}`.
                "ErrorInfo" => Ok(IrType::ErrorInfo),
                "PanicInfo" => Ok(IrType::PanicInfo),
                "TypeInfo" => Ok(IrType::TypeInfo),
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
                // M6 opaque server types — users may annotate handlers with
                // `Request -> Task Error Response` (via `exposing (Request, Response)`)
                // or route lists with `List Route`.  Mirrors `ir_type_from_ty`.
                "Request" => Ok(IrType::ServerRequest),
                "Response" => Ok(IrType::ServerResponse),
                "Route" => Ok(IrType::ServerRoute),
                "Cookie" => Ok(IrType::ServerCookie),
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
                // `Attribute msg` — a Std.Ui / Std.Html attribute.  Mirrors the
                // `ir_type_from_ty` "Attribute" arm: `Attribute` exists in BOTH
                // `Std.Ui` and `Std.Html`, disambiguated by the `home` path
                // (a path containing "Html" selects `HtmlAttribute`; everything
                // else — `["Std","Ui"]`, `["Ui"]`, or empty for the
                // builtin-injected form used by compiled-source stdlib modules
                // like `Std.Ui.Grid` / `Std.Ui.Transition` — selects
                // `UiAttribute`).  Without this arm an annotation such as
                // `columns : List Track -> Attribute msg` reaches the `other =>`
                // ICE with an empty home (SKY-I0001).
                "Attribute" if args.len() == 1 => {
                    let msg = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Attribute applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    let is_html = home.iter().any(|s| self.resolve(*s).ok() == Some("Html"));
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
                // `Event msg` — a Std.Ui / Std.Html event handler carrier.
                // Mirrors the `ir_type_from_ty` "Event" arm; same empty-home
                // gap as `Attribute` for compiled-source stdlib annotations.
                "Event" if args.len() == 1 => {
                    let msg = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Event applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::HtmlEvent,
                        msg: Box::new(msg),
                    })
                }
                // Sky.Live opaque types in annotations (mirrors `ir_type_from_ty`).
                "LiveReq" => Ok(IrType::LiveReq),
                // `StreamId` / `ChunkEvent` — builtin-registered Http.Stream ADTs
                // (no synthetic EnumDef injection, so not in enum_variants).
                // Mirrors the `ir_type_from_ty` arms added for these types.
                "StreamId" | "ChunkEvent" => Ok(IrType::Enum {
                    home: ModPath(Vec::new()),
                    name: *name,
                    args: Vec::new(),
                }),
                // `StreamWriter` — opaque server-side stream writer handle (#111).
                // Mirrors `ir_type_from_ty`'s "StreamWriter" arm.
                "StreamWriter" => Ok(IrType::StreamWriter),
                // `HttpRequest` — opaque HTTP request descriptor (#111).
                // Sky users write this as a structural record literal, but the
                // annotation `HttpRequest` maps directly to the runtime type via
                // this opaque variant.
                "HttpRequest" => Ok(IrType::HttpRequest),
                // #127: `WebSocketServer` — opaque per-peer WsHandle.
                "WebSocketServer" => Ok(IrType::WebSocketServer),
                // #127: `WebSocketServerCfg` — opaque WsServerCfg<SkyError>.
                "WebSocketServerCfg" => Ok(IrType::WebSocketServerCfg),
                // `LiveRoute page` is parametric on the page type it builds
                // (#108 round 4) — a bare `LiveRoute` annotation cannot
                // type-check (the solver's `LiveRoute` Con carries exactly one
                // argument, so a 0-arg annotation is a Con-arity SKY-T0001
                // before lowering); a miss here is an invariant violation.
                "LiveRoute" if args.len() == 1 => {
                    let page = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "LiveRoute applied without its page type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::LiveRoute(Box::new(page)))
                }
                // ── Std.Ui.Input parametric types (#124) ──────────────────────────
                "Label" if args.len() == 1 => {
                    let msg = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Label applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Ui { ctor: UiCtor::Label, msg: Box::new(msg) })
                }
                "Placeholder" if args.len() == 1 => {
                    let msg = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "Placeholder applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Ui { ctor: UiCtor::Placeholder, msg: Box::new(msg) })
                }
                "RadioOption" if args.len() == 1 => {
                    let msg = self.ir_type_from_canon(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_canon",
                                "RadioOption applied without its message type",
                            )
                        })?,
                        generics,
                    )?;
                    Ok(IrType::Ui { ctor: UiCtor::RadioOption, msg: Box::new(msg) })
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
                // The opaque JSON value type (`Value = any` in Sky). Placed AFTER
                // the `enum_variants` guard so a user-declared `type Value = …`
                // still resolves as its own enum (name-shadowing is allowed for
                // non-reserved names). Mirrors the `ir_type_from_ty` arm at the
                // `"Value"` case — both paths must map to `IrType::Json` for
                // consistency. Added by #138 to support bare `Value` annotations
                // on user functions (kernel-implicit Prelude type, #576).
                // `Claims` (D-00) maps to the same opaque JSON accumulator.
                "Value" | "Claims" => Ok(IrType::Json),
                // ── Kernel-implicit opaque server / Sky.Live types (#152) ────────
                // These names are registered in `KERNEL_IMPLICIT_PRELUDE_TYPE_NAMES`
                // in sky_canon so they pass N0002 without an explicit import.
                // They all carry zero type arguments at the annotation level.
                // `Handler` / `Middleware` — Sky.Http.Server function aliases.
                // `Session` / `Store` — Sky.Live session-management opaques.
                // `VNode` — Sky.Live virtual-DOM node.
                // All map to `IrType::Json` (universal opaque serde_json::Value) so
                // they can flow through the runtime without a dedicated Rust struct.
                "Handler" | "Middleware" | "Session" | "Store" | "VNode" => {
                    Ok(IrType::Json)
                }
                // ── JWT builder types (#152 / D-00) ─────────────────────────────
                // `Algorithm` — JWT signing algorithm descriptor encoded as a
                // `String` ("HS256:<secret>" or "RS256:<pem>").
                "Algorithm" => Ok(IrType::Str),
                // ── M7: Nullary Std.Ui plain types (no message parameter) ─────
                // Mirror of the `ir_type_from_ty` arms below.  Reached when a
                // type annotation writes `Color`, `Length`, etc. at a position
                // where canon emits a `Con { home: [], name, args: [] }` node —
                // i.e. the genuine opaque Std.Ui builtin, not a user-defined
                // enum (the `enum_variants` guard above fires first for those).
                "Length" => Ok(IrType::UiPlain(UiPlain::Length)),
                "Color" => Ok(IrType::UiPlain(UiPlain::Color)),
                "HAlign" => Ok(IrType::UiPlain(UiPlain::HAlign)),
                "VAlign" => Ok(IrType::UiPlain(UiPlain::VAlign)),
                "Location" => Ok(IrType::UiPlain(UiPlain::Location)),
                "PseudoClass" => Ok(IrType::UiPlain(UiPlain::PseudoClass)),
                "Description" => Ok(IrType::UiPlain(UiPlain::Description)),
                "LayoutContext" => Ok(IrType::UiPlain(UiPlain::LayoutContext)),
                other => {
                    // Every type reaching here has `home = []` but is NOT a
                    // known builtin.  `sky_canon::canonicalise_type` now emits
                    // SKY-N0002 (`TypeNotFound`) for any unknown unqualified type
                    // at compile time, so this arm is an invariant-violation ICE —
                    // it can no longer be triggered by valid user code.
                    Err(bug(
                        "sky_lower::ir_type_from_canon",
                        format!(
                            "type constructor `{other}` with empty home reached the \
                             lowerer — should have been caught by canon TypeNotFound \
                             (SKY-N0002)"
                        ),
                    ))
                }
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
            //
            // Exception: `any` wildcard in a union-ctor field (e.g.
            // `| CartTopicReceived any`) is the pub/sub wire carrier, pinned to
            // `Dict String String` by the solver (constrain.rs `pin_any_in_ty`).
            // The gate-1 check in `lower_enum` already skips these vars; here we
            // emit the matching concrete IR type so the emitted Rust is a
            // `HashMap<String, String>` field — no free generic, no `dyn Any`.
            canon::Type::Var(v) => {
                if self.interner.resolve(*v).is_some_and(|n| n == "any") && !generics.contains(v) {
                    return Ok(IrType::Dict(Box::new(IrType::Str), Box::new(IrType::Str)));
                }
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

    /// Collect the captured local variables for a closure body (T3, #121).
    ///
    /// Walks `canon_body` collecting every `VarLocal` free relative to
    /// `lambda_param_pats` (all flattened param patterns of the enclosing
    /// closure). For each captured symbol, looks up its use-site region type
    /// via [`Self::ir_type_from_ty`] to classify it by [`clone_class`].
    /// Returns `(symbol, ir_type_option)` pairs; `None` means the region type
    /// was unavailable (treated as bare / copy by the caller — safe default,
    /// no `CloneVar` inserted, no SKY-L0125).
    fn captured_locals(
        &self,
        lambda_param_pats: &[&canon::Pattern],
        canon_body: &canon::Expr,
    ) -> Vec<(Symbol, Option<IrType>)> {
        let mut outer_bound = BTreeSet::new();
        for &p in lambda_param_pats {
            canon_collect_pat_binds(p, &mut outer_bound);
        }
        let mut free: BTreeMap<Symbol, Span> = BTreeMap::new();
        canon_collect_free_locals(&mut free, &outer_bound, canon_body);
        free.into_iter()
            .map(|(sym, span)| {
                let ty = self
                    .region_ty(span)
                    .and_then(|ty| self.ir_type_from_ty(ty, span).ok());
                (sym, ty)
            })
            .collect()
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
        let ty = self.region_ty(span).ok_or_else(|| {
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
        // T3 (#121): accumulate all flattened canon param patterns to build
        // the outer-bound set for the capture-clone free-variable walk.
        let mut all_param_pats: Vec<&canon::Pattern> = Vec::new();
        loop {
            all_param_pats.extend(cur_params.iter());
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
                // A wildcard `_` (`PAnything`) parameter is never read, so its
                // concrete Rust type is irrelevant to correctness.  When the
                // solver leaves the argument type as a free `Ty::Var` — the
                // common case for callbacks like `\_ -> Task.succeed x` after
                // `Task.fail` (which never produces a value) or `\_ -> NoOp`
                // after `System.exit` (which diverges) — map the free variable
                // to `IrType::Json` (`JsonVal` / `any`) instead of raising
                // `SKY-L0102`.  This matches the Haskell reference:
                // `Can.PAnything -> (GoIr.GoParam "_" "any", [])`.
                let ir_ty = if matches!(&pat.value, canon::Pattern_::PAnything) {
                    self.ir_type_from_ty_json(arg, pat.span)?
                } else {
                    self.ir_type_from_ty(arg, pat.span)?
                };
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
        // T8 (#151 c02): use the JSON-friendly variant for the lambda return
        // type.  When the lambda's return type is a compound type containing a
        // free `Ty::Var` (e.g. `Task a` inside a polymorphic function like
        // `wrap : String -> Task Error a -> Task Error a`), the strict
        // `ir_type_from_ty` fails with SKY-L0102 (Polymorphism).
        // `ir_type_from_ty_json` maps the free `Ty::Var` to `IrType::Json`
        // instead — a sound stand-in since the Rust type is unified by the
        // surrounding kernel call site.
        //
        // Note: lambdas whose PARAMETER types contain free `Ty::Var` still
        // fail at the parameter step (line 4132 above) before reaching here;
        // the json fallback only affects the return-type slot.
        let ret = self.ir_type_from_ty_json(cur, span)?;
        // Save/set/restore fn_is_async so `lower_let`'s PAnything arm chooses
        // TaskSeqSync vs TaskSeq based on THIS lambda's return type, not the
        // enclosing def's.
        let prev_async = self.fn_is_async.get();
        self.fn_is_async.set(matches!(ret, IrType::Task(_)));
        let mut body = self.lower_expr(cur_body)?;
        self.fn_is_async.set(prev_async);
        // T3 (#121): Capture-clone rewrite — classify free locals captured
        // by this closure and replace CloneOk reads with `.clone()`, emitting
        // SKY-L0125 for NonClone captures outside callee position.
        {
            let captures = self.captured_locals(&all_param_pats, cur_body);
            let mut clone_set: BTreeSet<Symbol> = BTreeSet::new();
            let mut noncl_set: BTreeSet<Symbol> = BTreeSet::new();
            for (sym, ir_ty) in captures {
                match ir_ty.as_ref().map(clone_class) {
                    Some(CloneClass::CloneOk) => {
                        clone_set.insert(sym);
                    }
                    Some(CloneClass::NonClone) => {
                        noncl_set.insert(sym);
                    }
                    Some(CloneClass::CopyLeaf) | None => {}
                }
            }
            body = rewrite_captured_clones(&clone_set, &noncl_set, span, body, 0)?;
        }
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
        // T4 (#90, revert-incident Bug 1): a fn-carrying, non-Clone LAMBDA
        // parameter has no sound multi-use rewrite — fail closed on reuse,
        // same as the Def-head / let-binding / match-arm gates above.  This
        // call site was the one the first #90 landing (f80f05a, reverted)
        // missed entirely: `lower_lambda` builds its own `ir_params` here but
        // never ran them through `reject_fn_value_reuse`, so
        // `\mf -> consume mf + consume mf` with `mf : Maybe (Int -> Int)`
        // reused the boxed closure twice and reached `cargo build` as
        // E0382 use-of-moved-value instead of a clean SKY-L0127 diagnostic.
        // `reject_fn_value_reuse` self-guards on non-fn-carrying / CloneOk
        // params, so it is safe to call unconditionally for every param
        // (including a `PAnything` wildcard's fresh synthetic binder, which
        // by construction is referenced zero times).
        for (sym, ir_ty) in &ir_params {
            reject_fn_value_reuse(*sym, ir_ty, &body, span)?;
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
        let ty = self.region_ty(span).ok_or_else(|| {
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
            _other => Err(bug(
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
                // `Order` is the built-in three-way comparison result type (#123).
                // Backed by `sky_runtime::SkyOrder` (repr(u8) enum: LT/EQ/GT).
                "Order" => Ok(IrType::Order),
                // `Decimal` is the Std.Decimal arbitrary-precision type.
                // Backed by `sky_runtime::decimal::Decimal` (rust_decimal newtype).
                "Decimal" => Ok(IrType::Decimal),
                // `SqlFragment` is `Std.Db.Sql`'s opaque WHERE-fragment type
                // (backlog #61). Backed by `sky_runtime::db::SqlFragment`.
                "SqlFragment" => Ok(IrType::SqlFragment),
                // `Secret` is `Sky.Core.Secret`'s opaque sealed secret-string
                // type (backlog #44). Backed by `sky_runtime::secret::Secret`.
                "Secret" => Ok(IrType::Secret),
                // `Algorithm` (D-00) shares the `String` IR representation.
                "String" | "Algorithm" => Ok(IrType::Str),
                // `Error` — backed by the real `sky_runtime::error::SkyError` ADT
                // (backlog #85/#160), no longer merged with `String`. Lambda
                // parameters typed as `Error` (e.g. the `e` in `\e -> ...` when
                // `onError`/`mapError` pins the handler) lower here too.
                "Error" => Ok(IrType::Error),
                "ErrorKind" => Ok(IrType::ErrorKind),
                // `ErrorDetails` — mirrors the `ir_type_from_canon` arm added at
                // the same time (backlog #85 follow-up).
                "ErrorDetails" => Ok(IrType::ErrorDetails),
                // The NOMINAL error-payload types (SEAL fix 2026-07-11) —
                // backed by `sky_runtime::error::{SkyErrorInfo, SkyPanicInfo,
                // SkyTypeInfo}`. Pattern-bound payloads (`Error _ info ->` /
                // `FfiPanic p ->`) get these solved Cons, so unannotated
                // helpers over them lower to the runtime types and agree with
                // their call sites.
                "ErrorInfo" => Ok(IrType::ErrorInfo),
                "PanicInfo" => Ok(IrType::PanicInfo),
                "TypeInfo" => Ok(IrType::TypeInfo),
                "Char" => Ok(IrType::Char),
                // ── Kernel-implicit opaque server / Sky.Live types (#152) ────────
                // Mirror of the `ir_type_from_canon` arms added at the same
                // time: these are the HM-solved-type counterparts that fire when
                // the type is propagated via the region map rather than read from
                // a user annotation.
                // `Claims` (D-00) maps to the same opaque JSON accumulator.
                "Handler" | "Middleware" | "Session" | "Store" | "VNode" | "Claims" => {
                    Ok(IrType::Json)
                }
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
                // `StreamId` / `ChunkEvent` — builtin-registered Http.Stream ADTs.
                // Not compiled from source; constructors pre-registered in
                // `install_builtin_ctors`. All four map to `IrType::Enum` with an
                // empty home, matching the synthetic EnumDef and `Expr::Ctor` home.
                "SqlValue" | "SqlField" | "StreamId" | "ChunkEvent" => Ok(IrType::Enum {
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
                // `StreamWriter` — opaque stream writer handle (#111).
                "StreamWriter" => Ok(IrType::StreamWriter),
                // `HttpRequest` — opaque HTTP request descriptor (#111).
                // Mirrors `ir_type_from_canon`'s "HttpRequest" arm.
                "HttpRequest" => Ok(IrType::HttpRequest),
                // #127: `WebSocketServer` — opaque per-peer WsHandle.
                "WebSocketServer" => Ok(IrType::WebSocketServer),
                // #127: `WebSocketServerCfg` — opaque WsServerCfg<SkyError>.
                "WebSocketServerCfg" => Ok(IrType::WebSocketServerCfg),
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
                // ── Std.Ui.Input parametric types (#124) ───────────────────────
                // `Label msg` and `Placeholder msg` are kernel-reserved type
                // names produced by `constrain`'s `label_t` / `placeholder_t`
                // helper closures.  They are NOT user ADTs, so they precede the
                // `enum_variants` guard below.
                "Label" if args.len() == 1 => {
                    let msg = self.ir_type_from_ty_ui_msg(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Label applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::Label,
                        msg: Box::new(msg),
                    })
                }
                "Placeholder" if args.len() == 1 => {
                    let msg = self.ir_type_from_ty_ui_msg(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "Placeholder applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::Placeholder,
                        msg: Box::new(msg),
                    })
                }
                "RadioOption" if args.len() == 1 => {
                    let msg = self.ir_type_from_ty_ui_msg(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "RadioOption applied without its message type",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::Ui {
                        ctor: UiCtor::RadioOption,
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
                // `LiveRoute page` — the route descriptor produced by
                // `Live.route`, parametric on the page type it builds (#108).
                // The solver's `LiveRoute` Con always carries exactly one
                // argument (constrain's `live_route(page)` builder), so the
                // page type is threaded into the IR and rendered as
                // `Route<Page>` — the runtime `Route` struct has no default
                // type parameter, so dropping the argument is an E0107 cargo
                // failure in any rendered position (empty-vec turbofish /
                // fn signatures of let-bound route tables).
                "LiveRoute" if args.len() == 1 => {
                    let page = self.ir_type_from_ty(
                        args.first().ok_or_else(|| {
                            bug(
                                "sky_lower::ir_type_from_ty",
                                "LiveRoute Con without its page type argument",
                            )
                        })?,
                        span,
                    )?;
                    Ok(IrType::LiveRoute(Box::new(page)))
                }
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
                other => {
                    // Solver-side counterpart of `ir_type_from_canon`.  The HM
                    // solver propagates `module` from the canonical `home` set by
                    // `canonicalise_type`, so `module = []` on a non-builtin Con
                    // here means the annotation had an unknown type — which canon
                    // now rejects with SKY-N0002 before the solver runs.  This arm
                    // is therefore an invariant-violation ICE.
                    Err(bug(
                        "sky_lower::ir_type_from_ty",
                        format!(
                            "type constructor `{other}` with empty home reached the \
                             lowerer — should have been caught by canon TypeNotFound \
                             (SKY-N0002)"
                        ),
                    ))
                }
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
            // A record type (closed or open): lower each field type, keyed by
            // field name. The RowTail is a solver artefact — the IR type is
            // always a flat field map. Open fields not present in the resolved
            // `Ty` are simply absent from the IR (the optional-field mechanism
            // works through the open row var at type-check time, not at codegen).
            //
            // Special case: detect the canonical 7-field `HttpRequest` record —
            // `body`, `followRedirects`, `headers`, `maxRedirects`, `method`,
            // `timeout`, `url` — and fold it to the opaque `IrType::HttpRequest`
            // so call sites that pass the value to `http_stream_open` /
            // `http_request` kernels see the correct runtime type, rather than a
            // backend-synthesised struct with an auto-generated name.
            Ty::Record(fields, _tail) => {
                const HTTP_REQUEST_FIELDS: &[&str] = &[
                    "body",
                    "followRedirects",
                    "headers",
                    "maxRedirects",
                    "method",
                    "timeout",
                    "url",
                ];
                // `BTreeMap<Symbol, _>` iterates in Symbol-integer order (intern
                // assignment order), NOT in alphabetical order.  Collect names,
                // sort them, then compare to the alphabetically-sorted constant.
                if fields.len() == HTTP_REQUEST_FIELDS.len() {
                    let mut field_names: Vec<&str> = fields
                        .keys()
                        .filter_map(|sym| self.interner.resolve(*sym))
                        .collect();
                    field_names.sort_unstable();
                    let is_http_request = field_names
                        .iter()
                        .zip(HTTP_REQUEST_FIELDS.iter())
                        .all(|(a, b)| *a == *b);
                    if is_http_request {
                        return Ok(IrType::HttpRequest);
                    }
                }
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
            // A type variable in value position. If it's one of the enclosing
            // binding's own generic type parameters (`current_poly_tvars`,
            // installed by `lower_def` for a `Def::Typed` or a Boundary-
            // Scheme-Promoted `Def::Untyped` before it recurses into the
            // body), emit `IrType::Generic(sym)` — the backend produces
            // e.g. `Attribute<T1>` rather than failing closed.
            //
            // Otherwise: with M2a, a binding can be genuinely parametric, so
            // a region the solver left as a bare variable is an
            // under-determined polymorphic value the lowerer cannot
            // monomorphise here yet — e.g. a polymorphic function referenced
            // as a first-class value whose type never gets pinned to a
            // concrete instance at the use site. That is a real M2a feature
            // gap (the value's Rust type would itself have to be generic in a
            // position the backend does not yet model), not an invariant
            // violation, so it surfaces as a `Diagnostic::Lower` with the span
            // — never a `CompilerBug` for well-typed input.
            // [SKY-L0102, feature: polymorphism]
            Ty::Var(v) => {
                if let Some(&sym) = self.current_poly_tvars.borrow().get(v) {
                    Ok(IrType::Generic(sym))
                } else {
                    Err(unsupported(span, Feature::Polymorphism))
                }
            }
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
            // A type variable in msg position is either:
            //
            //   (a) an enclosing annotated function's generic type parameter —
            //       e.g. `parentMsg` in `view : (Msg -> parentMsg) -> Counter ->
            //       Html parentMsg`.  Region types inside the body carry this as
            //       `Ty::Var(uf_rep)` where `uf_rep` is the union-find
            //       representative of the rigid (skolem) created for `parentMsg`.
            //       → emit `IrType::Generic(sym)` so the backend produces
            //       `Attribute<T1>` rather than `Attribute<()>`, avoiding E0308.
            //
            //   (b) a truly unconstrained, message-free subtree — e.g. the element
            //       type of `[]` when the list's msg was never pinned to a concrete
            //       type.
            //       → emit `IrType::Unit` (Rust conventional `Html<()>`).
            //
            // `current_poly_tvars` (populated by `lower_def` for each `Def::Typed`
            // before it recurses into the body, and restored afterward) maps
            // uf_rep → annotation var symbol for the current enclosing function.
            // An empty map (unannotated or non-polymorphic context) always falls
            // through to `IrType::Unit`.
            Ty::Var(v) => {
                if let Some(&sym) = self.current_poly_tvars.borrow().get(v) {
                    Ok(IrType::Generic(sym))
                } else {
                    Ok(IrType::Unit)
                }
            }
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
    /// Also used for wildcard `_` (`PAnything`) lambda parameters: the
    /// parameter is never read, so any type that compiles is sound.  When the
    /// solver leaves the argument type as a free `Ty::Var` — or a compound
    /// type containing a free `Ty::Var` (e.g. `Result Error a` in a
    /// `Cmd.perform` callback) — this helper recurses into every compound
    /// type arm and maps each `Ty::Var` leaf to [`IrType::Json`] rather than
    /// failing with [`Feature::Polymorphism`] (SKY-L0102).
    // The match has one arm per compound builtin — each arm adds ~4 lines;
    // pushing past clippy's 100-line ceiling is unavoidable without splitting
    // on an arbitrary boundary.  The allow is narrow: only this function.
    #[allow(clippy::too_many_lines)]
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
            // Compound constructor types — recurse so that an embedded
            // `Ty::Var` in e.g. `Result Error a` maps to `IrType::Json`
            // rather than falling through to the strict `ir_type_from_ty`.
            // Scalar constructors (Int, Float, Bool, …) have no type args and
            // fall through to `ir_type_from_ty` unchanged.
            Ty::Con { name, args, .. } if !args.is_empty() => {
                match self.resolve(*name)? {
                    "Maybe" if args.len() == 1 => {
                        let elem = self.ir_type_from_ty_json(
                            args.first().ok_or_else(maybe_arg_bug)?,
                            span,
                        )?;
                        Ok(IrType::Maybe(Box::new(elem)))
                    }
                    "Result" if args.len() == 2 => {
                        // A free `Ty::Var` in the ERROR slot must pin to
                        // `IrType::Error` (`SkyError`), NOT the Json fallback:
                        // the emitted `ok_res` wrapper (`ResultOkDefault`, see
                        // the ctor-lowering arm) pins an unresolved error type
                        // to the project's `SkyError` — "the canonical
                        // default" — so a type ANNOTATION derived from the
                        // same free var (e.g. an eta-param binder for a piped
                        // `Ok f |> Result.andMap …`) must agree, or the
                        // emitted `let eta_0: SkyResult<JsonVal, _> = ok_res(…)`
                        // is an E0308 exit-0-then-cargo-fail (found while
                        // gating #90's 5th attempt: the
                        // `l0114_result_and_map_fn_payload` positive-path
                        // fixture). One defaulting policy, both sides.
                        let err_ty = args.first().ok_or_else(result_arg_bug)?;
                        let err = if matches!(err_ty, Ty::Var(_)) {
                            IrType::Error
                        } else {
                            self.ir_type_from_ty_json(err_ty, span)?
                        };
                        let ok = self.ir_type_from_ty_json(
                            args.get(1).ok_or_else(result_arg_bug)?,
                            span,
                        )?;
                        Ok(IrType::Result(Box::new(err), Box::new(ok)))
                    }
                    "List" if args.len() == 1 => {
                        let elem = self.ir_type_from_ty_json(
                            args.first().ok_or_else(list_arg_bug)?,
                            span,
                        )?;
                        Ok(IrType::List(Box::new(elem)))
                    }
                    "Task" if args.len() == 1 => {
                        let inner = self.ir_type_from_ty_json(
                            args.first().ok_or_else(task_arg_bug)?,
                            span,
                        )?;
                        Ok(IrType::Task(Box::new(inner)))
                    }
                    "Cmd" if args.len() == 1 => {
                        let inner = self.ir_type_from_ty_json(
                            args.first().ok_or_else(|| {
                                bug(
                                    "sky_lower::ir_type_from_ty_json",
                                    "Cmd applied without its message type",
                                )
                            })?,
                            span,
                        )?;
                        Ok(IrType::Cmd(Box::new(inner)))
                    }
                    "Sub" if args.len() == 1 => {
                        let inner = self.ir_type_from_ty_json(
                            args.first().ok_or_else(|| {
                                bug(
                                    "sky_lower::ir_type_from_ty_json",
                                    "Sub applied without its message type",
                                )
                            })?,
                            span,
                        )?;
                        Ok(IrType::Sub(Box::new(inner)))
                    }
                    "Set" if args.len() == 1 => {
                        let elem = self.ir_type_from_ty_json(
                            args.first().ok_or_else(set_arg_bug)?,
                            span,
                        )?;
                        if matches!(elem, IrType::Float) {
                            return Err(unsupported(span, Feature::FloatKeyedCollection));
                        }
                        Ok(IrType::Set(Box::new(elem)))
                    }
                    "Dict" if args.len() == 2 => {
                        let k = self.ir_type_from_ty_json(
                            args.first().ok_or_else(dict_arg_bug)?,
                            span,
                        )?;
                        let v = self.ir_type_from_ty_json(
                            args.get(1).ok_or_else(dict_arg_bug)?,
                            span,
                        )?;
                        if matches!(k, IrType::Float) {
                            return Err(unsupported(span, Feature::FloatKeyedCollection));
                        }
                        Ok(IrType::Dict(Box::new(k), Box::new(v)))
                    }
                    "Decoder" if args.len() == 1 => {
                        let inner = self.ir_type_from_ty_json(
                            args.first().ok_or_else(|| {
                                bug(
                                    "sky_lower::ir_type_from_ty_json",
                                    "Decoder applied without its element type",
                                )
                            })?,
                            span,
                        )?;
                        Ok(IrType::Decoder(Box::new(inner)))
                    }
                    // All other compound constructors (user enum types with
                    // type args, opaque types, etc.) delegate to the strict
                    // helper — only the known builtins above can embed a free
                    // type variable in a `PAnything` / JSON callback position.
                    _ => self.ir_type_from_ty(t, span),
                }
            }
            // For all other type forms (scalar constructors with no args,
            // opaque types, etc.), delegate to the strict helper.
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
        if let Some(Ty::Fun(_, _)) = self.region_ty(value.span) {
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
        ) && let Some(ty) = self.region_ty(e.span)
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
                    // Bare payload constructor used as a first-class function value
                    // (e.g. `onInput InputAlertMetric`, `List.map Just`).  Elm / Sky
                    // treat constructors as ordinary functions; here we eta-expand:
                    //   `Ctor` (arity N)  →  `|p0, …, p{N-1}| Ctor(p0, …, p{N-1})`
                    //
                    // Retrieve the inferred function type for this expression —
                    // the region type at `e.span` should be `T0 -> … -> Tn -> R`.
                    let fn_ty = self.region_ty(e.span).ok_or_else(|| {
                        bug(
                            "sky_lower::lower_expr/VarCtor",
                            "no region type for a bare payload-constructor reference",
                        )
                    })?;
                    // Peel `arity` arrows to collect param types and the return type.
                    let mut cur = fn_ty;
                    let mut arg_tys: Vec<&Ty> = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        let Ty::Fun(arg, rest) = cur else {
                            return Err(bug(
                                "sky_lower::lower_expr/VarCtor",
                                "bare-ctor region type has fewer arrows than declared arity",
                            ));
                        };
                        arg_tys.push(arg);
                        cur = rest.as_ref();
                    }
                    let ret_ty = cur;
                    // Mint fresh parameter symbols from the eta pool and build the lambda.
                    let mut params: Vec<(Symbol, IrType)> = Vec::with_capacity(arity);
                    let mut ctor_args: Vec<Expr> = Vec::with_capacity(arity);
                    for (i, arg_ty) in arg_tys.iter().enumerate() {
                        let sym = *self.eta_params.get(i).ok_or_else(|| {
                            bug(
                                "sky_lower::lower_expr/VarCtor",
                                "eta-parameter pool smaller than constructor arity",
                            )
                        })?;
                        let ir = self.ir_type_from_ty(arg_ty, e.span)?;
                        params.push((sym, ir));
                        ctor_args.push(Expr::Var(sym));
                    }
                    let ret = self.ir_type_from_ty(ret_ty, e.span)?;
                    let body = Expr::Ctor {
                        home: ctor_home,
                        ty: *type_name,
                        variant: *name,
                        args: ctor_args,
                    };
                    Ok(Expr::Lambda {
                        params,
                        ret,
                        body: Box::new(body),
                    })
                }
            }
            canon::Expr_::Binop { func, lhs, rhs, .. } => {
                // `++` is `Appendable a => a -> a -> a`. The type checker has
                // already pinned the result type to `String` or `List _`;
                // dispatch to the appropriate backend here rather than routing
                // through `binop()` so the single function stays
                // String-specific.
                if self.resolve(*func)? == "append" {
                    let lowered_lhs = self.lower_expr(lhs)?;
                    let lowered_rhs = self.lower_expr(rhs)?;
                    let is_list = self.region_ty(e.span).is_some_and(|ty| {
                        matches!(ty, Ty::Con { name, args, .. }
                            if args.len() == 1
                                && self.interner.resolve(*name) == Some("List"))
                    });
                    if is_list {
                        Ok(Expr::Call {
                            callee: Callee::Kernel(KernelFn::ListAppend),
                            args: vec![lowered_lhs, lowered_rhs],
                        })
                    } else {
                        Ok(Expr::BinOp {
                            op: BinOp::Append,
                            lhs: Box::new(lowered_lhs),
                            rhs: Box::new(lowered_rhs),
                        })
                    }
                } else {
                    Ok(Expr::BinOp {
                        op: self.binop(*func, e.span)?,
                        lhs: Box::new(self.lower_expr(lhs)?),
                        rhs: Box::new(self.lower_expr(rhs)?),
                    })
                }
            }
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
                    ) && let Some(ty) = self.region_ty(e.span)
                    {
                        // Return value discarded — only the error path matters.
                        let _ = self.ir_type_from_ty(ty, e.span)?;
                    }
                    return Ok(Expr::Call {
                        callee,
                        args: Vec::new(),
                    });
                }
                let ty = self.region_ty(e.span).ok_or_else(|| {
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
                // T6 (#121): arity-exact invariant — when def-arity is less
                // than the flattened `Fun` param count at this reify site, the
                // callee is curried and cannot be boxed directly into
                // `Box<dyn Fn(T0,T1,…)->R>` (Rust would reject it with E0593).
                // Emit an eta-adapter Lambda that expands the closure to the
                // expected full arity: `|eta_0,…,eta_{N-1}| (f(eta_0,…,eta_{k-1}))(eta_k,…)`.
                // A def-arity GREATER than the flattened count is a compiler bug
                // (the solver should have caught it), surfaced here as a bug()
                // rather than emitting broken Rust.
                // Kernels are arity-exact by construction; the adapter never fires
                // for them (their `callee_arity` matches their native signature).
                if let IrType::Fun(params, ret) = &ty_ir {
                    let def_arity = self.callee_arity(&callee)?;
                    if def_arity < params.len() {
                        return self.eta_adapt_funcvalue(callee, params, ret, e.span);
                    }
                    if def_arity > params.len() {
                        return Err(bug(
                            "sky_lower::lower_expr",
                            "def-arity exceeds the reference's flattened arity — the type-checker should have caught this",
                        ));
                    }
                    // def_arity == params.len(): arity-exact, emit FuncValue as before.
                }
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
        if let Some(base_ty) = self.region_ty(base.span)
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
        let Some(Ty::Con { name, args, .. }) = self.region_ty(span) else {
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

    /// #90 T3 (Tier 1 backstop — see [`Self::lower_callee`]'s doc comment):
    /// `Maybe.andMap` / `Result.andMap` resolved to a CURRIED (arity ≥ 2)
    /// payload function.
    ///
    /// `andMap : Maybe (a -> b) -> Maybe a -> Maybe b` (`Result e (a -> b) ->
    /// Result e a -> Result e b`) is arity-1 per application: it fully
    /// applies the wrapped function to exactly one argument. When the
    /// wrapped function is itself curried (`\a b -> …`, IR-flattened to one
    /// multi-parameter `Fun`), `a` instantiates to the first parameter and
    /// `b` to the REMAINING curried tail — itself a `Ty::Fun`. This
    /// reference's own solved type then has `Maybe b` / `Result e b` as its
    /// tail with `b` a function: the applicative chain has not reached a
    /// fully-applied value, and finishing it needs a nested-closure
    /// (`curryN`-style) lowering this Stage does not implement (Stage 2,
    /// tracked separately — see
    /// `docs/architecture/ctor-payload-function-design.md` §3). Fail closed
    /// here rather than let an unfinished chain reach a use site with no
    /// sound lowering.
    ///
    /// `andMap`'s OWN solved type at `callee`'s span
    /// (`self.region_ty(callee.span)`) is already the FULLY unified
    /// signature for every use, because HM solving is global across the
    /// whole binding: a `let`-bound partial application's LATER use still
    /// constrains the same type variables through the let-binding's own
    /// type, so `Result.andMap`'s reference type already reflects
    /// `b = Int -> Int -> Int` by the time lowering runs (solving completes
    /// before lowering starts). So this check does not need to look at any
    /// ARGUMENT EXPRESSIONS, nor at how this reference is being used — it
    /// peels `andMap`'s fixed arity (2) off ITS OWN reference type and
    /// inspects the trailing payload position of the result (`b` in
    /// `Maybe b` / `Result e b`) for a residual `Ty::Fun`, catching the
    /// curried-payload hazard under every syntactic spelling and every
    /// aliasing hop between the kernel reference and its eventual use.
    ///
    /// Only fires for the two `andMap` kernels; every other resolved callee
    /// is untouched (`Ok(())` fast path). Kept as defense-in-depth behind the
    /// primary Tier-2 type-checker obligation (see [`Self::lower_callee`]'s
    /// doc comment) — a bug in the Tier-2 wiring should not silently reopen
    /// this hazard.
    fn reject_curried_andmap_payload(&self, resolved: &Callee, callee: &canon::Expr) -> DResult<()> {
        if !matches!(
            resolved,
            Callee::Kernel(KernelFn::MaybeAndMap | KernelFn::ResultAndMap)
        ) {
            return Ok(());
        }
        // `andMap`'s own reference type: `Con a -> Con (a -> b) -> Con b`
        // (Maybe/Result-headed). Peel exactly its fixed arity (2 arrows) to
        // reach the final `Con b` return — independent of how many arguments
        // any particular AST node happens to supply at this reference.
        let Some(ty) = self.region_ty(callee.span) else {
            return Ok(());
        };
        let Ty::Fun(_, after_first_arrow) = ty else {
            return Ok(());
        };
        let Ty::Fun(_, call_ret) = after_first_arrow.as_ref() else {
            return Ok(());
        };
        // `call_ret` is `Maybe b` / `Result e b` — the payload position is
        // the LAST type argument of that `Con`. The curried signal is
        // whether `b` is ITSELF an arrow (arity ≥ 2 flattened into one
        // `IrType::Fun`, which `maybe_and_map`/`result_and_map`'s
        // `F: FnOnce(A) -> B` cannot represent when `B` is a function — no
        // `Box<dyn Fn(A0,A1)->R>` implements `FnOnce(A0) -> (A1 -> R)`).
        let Ty::Con { args: ret_args, .. } = call_ret.as_ref() else {
            return Ok(());
        };
        let Some(b) = ret_args.last() else {
            return Ok(());
        };
        if matches!(b, Ty::Fun(_, _)) {
            return Err(unsupported(callee.span, Feature::CtorPayloadFunction));
        }
        Ok(())
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

    /// Lower the page-builder argument of a `Live.route pattern builder` call
    /// (#108 round 4).
    ///
    /// A BARE payload constructor (`UserPage` with `UserPage : String -> Page`
    /// — the canonical `:param` route shape) lowers to a zero-arg
    /// [`Expr::Ctor`] carrier instead of tripping the general
    /// `Feature::CtorAsFunction` gate: in this one position the constructor is
    /// never a first-class function value — `emit_live_call::LiveRoute` folds
    /// it into the route's builder closure, applying one type-directed
    /// `params.get(i)` conversion per declared payload field.
    ///
    /// `True` / `False` are excluded (they lower to boolean literals, and a
    /// `Bool`-built route page is nonsense the type checker rejects anyway).
    /// Every other builder shape — nullary ctor, lambda, saturated call —
    /// takes the uniform [`Self::lower_expr`] path unchanged.
    fn lower_route_builder(&self, e: &canon::Expr) -> DResult<Expr> {
        if let canon::Expr_::VarCtor {
            home,
            type_name,
            name,
            ..
        } = &e.value
            && !matches!(self.resolve(*name)?, "True" | "False")
        {
            let ctor_home = ModPath(home.clone());
            if self.ctor_arity_of(&ctor_home, *name)? > 0 {
                return Ok(Expr::Ctor {
                    home: ctor_home,
                    ty: *type_name,
                    variant: *name,
                    args: vec![],
                });
            }
        }
        self.lower_expr(e)
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

        // App-entry / Live.route intercepts (Phase-1b/#108) — see the helper.
        match self.intercept_live_kernel_call(callee, args)? {
            Intercepted::Done(e) => Ok(e),
            Intercepted::Fallthrough(peeked) => {
                self.lower_call_uniform(callee, args, call_span, peeked)
            }
        }
    }

    /// Kernel-call intercepts that must run BEFORE the uniform arg lowering
    /// of [`Self::lower_call_uniform`] (Phase-1b + #108).
    ///
    /// Returns [`Intercepted::Done`] when the call was intercepted and fully
    /// lowered here; [`Intercepted::Fallthrough`] to continue on the uniform
    /// path. `lower_callee` is a pure symbol-table lookup (no side effects);
    /// a fall-through carries the already-resolved [`Callee`] so the uniform
    /// path doesn't re-run the large dispatch (efficiency-audit §3 medium —
    /// it used to be deliberately re-called, "safe but minimal-diff").
    fn intercept_live_kernel_call(
        &self,
        callee: &canon::Expr,
        args: &[canon::Expr],
    ) -> DResult<Intercepted> {
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
                        return Ok(Intercepted::Done(Expr::Call {
                            callee: peek,
                            args: vec![lowered_cfg],
                        }));
                    }
                }
                // ── Tui.app / Tui.program / Webview.app / Cli.program cfg literal
                //    (L0107 exemption) ──
                //
                // Same pattern as `Live.app`: intercept the single cfg-record arg
                // BEFORE the uniform `lower_expr` path so function-typed fields
                // (init/update/view/subscriptions/onKey) do not trip SKY-L0107.
                // Phase-1c: TuiApp / TuiProgram.
                // Phase-1d: WebviewApp — the extra `window` field is a plain record
                //   value (no functions); `lower_app_entry_cfg` additionally
                //   requires that record — and its `size` tuple — to be inline
                //   literals (the G4 emit gates).
                // #111: CliProgram — 5-field cfg (init/update/view/subscriptions/
                //   onLine), all function-typed; without this arm every real
                //   `Cli.program` call would trip SKY-L0107 and the emit_cli
                //   path could never fire.
                // A non-literal cfg (let-bound, piped, etc.) is rejected here with
                // SKY-L0119 at the argument span — fail-closed, never an ICE.
                Callee::Kernel(
                    KernelFn::TuiApp
                    | KernelFn::TuiProgram
                    | KernelFn::WebviewApp
                    | KernelFn::CliProgram,
                ) if args.len() == 1 =>
                {
                    if let Some(arg0) = args.first() {
                        // Borrow `peek` for the gate BEFORE moving it below.
                        let lowered_cfg = self.lower_app_entry_cfg(&peek, arg0)?;
                        return Ok(Intercepted::Done(Expr::Call {
                            callee: peek,
                            args: vec![lowered_cfg],
                        }));
                    }
                }
                // ── Input.text / email / username / search / currentPassword /
                //    newPassword / multiline / checkbox cfg literal
                //    (L0107 exemption, #124) ──
                //
                // Input kernels take TWO arguments: `(attrs : List Attr, cfg : Cfg)`.
                // The cfg record contains function-valued fields (e.g. `onChange :
                // String -> msg`), which the per-field `reject_function_valued_field`
                // gate would reject as SKY-L0107 if lowered through the uniform path.
                //
                // Fix: lower `args[0]` (attrs list) normally — it never carries
                // function values — and lower `args[1]` (the cfg record) via
                // `lower_app_cfg_record`, which intentionally omits the L0107 gate.
                // The emit side (emit_expr.rs) already destructures the cfg record
                // directly (`let Expr::Record(fields) = cfg_e`), so the record MUST
                // remain a literal — matching the require in `lower_app_cfg_record`.
                //
                // A non-literal cfg arg is currently unsupported (emit would ICE); it
                // is fail-closed via the `Expr::Record` guard in emit_expr.rs rather
                // than a separate SKY-L0119 here, since Phase-0 only wires literal
                // cfg forms and a non-literal would produce a CompilerBug diagnostic
                // with a clear location at the emit boundary.
                Callee::Kernel(
                    KernelFn::InputText
                    | KernelFn::InputEmail
                    | KernelFn::InputUsername
                    | KernelFn::InputSearch
                    | KernelFn::InputCurrentPassword
                    | KernelFn::InputNewPassword
                    | KernelFn::InputMultiline
                    | KernelFn::InputCheckbox
                    | KernelFn::InputSlider
                    | KernelFn::InputRadio
                    | KernelFn::InputRadioRow,
                ) if args.len() == 2 =>
                {
                    if let (Some(attrs_arg), Some(cfg_arg)) = (args.first(), args.get(1)) {
                        let lowered_attrs = self.lower_expr(attrs_arg)?;
                        let canon::Expr_::Record(fields) = &cfg_arg.value else {
                            // Non-literal cfg: fall through to uniform path, which
                            // surfaces SKY-L0107 on the function-valued field — a
                            // clear, actionable diagnostic at the right span.
                            return Ok(Intercepted::Fallthrough(Some(peek)));
                        };
                        let lowered_cfg = self.lower_app_cfg_record(fields)?;
                        return Ok(Intercepted::Done(Expr::Call {
                            callee: peek,
                            args: vec![lowered_attrs, lowered_cfg],
                        }));
                    }
                }
                // T4 (#108): `Live.appRouted` is a vestigial alias — the
                // reference has ONE `Live.app` that branches at emit time
                // (emit_live.rs T5).  Route it through the same
                // `lower_app_entry_cfg` path as `Live.app` so any code that
                // still calls the deprecated form compiles rather than hitting
                // SKY-L0118.  The emit branch (T5) will select `live_app` vs
                // `live_app_routed` based on whether the Model has a `page` field.
                Callee::Kernel(KernelFn::LiveAppRouted) if args.len() == 1 => {
                    if let Some(arg0) = args.first() {
                        let lowered_cfg = self.lower_app_entry_cfg(&peek, arg0)?;
                        return Ok(Intercepted::Done(Expr::Call {
                            callee: peek,
                            args: vec![lowered_cfg],
                        }));
                    }
                }
                // ── #108 round 4: `Live.route pattern PageCtor` builder peephole ──
                //
                // The canonical param-route shape passes a BARE payload
                // constructor as the page builder (`Live.route "/u/:id"
                // UserPage` with `UserPage : String -> Page`).  The uniform
                // `lower_expr` path rejects a bare payload-constructor
                // reference (`Feature::CtorAsFunction`) because a general
                // first-class constructor value is unsupported — but in THIS
                // position the constructor never becomes a first-class
                // function: `emit_live_call::LiveRoute` compiles it into the
                // route's `move |params| Ctor(param0, …)` builder closure (the
                // `route_param_get` type-directed conversion path).  Lower it
                // directly to a zero-arg `Expr::Ctor` carrier, mirroring the
                // exemption precedent of the app-cfg intercepts above.
                Callee::Kernel(KernelFn::LiveRoute) if args.len() == 2 => {
                    if let (Some(pattern_e), Some(builder_e)) = (args.first(), args.get(1)) {
                        let lowered_pattern = self.lower_expr(pattern_e)?;
                        let lowered_builder = self.lower_route_builder(builder_e)?;
                        return Ok(Intercepted::Done(Expr::Call {
                            callee: peek,
                            args: vec![lowered_pattern, lowered_builder],
                        }));
                    }
                }
                _ => {}
            }
            // Any other callee: fall through to the uniform path, carrying
            // the resolved callee so the dispatch isn't run twice.
            return Ok(Intercepted::Fallthrough(Some(peek)));
        }
        Ok(Intercepted::Fallthrough(None))
    }

    /// The uniform (non-intercepted) call lowering: lower every argument with
    /// [`Self::lower_expr`], then dispatch on the callee shape.
    ///
    /// `peeked` is the callee [`Self::intercept_live_kernel_call`] already
    /// resolved for a `VarKernel`/`VarTopLevel` callee (or `None` for every
    /// other callee shape) — reused here instead of re-running the large
    /// `lower_callee` dispatch per call (efficiency-audit §3 medium).
    fn lower_call_uniform(
        &self,
        callee: &canon::Expr,
        args: &[canon::Expr],
        call_span: Span,
        peeked: Option<Callee>,
    ) -> DResult<Expr> {
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
                // three-field `Node`) eta-expands: `|eta_k,…,eta_{N-1}| Ctor(a0,…,eta_k,…)`.
                // Over-application is ruled out by type-checking (applying past
                // the fields makes the result a non-function), so a non-equal
                // count here is always partial.
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
                    // Partial ctor application: eta-expand into a closure that
                    // captures the supplied args and takes the missing ones.
                    // Applies the same T4/#130 capture-clone discipline as
                    // `eta_expand_partial` for named-function partial application.
                    self.eta_expand_partial_ctor(
                        callee,
                        ctor_home,
                        *type_name,
                        *name,
                        lowered_args,
                        arity,
                        call_span,
                    )
                }
            }
            canon::Expr_::VarKernel { .. } | canon::Expr_::VarTopLevel { .. } => {
                let resolved = match peeked {
                    Some(c) => c,
                    None => self.lower_callee(callee)?,
                };
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
                if let Some(ty) = self.region_ty(callee.span) {
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
        let fn_ty = self.region_ty(callee.span).ok_or_else(|| {
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
        // T4 (#121/#130): the supplied args are captured inside the emitted closure.
        // A non-Copy CloneOk arg (e.g. a String-typed var) must be cloned on
        // each call so the closure is `Fn` (re-callable), not `FnOnce`.
        //
        // Var supplied args: replace `Var(sym)` with `CloneVar(sym)` for
        // CloneOk, error for NonClone, leave bare for CopyLeaf.
        //
        // Non-Var supplied args (complex expressions): hoist to a
        // `let __sky_cap_i = <expr>` binding OUTSIDE the lambda using a
        // pre-minted symbol from `cap_params`.  The lambda body uses the
        // cap symbol (clone-wrapped if CloneOk) so each call reads a
        // captured binding rather than re-evaluating the expression —
        // closing both the re-evaluation and the FnOnce hazard (T4).
        //
        // T7 (#130): unknown type (`ir_type_from_ty` returns Err) is treated
        // conservatively: if the slot type is a top-level function arrow
        // (`Ty::Fun`) the resolution failure is due to a nested polymorphic type
        // variable (e.g. `Error -> Task Error a` in a `String -> Task Error a
        // -> Task Error a` binding) — the slot is definitionally NonClone, and
        // forwarding a Var in is a plain ownership transfer into `impl FnOnce`.
        // Any other failed slot stays `None` → fail-close below (T7 original).
        let mut hoisted: Vec<(Symbol, Expr)> = Vec::new();
        let mut cap_cursor = 0usize;
        // ir_type_from_ty needs `&mut self`, so classify every supplied slot
        // BEFORE the iter_mut borrow of call_args.
        let slot_classes: Vec<Option<CloneClass>> = arg_tys
            .iter()
            .take(supplied)
            .map(|slot_ty| {
                match self.ir_type_from_ty(slot_ty, call_span) {
                    Ok(ir_ty) => Some(clone_class(&ir_ty)),
                    // T7b: ir_type_from_ty failed, but the slot's top-level type
                    // IS a function arrow.  The failure is from a nested Ty::Var
                    // (e.g. the polymorphic result type `a` in `Task Error a`).
                    // A Fun slot is always NonClone — forwarding is safe.
                    Err(_) if matches!(slot_ty, Ty::Fun(_, _)) => Some(CloneClass::NonClone),
                    // Genuinely indeterminate slot — conservative None.
                    Err(_) => None,
                }
            })
            .collect();
        for (arg, cls) in call_args.iter_mut().zip(slot_classes) {
            if let Expr::Var(sym) = *arg {
                match cls {
                    Some(CloneClass::CloneOk) => *arg = Expr::CloneVar(sym),
                    // CopyLeaf: scalar `Copy` type — bare Var is already correct;
                    //   the eta-lambda copies it by value.
                    // NonClone: a function/task/decoder variable forwarded as a
                    //   HOF callback (e.g. `Task.andThen writeAll`).  The
                    //   eta-lambda produced here is a *fresh* closure (not nested
                    //   inside another), so moving the Var in is a plain ownership
                    //   transfer — no E0525 (move-out-of-captured-env).  HOF
                    //   callbacks like `task_and_then` / `cmd_perform` take
                    //   `impl FnOnce`, so consuming the moved value once is
                    //   correct (#149).
                    Some(CloneClass::CopyLeaf | CloneClass::NonClone) => {}
                    // None — T7 (#130): the slot type is genuinely indeterminate
                    // (a non-Fun type whose resolution failed).  Conservatively
                    // fail-close with SKY-L0126: we cannot prove the Var is
                    // Copy-safe, and a NonClone value would produce E0525 at cargo.
                    // (T7b Fun-arrow case is handled in `slot_classes` above.)
                    None => {
                        return Err(unsupported(call_span, Feature::NonCloneCapture));
                    }
                }
            } else {
                // Non-Var complex expression.
                //
                // CopyLeaf slot: the expression evaluates to a Rust `Copy`
                // scalar — inlining is safe.  No hoist needed.
                //
                // CloneOk slot: hoist to a `let __sky_cap_i = <expr>` OUTSIDE
                // the lambda.  The lambda body uses `CloneVar(__sky_cap_i)` so
                // each call clones the named binding rather than re-evaluating
                // the expression with bare-moved free vars → FnOnce → E0525.
                //
                // NonClone slot: inline as-is.  A complex expression in a
                // NonClone slot constructs a FRESH value on every call (e.g.
                // `\x -> f x` constructs a new `Box<dyn Fn>` each call).  This
                // is NOT a variable capture — no move-out-of-closure issue.
                // Distinct from a Var in NonClone position (above), which IS a
                // capture.  `List.map (\x -> x + 1)` is the canonical case:
                // the lambda is a NonClone fresh construction, safe to inline.
                //
                // None (unknown / polymorphic slot): inline as-is for the same
                // reason as NonClone above.
                match cls {
                    Some(CloneClass::CloneOk) => {
                        let cap_sym = *self.cap_params.get(cap_cursor).ok_or_else(|| {
                            bug(
                                "sky_lower::eta_expand_partial",
                                "cap_params pool too small for complex-arg hoist",
                            )
                        })?;
                        cap_cursor += 1;
                        let original = std::mem::replace(arg, Expr::CloneVar(cap_sym));
                        hoisted.push((cap_sym, original));
                    }
                    // CopyLeaf, NonClone, or unknown: inline as-is.
                    Some(CloneClass::CopyLeaf | CloneClass::NonClone) | None => {}
                }
            }
        }
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
            // Use the JSON-friendly variant so that a free `Ty::Var` in the
            // missing-arg slot — the common case for diverging / always-failing
            // tasks passed to `Task.andThen` or `Cmd.perform` where the result
            // type `a` is never constrained — maps to `IrType::Json` (`JsonVal`)
            // instead of raising SKY-L0102.  The eta-param is only a closure
            // binder forwarded verbatim to the full kernel call; its concrete
            // Rust type is unified by the compiler from the call site, so
            // `JsonVal` is a sound stand-in for any unconstrained `Ty::Var`.
            let ir = self.ir_type_from_ty_json(arg_ty, call_span)?;
            params.push((sym, ir));
            call_args.push(Expr::Var(sym));
        }
        // T8 (#151 c02): use the JSON-friendly variant for the lambda return
        // type for the same reason the eta-params (above) use it: when
        // `ret_ty` is `Task a` (a 1-arg Task) and `a` is a free `Ty::Var`
        // (the common case for polymorphic helpers like
        // `wrap : String -> Task Error a -> Task Error a`), the strict
        // `ir_type_from_ty` would fail with SKY-L0102 (Polymorphism).
        // `ir_type_from_ty_json` maps the free `Ty::Var` to `IrType::Json`
        // instead — a sound stand-in since the eta-lambda's return slot is
        // type-unified by the kernel signature at the call site.
        let ret = self.ir_type_from_ty_json(ret_ty, call_span)?;
        let body = Expr::Call {
            callee: resolved,
            args: call_args,
        };
        let lambda = Expr::Lambda {
            params,
            ret,
            body: Box::new(body),
        };
        // T4 (#130): wrap any hoisted let-bindings around the lambda.
        // hoisted = [(cap_sym_0, expr_0), (cap_sym_1, expr_1), ...] in source
        // order.  Folding in reverse yields:
        //   let cap_0 = expr_0 in let cap_1 = expr_1 in <lambda>
        // which evaluates the args left-to-right before the closure is built,
        // matching Sky's pure-functional semantics.
        let result = hoisted.into_iter().rev().fold(lambda, |inner, (cap_sym, original)| {
            Expr::Let { name: cap_sym, value: Box::new(original), body: Box::new(inner) }
        });
        Ok(result)
    }

    /// Eta-expand a **partially-applied constructor** into a boxed closure that
    /// captures the supplied args and takes the remaining ones:
    ///
    /// ```text
    /// Tagged n          (arity 2, 1 supplied)
    /// ──────────────────────────────────────────
    /// Box::new(move |eta_0: String| -> Tagged { Main_Tagged(n, eta_0) })
    /// ```
    ///
    /// This is the ctor counterpart of [`Self::eta_expand_partial`] for named
    /// functions.  The T4/#130 capture-clone discipline applies identically:
    ///
    /// * `Var(sym)` supplied args are rewritten to `CloneVar(sym)` when the slot
    ///   type is `CloneOk`; left bare for `CopyLeaf`.  `NonClone` or unknown types
    ///   surface [`Feature::NonCloneCapture`] (SKY-L0126).
    /// * Non-`Var` complex expressions in `CloneOk` slots are hoisted to a
    ///   `let __sky_cap_i = <expr>` binding outside the lambda so each call reads
    ///   a captured binding (closing the re-evaluation / `FnOnce` hazard).
    ///
    /// The region type for the ctor is looked up at `callee.span`; it must peel
    /// exactly `arity` arrows.  A missing region or a shallow arrow is a compiler
    /// bug (invariant violation for well-typed input), not a missing feature.
    #[allow(clippy::too_many_arguments)] // mirrors eta_expand_partial — same ctor decomposition pattern
    fn eta_expand_partial_ctor(
        &self,
        callee: &canon::Expr,
        ctor_home: ModPath,
        type_name: Symbol,
        name: Symbol,
        lowered_args: Vec<Expr>,
        arity: usize,
        call_span: Span,
    ) -> DResult<Expr> {
        let fn_ty = self.region_ty(callee.span).ok_or_else(|| {
            bug(
                "sky_lower::eta_expand_partial_ctor",
                "no inferred type for a partially-applied constructor",
            )
        })?;
        // Peel exactly `arity` arrows: collect each argument type in order,
        // then the trailing result type R.
        let mut cur = fn_ty;
        let mut arg_tys: Vec<&Ty> = Vec::with_capacity(arity);
        for _ in 0..arity {
            let Ty::Fun(arg, rest) = cur else {
                return Err(bug(
                    "sky_lower::eta_expand_partial_ctor",
                    "constructor type has fewer arrows than its arity",
                ));
            };
            arg_tys.push(arg);
            cur = rest.as_ref();
        }
        let ret_ty = cur;

        let supplied = lowered_args.len();
        let mut params: Vec<(Symbol, IrType)> = Vec::with_capacity(arity - supplied);
        let mut call_args = lowered_args;

        // T4 (#121/#130): classify every supplied arg slot and apply capture-clone
        // discipline so the emitted closure is `Fn` (re-callable), not `FnOnce`.
        let slot_classes: Vec<Option<CloneClass>> = arg_tys
            .iter()
            .take(supplied)
            .map(|slot_ty| {
                self.ir_type_from_ty(slot_ty, call_span)
                    .ok()
                    .as_ref()
                    .map(clone_class)
            })
            .collect();
        let mut hoisted: Vec<(Symbol, Expr)> = Vec::new();
        let mut cap_cursor = 0usize;
        for (arg, cls) in call_args.iter_mut().zip(slot_classes) {
            if let Expr::Var(sym) = *arg {
                match cls {
                    Some(CloneClass::CloneOk) => *arg = Expr::CloneVar(sym),
                    Some(CloneClass::CopyLeaf) => {} // bare Var — Copy, no clone
                    // NonClone: a captured non-Clone value cannot be re-forwarded.
                    // None: unknown type on a bare Var — conservatively fail-close
                    // (T7/#130): cannot prove Copy-safety.
                    Some(CloneClass::NonClone) | None => {
                        return Err(unsupported(call_span, Feature::NonCloneCapture));
                    }
                }
            } else {
                // Non-Var complex expression: hoist CloneOk slots to a named
                // binding outside the lambda; inline CopyLeaf / NonClone / unknown.
                match cls {
                    Some(CloneClass::CloneOk) => {
                        let cap_sym = *self.cap_params.get(cap_cursor).ok_or_else(|| {
                            bug(
                                "sky_lower::eta_expand_partial_ctor",
                                "cap_params pool too small for complex-arg hoist",
                            )
                        })?;
                        cap_cursor += 1;
                        let original = std::mem::replace(arg, Expr::CloneVar(cap_sym));
                        hoisted.push((cap_sym, original));
                    }
                    Some(CloneClass::CopyLeaf | CloneClass::NonClone) | None => {}
                }
            }
        }
        // Build the missing parameter list from argument positions `supplied..arity`.
        // Use the JSON-friendly variant so a free `Ty::Var` in an unconstrained
        // slot maps to `IrType::Json` rather than raising SKY-L0102.
        for (offset, arg_ty) in arg_tys.get(supplied..).unwrap_or(&[]).iter().enumerate() {
            let sym = *self.eta_params.get(offset).ok_or_else(|| {
                bug(
                    "sky_lower::eta_expand_partial_ctor",
                    "eta-parameter pool smaller than the partial-application gap",
                )
            })?;
            let ir = self.ir_type_from_ty_json(arg_ty, call_span)?;
            params.push((sym, ir));
            call_args.push(Expr::Var(sym));
        }
        let ret = self.ir_type_from_ty(ret_ty, call_span)?;
        let body = Expr::Ctor {
            home: ctor_home,
            ty: type_name,
            variant: name,
            args: call_args,
        };
        let lambda = Expr::Lambda {
            params,
            ret,
            body: Box::new(body),
        };
        // T4 (#130): wrap any hoisted let-bindings around the lambda.
        // Folding in reverse preserves left-to-right evaluation order.
        let result = hoisted.into_iter().rev().fold(lambda, |inner, (cap_sym, original)| {
            Expr::Let { name: cap_sym, value: Box::new(original), body: Box::new(inner) }
        });
        Ok(result)
    }

    /// T6 (#121): emit an eta-adapter `Lambda` when a callee's def-arity is
    /// less than the flattened `IrType::Fun` param count at its reify site.
    ///
    /// The invariant this enforces: every `Expr::FuncValue`'s callee has a
    /// def-arity equal to its `Fun` param count.  When def-arity `k < N`, the
    /// callee is curried — its `k` declared params return an inner closure —
    /// but the reference slot expects a flat `Box<dyn Fn(T0,…,T_{N-1})->R>`.
    /// Rust rejects the mismatch with E0593; the eta-adapter makes it exact.
    ///
    /// The adapter for `mk` (def-arity k=1, `Fun([Str, Str], Page)`):
    /// ```text
    /// Lambda {
    ///     params: [(eta_0, Str), (eta_1, Str)], ret: Page,
    ///     body:   Apply { func: Call(mk, [eta_0]), args: [eta_1] }
    /// }
    /// ```
    /// The backend renders this as:
    /// ```text
    /// Box::new(move |eta_0: String, eta_1: String| -> MainPage {
    ///     (main_mk(eta_0))(eta_1)
    /// })
    /// ```
    ///
    /// A def-arity of 0 uses `Call(callee, [])` as the `Apply.func`:
    /// ```text
    /// Lambda { params: [(eta_0, T0)], ret: R,
    ///          body: Apply { func: Call(callee, []), args: [eta_0] } }
    /// ```
    fn eta_adapt_funcvalue(
        &self,
        callee: Callee,
        params: &[IrType],
        ret: &IrType,
        _span: Span,
    ) -> DResult<Expr> {
        let def_arity = self.callee_arity(&callee)?;
        let n = params.len();
        debug_assert!(
            def_arity < n,
            "eta_adapt_funcvalue called on an arity-exact callee (def_arity={def_arity}, n={n})"
        );
        // Allocate N eta params from the pool (one slot per param position).
        // Each eta-lambda is its own closure scope, so the same pool entries can
        // be reused positionally across sites without name collision.
        let mut eta_syms: Vec<Symbol> = Vec::with_capacity(n);
        for offset in 0..n {
            let sym = *self.eta_params.get(offset).ok_or_else(|| {
                bug(
                    "sky_lower::eta_adapt_funcvalue",
                    "eta-parameter pool smaller than the full function arity",
                )
            })?;
            eta_syms.push(sym);
        }
        // Typed lambda params: (sym, type) for all N positions.
        let lam_params: Vec<(Symbol, IrType)> = eta_syms
            .iter()
            .zip(params.iter())
            .map(|(&sym, ty)| (sym, ty.clone()))
            .collect();
        // Direct args: the first `def_arity` eta params go to the inner Call.
        let direct_args: Vec<Expr> = eta_syms.iter().take(def_arity).map(|&s| Expr::Var(s)).collect();
        // Inner Call: `callee(eta_0, …, eta_{k-1})` — returns the inner closure.
        let inner_call = Expr::Call {
            callee,
            args: direct_args,
        };
        // Apply: pass the remaining eta params to the returned closure.
        // `def_arity == 0` ⇒ `apply_args == eta_syms[0..]` (all params).
        let apply_args: Vec<Expr> = eta_syms.iter().skip(def_arity).map(|&s| Expr::Var(s)).collect();
        let body = Expr::Apply {
            func: Box::new(inner_call),
            args: apply_args,
        };
        Ok(Expr::Lambda {
            params: lam_params,
            ret: ret.clone(),
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
        if let Some(ty) = self.region_ty(callee.span) {
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
                | KernelFn::ErrorPermissionDenied
                // ── Task.defaultRetryPolicy — arity 0 ────────────────────────
                | KernelFn::TaskDefaultRetryPolicy
                // ── #127: Sky.Http.Server.WebSocket arity-0 ──────────────────
                | KernelFn::WsDefaultCfg
                // ── Jwt builder: claims arity-0 (D-00, #152) ─────────────────
                | KernelFn::JwtClaims,
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
                // ── Basics numerics (#115) — arity 1 ────────────────────────
                | KernelFn::BasicsNegate
                | KernelFn::BasicsAbs
                | KernelFn::BasicsSqrt
                // ── end Basics numerics (#115) ──────────────────────────────
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
                // ── Math arity-1 (Float → Bool) ──────────────────────────────
                | KernelFn::MathIsNaN
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
                | KernelFn::TaskPerform
                | KernelFn::TaskLazy
                // ── Task.withJitter — arity 1 ────────────────────────────────
                | KernelFn::TaskWithJitter
                // ── Io arity-1 (M5a) ──────────────────────────────────────────
                | KernelFn::IoReadLine
                | KernelFn::IoWriteStdout
                | KernelFn::IoWriteStderr
                // ── Time arity-1 (M5a) ────────────────────────────────────────
                | KernelFn::TimeNow
                | KernelFn::TimeSleep
                | KernelFn::TimeUnixMillis
                | KernelFn::TimeTimeString
                | KernelFn::TimeIsLeapYear
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
                // `money : String -> Decoder (Decimal, String)` (#34)
                | KernelFn::DbDecMoney
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
                // `Middleware.withCsrf : Handler -> Handler`
                | KernelFn::MiddlewareWithCsrf
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
                // `Error.isRetryable : Error -> Bool` (#85/#160)
                | KernelFn::ErrorIsRetryable
                // ── CssSafety arity-1 (Std.Css leaf kernels, #47) ─────────────
                // `safeValue`/`safePropName`/`safeSelector : String -> Maybe String`
                // `stripStyleClose : String -> String`
                | KernelFn::CssSafetySafeValue
                | KernelFn::CssSafetySafePropName
                | KernelFn::CssSafetySafeSelector
                | KernelFn::CssSafetyStripStyleClose
                // ── Jwt builder arity-1 (D-00, #152) ─────────────────────────
                // `hs256 : String -> Algorithm`
                // `rs256 : String -> Algorithm`
                | KernelFn::JwtHs256
                | KernelFn::JwtRs256,
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
                // ── Basics numerics (#115) — arity 2 ────────────────────────
                | KernelFn::BasicsMin
                | KernelFn::BasicsMax
                // `compare : comparable -> comparable -> Order` — arity 2 (#123)
                | KernelFn::BasicsCompare
                // ── end Basics numerics (#115) ──────────────────────────────
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
                // ── Time arity-2 (Std.Time calendar helper) ─────────────────
                | KernelFn::TimeDaysInMonth
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
                // ── Jwt builder arity-2 (D-00, #152) ──────────────────────────
                | KernelFn::JwtSubject
                | KernelFn::JwtIssuer
                | KernelFn::JwtAudience
                | KernelFn::JwtExpiresAt
                | KernelFn::JwtNotBefore
                | KernelFn::JwtIssuedAt
                | KernelFn::JwtJwtId
                | KernelFn::JwtEncode
                // ── Task combinators arity-2 (M5a) ────────────────────────────
                | KernelFn::TaskMap
                | KernelFn::TaskAndThen
                | KernelFn::TaskMapError
                | KernelFn::TaskOnError
                | KernelFn::TaskAndThenResult
                // ── Task retry surface arity-2 ────────────────────────────────
                | KernelFn::TaskRetryWith
                | KernelFn::TaskLinearBackoff
                | KernelFn::TaskExponentialBackoff
                | KernelFn::TaskRetryOn
                | KernelFn::TaskWithRetryOn
                | KernelFn::TaskWithMaxAttempts
                | KernelFn::TaskWithBaseMs
                | KernelFn::TaskWithKind
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
                // `Sub.subscribeTopic : String -> (any -> msg) -> Sub msg`  (M5d wired)
                | KernelFn::SubSubscribeTopic
                // ── TEA arity-2 (M6 reserved — not emitted yet) ───────────────
                // `Cmd.publish : String -> a -> Cmd msg`
                | KernelFn::CmdPublish
                // `Cmd.publishNoEcho : String -> a -> Cmd msg`
                | KernelFn::CmdPublishNoEcho
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
                | KernelFn::ErrorWithMessage
                // `Error.withDetails : ErrorDetails -> Error -> Error` (#85 follow-up)
                | KernelFn::ErrorWithDetails,
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
                // `DbInsertRow : Db -> String -> Dict String String -> Task Error Int`
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
                | KernelFn::MiddlewareWithBasicAuth
                // ── Jwt builder arity-3 (D-00, #152) ─────────────────────────
                // `withClaim : String -> String -> Claims -> Claims`
                | KernelFn::JwtWithClaim
                // `Jwt.decode : Algorithm -> Int -> String -> Result Error String`
                | KernelFn::JwtDecode,
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
                // `DbUpdateById : Db -> String -> String -> Dict String String -> Task Error Int`
                | KernelFn::DbUpdateById
                // `DbFindOneByField : Db -> String -> String -> String -> Task Error (Maybe Row)`
                | KernelFn::DbFindOneByField
                // `DbFindManyByField : Db -> String -> String -> String -> Task Error (List Row)`
                | KernelFn::DbFindManyByField
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
                // `Ui.clipX : Attribute msg`
                | KernelFn::UiClipX
                // `Ui.clipY : Attribute msg`
                | KernelFn::UiClipY
                // `Ui.scrollbars : Attribute msg`
                | KernelFn::UiScrollbars
                // `Ui.scrollbarX : Attribute msg`
                | KernelFn::UiScrollbarX
                // `Ui.scrollbarY : Attribute msg`
                | KernelFn::UiScrollbarY
                // `Font.bold : Attribute msg`
                | KernelFn::FontBold
                // `Font.italic : Attribute msg`
                | KernelFn::FontItalic
                // `Attr.noAttr : Attribute msg` (#76)
                | KernelFn::HtmlNoAttr
                // ── #76 Tier 1 — nullary attrs ────────────────────────────────
                | KernelFn::UiSquare
                | KernelFn::UiWidescreen
                | KernelFn::UiCinemascope
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
                | KernelFn::FontLineThrough
                | KernelFn::FontAlignLeft
                | KernelFn::FontAlignRight
                | KernelFn::FontAlignCenter
                | KernelFn::FontCenter
                | KernelFn::FontJustify
                // Font string constants (nullary, return String)
                | KernelFn::FontSansSerif
                | KernelFn::FontSerif
                | KernelFn::FontMonospace
                // ── Std.Ui.Region (#117) — arity-0 attrs ─────────────────────────
                | KernelFn::RegionMainContent
                | KernelFn::RegionNavigation
                | KernelFn::RegionFooter
                | KernelFn::RegionAside
                | KernelFn::RegionAnnounce
                | KernelFn::RegionAnnounceUrgently
                // ── Ui.describe desc* constructors — arity-0 ────────────────
                | KernelFn::UiDescMain
                | KernelFn::UiDescNavigation
                | KernelFn::UiDescContentInfo
                | KernelFn::UiDescComplementary
                | KernelFn::UiDescLivePolite
                | KernelFn::UiDescLiveAssertive
                // ── #154: Breakpoint constants — return String, arity 0 ──────
                | KernelFn::UiMobile
                | KernelFn::UiTablet
                | KernelFn::UiDesktop
                | KernelFn::UiDarkMode
                | KernelFn::UiLightMode
                | KernelFn::UiReducedMotion
                // ── #76: PseudoClass constants — return PseudoClass, arity 0 ──
                | KernelFn::UiHover
                | KernelFn::UiFocus
                | KernelFn::UiFocusVisible
                | KernelFn::UiActive
                | KernelFn::UiDisabled,
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
                // ── M7: Std.Ui nearby attribute builders — arity 1 ───────────
                // `Ui.above/below/onLeft/onRight/inFront/behind : Element msg -> Attribute msg`
                | KernelFn::UiAbove
                | KernelFn::UiBelow
                | KernelFn::UiOnLeft
                | KernelFn::UiOnRight
                | KernelFn::UiInFront
                | KernelFn::UiBehind
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
                // `Border.widthEach : { top : Int, right : Int, bottom : Int, left : Int } -> Attribute msg`
                | KernelFn::BorderWidthEach
                // `Border.shadow : { offsetX, offsetY, blur, spread : Int, color : Color } -> Attribute msg`
                | KernelFn::BorderShadow
                // `Border.innerShadow : { offsetX, offsetY, blur, spread : Int, color : Color } -> Attribute msg`
                | KernelFn::BorderInnerShadow
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
                // `Html.linkNode : List (Attribute msg) -> Html msg` (void element alias)
                | KernelFn::HtmlLinkNode
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
                // `Ui.onSubmit : (a -> msg) -> Attribute msg`
                | KernelFn::UiOnSubmit
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
                // `Cli.program : CliCfg model msg -> Task Error ()` (#111)
                | KernelFn::CliProgram
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
                | KernelFn::HtmlAttrAutocomplete
                // ── #76 Tier 1 — arity 1 ────────────────────────────────────
                | KernelFn::UiAspectRatio
                | KernelFn::UiName
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
                | KernelFn::HtmlAttrTabindex
                | KernelFn::HtmlAttrRows
                // ── Std.Ui.Region (#117) — arity-1 attrs ─────────────────────────
                | KernelFn::RegionHeading
                | KernelFn::RegionLabel
                // ── Ui.input + Ui.describe + desc* arity-1 ──────────────────────
                | KernelFn::UiInput
                | KernelFn::UiDescribe
                | KernelFn::UiDescHeading
                | KernelFn::UiDescLabel
                // ── Std.Ui.Input (#124) — arity-1 constructors ───────────────────
                // `Input.labelHidden : String -> Label msg`
                | KernelFn::InputLabelHidden
                // ── #76: 20-kernel wiring batch — arity 1 ─────────────────────
                // `Ui.paddingEach : { top, right, bottom, left : Int } -> Attribute msg`
                | KernelFn::UiPaddingEach
                // `Ui.onFile : (String -> msg) -> Attribute msg`
                | KernelFn::UiOnFile
                // `Html.doctype : List (Html msg) -> Html msg`
                | KernelFn::HtmlDoctype
                // `Html.titleNode : String -> Html msg`
                | KernelFn::HtmlTitleNode
                // `Html.toString : Html msg -> String`
                | KernelFn::HtmlToString,
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
                // `Ui.paragraph : List (Attribute msg) -> List (Element msg) -> Element msg`
                | KernelFn::UiParagraph
                // `Ui.textColumn : List (Attribute msg) -> List (Element msg) -> Element msg`
                | KernelFn::UiTextColumn
                // `Ui.form : List (Attribute msg) -> List (Element msg) -> Element msg`
                | KernelFn::UiForm
                // `Ui.button : List (Attribute msg) -> { onPress : Maybe msg, label : Element msg } -> Element msg`
                | KernelFn::UiButton
                // `Ui.link : List (Attribute msg) -> { url : String, label : Element msg } -> Element msg`
                | KernelFn::UiLink
                // `Ui.paddingXY : Int -> Int -> Attribute msg`
                | KernelFn::UiPaddingXY
                // `Border.glow : Int -> Color -> Attribute msg`
                | KernelFn::BorderGlow
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
                // `Html.headerNode : List (Attribute msg) -> List (Html msg) -> Html msg`
                // (legacy compat alias — same <header> tag, arity 2)
                | KernelFn::HtmlHeaderNode
                // `Html.codeNode : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlCodeNode
                // `Html.mainNode : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlMainNode
                // `Html.footerNode : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlFooterNode
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
                // `Html.title : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlTitle
                // `Html.htmlNode : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlHtmlNode
                // `Html.headNode : List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlHeadNode
                // `Live.route : String -> page -> LiveRoute` (#106: `page` is a
                // bare polymorphic value — nullary ctor OR `String -> Page`)
                | KernelFn::LiveRoute
                // `Live.renderStatic : LiveAppCfg model msg -> String -> Task Error String`
                | KernelFn::LiveRenderStatic
                // #76: generic `Attr.attribute k v` / `Attr.boolAttribute k b`.
                | KernelFn::HtmlAttribute
                | KernelFn::HtmlBoolAttribute
                // -- #76 Tier 1 -- arity 2
                | KernelFn::UiAspectRatioWH
                | KernelFn::UiHtmlAttribute
                | KernelFn::UiStyle
                // `Ui.transitionRaw : String -> Bool -> Attribute msg`
                | KernelFn::UiTransitionRaw
                // `Ui.gridTracksRaw : String -> String -> Attribute msg`
                | KernelFn::UiGridTracksRaw
                // ── Std.Ui.Input (#124) — arity-2 constructors ───────────────────
                // `Input.labelAbove : List (Attribute msg) -> Element msg -> Label msg`
                | KernelFn::InputLabelAbove
                // `Input.labelBelow : List (Attribute msg) -> Element msg -> Label msg`
                | KernelFn::InputLabelBelow
                // `Input.labelLeft : List (Attribute msg) -> Element msg -> Label msg`
                | KernelFn::InputLabelLeft
                // `Input.labelRight : List (Attribute msg) -> Element msg -> Label msg`
                | KernelFn::InputLabelRight
                // `Input.placeholder : List (Attribute msg) -> Element msg -> Placeholder msg`
                | KernelFn::InputPlaceholder
                // `Input.text : List (Attribute msg) -> { ... } -> Element msg`
                | KernelFn::InputText
                // `Input.multiline : List (Attribute msg) -> { ..., spellcheck : Bool } -> Element msg`
                | KernelFn::InputMultiline
                // `Input.email : List (Attribute msg) -> { ... } -> Element msg`
                | KernelFn::InputEmail
                // `Input.username : List (Attribute msg) -> { ... } -> Element msg`
                | KernelFn::InputUsername
                // `Input.search : List (Attribute msg) -> { ... } -> Element msg`
                | KernelFn::InputSearch
                // `Input.currentPassword : List (Attribute msg) -> { ... } -> Element msg`
                | KernelFn::InputCurrentPassword
                // `Input.newPassword : List (Attribute msg) -> { ... } -> Element msg`
                | KernelFn::InputNewPassword
                // `Input.checkbox : List (Attribute msg) -> { onChange : Bool -> msg, ... } -> Element msg`
                | KernelFn::InputCheckbox
                // `Input.slider : List (Attribute msg) -> { onChange, value, min, max, step, label } -> Element msg`
                | KernelFn::InputSlider
                // `Input.option : String -> Element msg -> RadioOption msg`
                | KernelFn::InputOption
                // `Input.radio : List (Attribute msg) -> { onChange, options, selected, label } -> Element msg`
                | KernelFn::InputRadio
                // `Input.radioRow : List (Attribute msg) -> { onChange, options, selected, label } -> Element msg`
                | KernelFn::InputRadioRow
                // ── #76: 20-kernel wiring batch — arity 2 ─────────────────────
                // `Ui.image : List (Attribute msg) -> { src : String, description : String } -> Element msg`
                | KernelFn::UiImage
                // `Background.linearGradient : Float -> List (Float, Color) -> Attribute msg`
                | KernelFn::BackgroundLinearGradient
                // `Html.voidNode : String -> List (Attribute msg) -> Html msg`
                | KernelFn::HtmlVoidNode
                // `Ui.onPseudo : PseudoClass -> List (Attribute msg) -> Attribute msg`
                | KernelFn::UiOnPseudo,
            ) => Ok(2),
            // Arity 3: `Ui.rgb r g b`, `Html.node tag attrs children`,
            //          `Ui.breakpoint query attrs element`.
            Callee::Kernel(
                // `Ui.rgb : Int -> Int -> Int -> Color`
                KernelFn::UiRgb
                // `Html.node : String -> List (Attribute msg) -> List (Html msg) -> Html msg`
                | KernelFn::HtmlNode
                // `Ui.breakpoint : String -> List (Attribute msg) -> Element msg -> Element msg`
                // (delegates to Ui.mediaQuery at runtime — see ui_breakpoint_)
                | KernelFn::UiBreakpoint
                // `Ui.mediaQuery : String -> List (Attribute msg) -> Element msg -> Element msg`
                // Raw-query escape hatch — marker-carrying wrapper consumed by
                // live::style_inject::build_mq (see ui_media_query_).
                | KernelFn::UiMediaQuery,
            ) => Ok(3),
            // Arity 4: `Ui.rgba r g b a`.
            Callee::Kernel(
                // `Ui.rgba : Int -> Int -> Int -> Float -> Color`
                KernelFn::UiRgba,
            ) => Ok(4),
            // ── #111: Std.Auth / Stream / HttpStream — fail-closed kernels ──
            // These kernels are registered in the qualifier table but have no
            // lower arm (they hit SKY-L0108).  callee_arity is consulted
            // before lowering, so each variant must declare its correct arity
            // (matching the `decl()` table in sky_kernels).
            Callee::Kernel(
                KernelFn::AuthHashPassword
                | KernelFn::AuthPasswordStrength
                | KernelFn::StreamFinish
                | KernelFn::HttpStreamOpen
                | KernelFn::HttpStreamClose
                | KernelFn::UiColorCss
                // ── #127: Sky.Http.Server.WebSocket arity-1 ──────────────────
                | KernelFn::WsCloseClient,
            ) => Ok(1),
            Callee::Kernel(
                KernelFn::AuthHashPasswordCost
                | KernelFn::AuthVerifyPassword
                | KernelFn::AuthVerifyToken
                | KernelFn::StreamStream
                | KernelFn::StreamEmit
                | KernelFn::StreamWithContentType
                | KernelFn::HttpStreamForEachChunk
                | KernelFn::HttpStreamChunks
                // ── #127: Sky.Http.Server.WebSocket arity-2 ──────────────────
                | KernelFn::WsWithOnConnect
                | KernelFn::WsWithOnMessage
                | KernelFn::WsWithOnClose
                | KernelFn::WsWithOnError
                | KernelFn::WsWithMaxMessageBytes
                | KernelFn::WsWithOriginPatterns
                | KernelFn::WsUpgrade
                | KernelFn::WsSendToClient
                | KernelFn::WsSendBinaryToClient
                | KernelFn::WsBroadcast,
            ) => Ok(2),
            Callee::Kernel(
                KernelFn::AuthSignToken
                | KernelFn::AuthRegister
                | KernelFn::AuthLogin
                | KernelFn::AuthSetRole,
            ) => Ok(3),
            // ── Std.Ui.Lazy (#146) ────────────────────────────────────────────
            // lazy  : (a -> Element msg) -> a -> Element msg          — arity 2
            Callee::Kernel(KernelFn::LazyLazy) => Ok(2),
            // Keyed.column/row : List Attr -> List (String, Element) -> Element — arity 2
            Callee::Kernel(KernelFn::KeyedColumn | KernelFn::KeyedRow) => Ok(2),
            // lazy2 : (a -> b -> Element msg) -> a -> b -> …          — arity 3
            Callee::Kernel(KernelFn::LazyLazy2) => Ok(3),
            // lazy3 : (a -> b -> c -> Element msg) -> …               — arity 4
            Callee::Kernel(KernelFn::LazyLazy3) => Ok(4),
            // lazy4 : (a -> b -> c -> d -> Element msg) -> …          — arity 5
            Callee::Kernel(KernelFn::LazyLazy4) => Ok(5),
            // lazy5 : (a -> b -> c -> d -> e -> Element msg) -> …     — arity 6
            Callee::Kernel(KernelFn::LazyLazy5) => Ok(6),
            // ── Std.Decimal — arity 0 ────────────────────────────────────────
            Callee::Kernel(
                KernelFn::DecZero
                | KernelFn::DecOne
                | KernelFn::DecOneHundred,
            ) => Ok(0),
            // ── Std.Decimal — arity 1 ────────────────────────────────────────
            Callee::Kernel(
                KernelFn::DecFromString
                | KernelFn::DecFromInt
                | KernelFn::DecFromFloat
                | KernelFn::DecToString
                | KernelFn::DecToFloat
                | KernelFn::DecToInt
                | KernelFn::DecNeg
                | KernelFn::DecAbs
                | KernelFn::DecFloor
                | KernelFn::DecCeil
                | KernelFn::DecIsZero
                | KernelFn::DecIsPositive
                | KernelFn::DecIsNegative,
            ) => Ok(1),
            // ── Std.Decimal — arity 2 ────────────────────────────────────────
            Callee::Kernel(
                KernelFn::DecFromMinor
                | KernelFn::DecToStringFixed
                | KernelFn::DecToMinor
                | KernelFn::DecAdd
                | KernelFn::DecSub
                | KernelFn::DecMul
                | KernelFn::DecDiv
                | KernelFn::DecMod
                | KernelFn::DecRound
                | KernelFn::DecRoundHalfUp
                | KernelFn::DecTruncate
                | KernelFn::DecCompare
                | KernelFn::DecEq
                | KernelFn::DecNeq
                | KernelFn::DecLt
                | KernelFn::DecLte
                | KernelFn::DecGt
                | KernelFn::DecGte
                | KernelFn::DecMin
                | KernelFn::DecMax
                | KernelFn::DecPercentOf
                | KernelFn::DecAddPercent
                | KernelFn::DecSubPercent,
            ) => Ok(2),
            // ── Std.Decimal — arity 4 ────────────────────────────────────────
            // `Decimal.formatWith : String -> String -> Int -> Decimal -> String`
            Callee::Kernel(KernelFn::DecFormatWith) => Ok(4),
            // ── Std.Db.Sql — SqlFragment builder, arity 1 (backlog #61) ──────
            // `column : String -> SqlFragment`, `param : SqlValue -> SqlFragment`,
            // `int`/`string`/`float`/`bool : _ -> SqlFragment` (sugar over
            // `param`), `not`/`isNull`/`isNotNull : SqlFragment -> SqlFragment`.
            Callee::Kernel(
                KernelFn::SqlColumn
                | KernelFn::SqlParam
                | KernelFn::SqlInt
                | KernelFn::SqlString
                | KernelFn::SqlFloat
                | KernelFn::SqlBool
                | KernelFn::SqlNot
                | KernelFn::SqlIsNull
                | KernelFn::SqlIsNotNull,
            ) => Ok(1),
            // ── Std.Db.Sql — SqlFragment builder, arity 2 ─────────────────────
            // `eq`/`ne`/`gt`/`lt`/`gte`/`lte`/`and`/`or : SqlFragment -> SqlFragment -> SqlFragment`,
            // `inList : SqlFragment -> List SqlValue -> SqlFragment`,
            // `like : SqlFragment -> String -> SqlFragment`.
            Callee::Kernel(
                KernelFn::SqlEq
                | KernelFn::SqlNe
                | KernelFn::SqlGt
                | KernelFn::SqlLt
                | KernelFn::SqlGte
                | KernelFn::SqlLte
                | KernelFn::SqlAnd
                | KernelFn::SqlOr
                | KernelFn::SqlInList
                | KernelFn::SqlLike,
            ) => Ok(2),
            // ── Db.findWhere / Db.deleteWhere — arity 3 (backlog #61) ─────────
            // `findWhere : Db -> String -> SqlFragment -> Task Error (List Row)`
            // `deleteWhere : Db -> String -> SqlFragment -> Task Error Int`
            Callee::Kernel(KernelFn::DbFindWhere | KernelFn::DbDeleteWhere) => Ok(3),
            // ── Sky.Core.Secret — opaque secret-string wrapper, arity 1 (#44) ──
            // `fromString : String -> Secret`, `reveal : Secret -> String`,
            // `redacted : Secret -> String`.
            Callee::Kernel(
                KernelFn::SecretFromString | KernelFn::SecretReveal | KernelFn::SecretRedacted,
            ) => Ok(1),
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
        match self.region_ty(span) {
            Some(Ty::Con { name, args, .. }) => {
                self.resolve(*name).is_ok_and(|n| n == "Result")
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

    /// Resolve a named callee (`Maybe.andMap`, `String.length`, a user
    /// top-level function, …) to its [`Callee`], then run the #90 T3
    /// curried-`andMap`-payload backstop over the RESULT.
    ///
    /// **Revert-incident Bug 3 (BACKLOG #90).** The first two #90 landings
    /// (`f80f05a`/`39d9a57`, both reverted) ran the curried-payload check
    /// from INSIDE [`Self::lower_call_uniform`]'s `VarKernel | VarTopLevel`
    /// arm — which only sees a callee that is the DIRECT callee of a `Call`
    /// AST node. A bare-value reference to `Result.andMap` /
    /// `Maybe.andMap` — passed as a higher-order argument, `let`-bound as a
    /// point-free alias (`myAndMap = Result.andMap`), extracted from a
    /// record field, or re-exported through an `import … as …` alias — never
    /// passes through a `Call` node at all; it lowers through
    /// [`Self::lower_expr`]'s bare-value arm instead, which calls
    /// [`Self::lower_callee_resolve`] (below) directly. That second call site
    /// never ran the check, so `myAndMap (Ok 1) (Ok add3curried)` reached
    /// `cargo build` as E0277 despite the previous fix.
    ///
    /// The fix: this wrapper is now the SINGLE funnel both callers go
    /// through — [`Self::lower_call_uniform`]'s direct-call arm and
    /// [`Self::lower_expr`]'s bare-value arm both call `lower_callee`
    /// (never `lower_callee_resolve` directly) — so every literal AST
    /// occurrence of `Result.andMap` / `Maybe.andMap`, in ANY syntactic
    /// position, is checked exactly once, by construction, regardless of how
    /// many more lowering arms are added later. This is a lowering-time
    /// BACKSTOP (Tier 1) behind the primary type-checker obligation
    /// (`sky_types::constrain::constrain_var_kernel`'s `hof_kernel_result`
    /// `TyBounds` tie, Tier 2 — see
    /// `docs/architecture/ctor-payload-andmap-arity-gate-design.md` §3.2):
    /// Tier 2 already rejects the hazard as a type error (`SKY-T0014`)
    /// before lowering ever runs; this backstop gives a second, independent
    /// line of defense keyed on the ACTUAL kernel-call resolution boundary
    /// rather than any particular AST shape. Scope note: this Tier-1
    /// backstop covers the `andMap` kernels ONLY (its peeling logic reads
    /// the `Con (a -> b)` payload position specific to `andMap`'s scheme);
    /// the `map`/`map2..5`/`mapError` members of the hazard family are
    /// covered by Tier 2 alone, whose fail-closed predicate
    /// (`sky_types::emitted_bound_satisfied`, rejecting both `Ty::Fun` and
    /// bare `Ty::Var`) is the load-bearing gate for every member.
    fn lower_callee(&self, callee: &canon::Expr) -> DResult<Callee> {
        let resolved = self.lower_callee_resolve(callee)?;
        self.reject_curried_andmap_payload(&resolved, callee)?;
        Ok(resolved)
    }

    #[allow(clippy::too_many_lines)] // declarative kernel-name dispatch table
    fn lower_callee_resolve(&self, callee: &canon::Expr) -> DResult<Callee> {
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
                    // ── Basics numerics (#115) ────────────────────────────────
                    ("Basics", "negate") => Ok(Callee::Kernel(KernelFn::BasicsNegate)),
                    ("Basics", "abs")    => Ok(Callee::Kernel(KernelFn::BasicsAbs)),
                    ("Basics", "sqrt")   => Ok(Callee::Kernel(KernelFn::BasicsSqrt)),
                    ("Basics", "min")    => Ok(Callee::Kernel(KernelFn::BasicsMin)),
                    ("Basics", "max")    => Ok(Callee::Kernel(KernelFn::BasicsMax)),
                    // `compare : comparable -> comparable -> Order` (#123)
                    ("Basics", "compare") => Ok(Callee::Kernel(KernelFn::BasicsCompare)),
                    // ── end Basics numerics (#115) ────────────────────────────
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
                    // `errorToString` is reachable from both `Basics.errorToString` (Prelude)
                    // and `Error.toString` (qualified form) — same kernel either way.
                    ("Basics", "errorToString") | ("Error", "toString") => {
                        Ok(Callee::Kernel(KernelFn::ErrorToString))
                    }
                    ("Error", "withMessage") => Ok(Callee::Kernel(KernelFn::ErrorWithMessage)),
                    ("Error", "isRetryable") => Ok(Callee::Kernel(KernelFn::ErrorIsRetryable)),
                    ("Error", "withDetails") => Ok(Callee::Kernel(KernelFn::ErrorWithDetails)),
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
                    // ── Math arity-1 (Float → Bool) ──────────────────────────
                    ("Math", "isNaN") => Ok(Callee::Kernel(KernelFn::MathIsNaN)),
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
                    // ── Jwt builder API (D-00, #152) ──────────────────────────
                    ("Jwt", "claims") => Ok(Callee::Kernel(KernelFn::JwtClaims)),
                    ("Jwt", "hs256") => Ok(Callee::Kernel(KernelFn::JwtHs256)),
                    ("Jwt", "rs256") => Ok(Callee::Kernel(KernelFn::JwtRs256)),
                    ("Jwt", "subject") => Ok(Callee::Kernel(KernelFn::JwtSubject)),
                    ("Jwt", "issuer") => Ok(Callee::Kernel(KernelFn::JwtIssuer)),
                    ("Jwt", "audience") => Ok(Callee::Kernel(KernelFn::JwtAudience)),
                    ("Jwt", "expiresAt") => Ok(Callee::Kernel(KernelFn::JwtExpiresAt)),
                    ("Jwt", "notBefore") => Ok(Callee::Kernel(KernelFn::JwtNotBefore)),
                    ("Jwt", "issuedAt") => Ok(Callee::Kernel(KernelFn::JwtIssuedAt)),
                    ("Jwt", "jwtId") => Ok(Callee::Kernel(KernelFn::JwtJwtId)),
                    ("Jwt", "withClaim") => Ok(Callee::Kernel(KernelFn::JwtWithClaim)),
                    ("Jwt", "encode") => Ok(Callee::Kernel(KernelFn::JwtEncode)),
                    ("Jwt", "decode") => Ok(Callee::Kernel(KernelFn::JwtDecode)),
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
                    ("Task", "perform") => Ok(Callee::Kernel(KernelFn::TaskPerform)),
                    ("Task", "lazy") => Ok(Callee::Kernel(KernelFn::TaskLazy)),
                    // ── Task retry surface (M5a) ──────────────────────────────
                    ("Task", "retryWith") => Ok(Callee::Kernel(KernelFn::TaskRetryWith)),
                    ("Task", "linearBackoff") => Ok(Callee::Kernel(KernelFn::TaskLinearBackoff)),
                    ("Task", "exponentialBackoff") => {
                        Ok(Callee::Kernel(KernelFn::TaskExponentialBackoff))
                    }
                    ("Task", "withJitter") => Ok(Callee::Kernel(KernelFn::TaskWithJitter)),
                    ("Task", "retryOn") => Ok(Callee::Kernel(KernelFn::TaskRetryOn)),
                    ("Task", "withRetryOn") => Ok(Callee::Kernel(KernelFn::TaskWithRetryOn)),
                    ("Task", "defaultRetryPolicy") => {
                        Ok(Callee::Kernel(KernelFn::TaskDefaultRetryPolicy))
                    }
                    ("Task", "withMaxAttempts") => {
                        Ok(Callee::Kernel(KernelFn::TaskWithMaxAttempts))
                    }
                    ("Task", "withBaseMs") => Ok(Callee::Kernel(KernelFn::TaskWithBaseMs)),
                    ("Task", "withKind") => Ok(Callee::Kernel(KernelFn::TaskWithKind)),
                    // ── Io kernels (M5a) ──────────────────────────────────────
                    ("Io", "readLine") => Ok(Callee::Kernel(KernelFn::IoReadLine)),
                    ("Io", "writeStdout") => Ok(Callee::Kernel(KernelFn::IoWriteStdout)),
                    ("Io", "writeStderr") => Ok(Callee::Kernel(KernelFn::IoWriteStderr)),
                    // ── Time kernels (M5a) ────────────────────────────────────
                    ("Time", "now") => Ok(Callee::Kernel(KernelFn::TimeNow)),
                    ("Time", "sleep") => Ok(Callee::Kernel(KernelFn::TimeSleep)),
                    ("Time", "unixMillis") => Ok(Callee::Kernel(KernelFn::TimeUnixMillis)),
                    ("Time", "timeString") => Ok(Callee::Kernel(KernelFn::TimeTimeString)),
                    ("Time", "isLeapYear") => Ok(Callee::Kernel(KernelFn::TimeIsLeapYear)),
                    ("Time", "daysInMonth") => Ok(Callee::Kernel(KernelFn::TimeDaysInMonth)),
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
                    // ── Sql kernels (#61 SqlFragment combinators) ──────────
                    ("Sql", "column") => Ok(Callee::Kernel(KernelFn::SqlColumn)),
                    ("Sql", "param") => Ok(Callee::Kernel(KernelFn::SqlParam)),
                    ("Sql", "int") => Ok(Callee::Kernel(KernelFn::SqlInt)),
                    ("Sql", "string") => Ok(Callee::Kernel(KernelFn::SqlString)),
                    ("Sql", "float") => Ok(Callee::Kernel(KernelFn::SqlFloat)),
                    ("Sql", "bool") => Ok(Callee::Kernel(KernelFn::SqlBool)),
                    ("Sql", "eq") => Ok(Callee::Kernel(KernelFn::SqlEq)),
                    ("Sql", "ne") => Ok(Callee::Kernel(KernelFn::SqlNe)),
                    ("Sql", "gt") => Ok(Callee::Kernel(KernelFn::SqlGt)),
                    ("Sql", "lt") => Ok(Callee::Kernel(KernelFn::SqlLt)),
                    ("Sql", "gte") => Ok(Callee::Kernel(KernelFn::SqlGte)),
                    ("Sql", "lte") => Ok(Callee::Kernel(KernelFn::SqlLte)),
                    ("Sql", "and") => Ok(Callee::Kernel(KernelFn::SqlAnd)),
                    ("Sql", "or") => Ok(Callee::Kernel(KernelFn::SqlOr)),
                    ("Sql", "not") => Ok(Callee::Kernel(KernelFn::SqlNot)),
                    ("Sql", "isNull") => Ok(Callee::Kernel(KernelFn::SqlIsNull)),
                    ("Sql", "isNotNull") => Ok(Callee::Kernel(KernelFn::SqlIsNotNull)),
                    ("Sql", "inList") => Ok(Callee::Kernel(KernelFn::SqlInList)),
                    ("Sql", "like") => Ok(Callee::Kernel(KernelFn::SqlLike)),
                    ("Db", "findWhere") => Ok(Callee::Kernel(KernelFn::DbFindWhere)),
                    ("Db", "deleteWhere") => Ok(Callee::Kernel(KernelFn::DbDeleteWhere)),
                    // ── Secret kernels (backlog #44) ──────────────────────────
                    ("Secret", "fromString") => Ok(Callee::Kernel(KernelFn::SecretFromString)),
                    ("Secret", "reveal") => Ok(Callee::Kernel(KernelFn::SecretReveal)),
                    ("Secret", "redacted") => Ok(Callee::Kernel(KernelFn::SecretRedacted)),
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
                    ("Db.Decode", "money") => Ok(Callee::Kernel(KernelFn::DbDecMoney)),
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
                    // ── TEA Cmd / Sub / Time kernels (M5c / M5e) ─────────────────
                    ("Cmd", "none") => Ok(Callee::Kernel(KernelFn::CmdNone)),
                    ("Cmd", "batch") => Ok(Callee::Kernel(KernelFn::CmdBatch)),
                    ("Cmd", "perform") => Ok(Callee::Kernel(KernelFn::CmdPerform)),
                    // `Cmd.publish : String -> Dict String String -> Cmd msg` (M5e)
                    ("Cmd", "publish") => Ok(Callee::Kernel(KernelFn::CmdPublish)),
                    // `Cmd.publishNoEcho : String -> Dict String String -> Cmd msg` (M5e)
                    ("Cmd", "publishNoEcho") => Ok(Callee::Kernel(KernelFn::CmdPublishNoEcho)),
                    ("Sub", "none") => Ok(Callee::Kernel(KernelFn::SubNone)),
                    ("Sub", "batch") => Ok(Callee::Kernel(KernelFn::SubBatch)),
                    ("Sub", "every") => Ok(Callee::Kernel(KernelFn::SubEvery)),
                    ("Sub", "subscribeTopic") => Ok(Callee::Kernel(KernelFn::SubSubscribeTopic)),
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
                    ("Middleware", "withCsrf") => Ok(Callee::Kernel(KernelFn::MiddlewareWithCsrf)),
                    ("RateLimit", "allow") => Ok(Callee::Kernel(KernelFn::RateLimitAllow)),
                    // ── M7: Std.Ui / Std.Html render kernels ─────────────────
                    ("Ui", "layout") => Ok(Callee::Kernel(KernelFn::UiLayout)),
                    ("Ui", "layoutWith") => Ok(Callee::Kernel(KernelFn::UiLayoutWith)),
                    ("Html", "render") => Ok(Callee::Kernel(KernelFn::HtmlRender)),
                    // `Html.toString` is a distinct arity-1 kernel (decl() name
                    // "toString") that shares `HtmlRender`'s runtime fn
                    // (`html_render_`) but is a SEPARATE `KernelFn` variant so
                    // `decl_equiv_legacy_match` sees a 1:1 name↔variant mapping.
                    ("Html", "toString") => Ok(Callee::Kernel(KernelFn::HtmlToString)),
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
                    ("Ui", "paragraph") => Ok(Callee::Kernel(KernelFn::UiParagraph)),
                    ("Ui", "textColumn") => Ok(Callee::Kernel(KernelFn::UiTextColumn)),
                    ("Ui", "form") => Ok(Callee::Kernel(KernelFn::UiForm)),
                    ("Ui", "button") => Ok(Callee::Kernel(KernelFn::UiButton)),
                    ("Ui", "link") => Ok(Callee::Kernel(KernelFn::UiLink)),
                    ("Ui", "image") => Ok(Callee::Kernel(KernelFn::UiImage)),
                    // ── M7: Std.Ui nearby attribute builders ───────────────────
                    ("Ui", "above") => Ok(Callee::Kernel(KernelFn::UiAbove)),
                    ("Ui", "below") => Ok(Callee::Kernel(KernelFn::UiBelow)),
                    ("Ui", "onLeft") => Ok(Callee::Kernel(KernelFn::UiOnLeft)),
                    ("Ui", "onRight") => Ok(Callee::Kernel(KernelFn::UiOnRight)),
                    ("Ui", "inFront") => Ok(Callee::Kernel(KernelFn::UiInFront)),
                    ("Ui", "behind") => Ok(Callee::Kernel(KernelFn::UiBehind)),
                    // ── M7: Std.Ui attribute builders ─────────────────────────
                    ("Ui", "spacing") => Ok(Callee::Kernel(KernelFn::UiSpacing)),
                    ("Ui", "padding") => Ok(Callee::Kernel(KernelFn::UiPadding)),
                    ("Ui", "paddingXY") => Ok(Callee::Kernel(KernelFn::UiPaddingXY)),
                    ("Ui", "paddingEach") => Ok(Callee::Kernel(KernelFn::UiPaddingEach)),
                    ("Ui", "width") => Ok(Callee::Kernel(KernelFn::UiWidth)),
                    ("Ui", "height") => Ok(Callee::Kernel(KernelFn::UiHeight)),
                    ("Ui", "centerX") => Ok(Callee::Kernel(KernelFn::UiCenterX)),
                    ("Ui", "centerY") => Ok(Callee::Kernel(KernelFn::UiCenterY)),
                    ("Ui", "alignLeft") => Ok(Callee::Kernel(KernelFn::UiAlignLeft)),
                    ("Ui", "alignRight") => Ok(Callee::Kernel(KernelFn::UiAlignRight)),
                    ("Ui", "alignTop") => Ok(Callee::Kernel(KernelFn::UiAlignTop)),
                    ("Ui", "alignBottom") => Ok(Callee::Kernel(KernelFn::UiAlignBottom)),
                    ("Ui", "pointer") => Ok(Callee::Kernel(KernelFn::UiPointer)),
                    ("Ui", "clip") => Ok(Callee::Kernel(KernelFn::UiClip)),
                    // ── #76: clipX/clipY/scrollbarX/scrollbarY are distinct
                    // per-axis kernels (different `AttrOverflow` values than
                    // `clip`/`scrollbars`) — the former combined arm above
                    // silently folded them onto the wrong (both-axes) semantics.
                    ("Ui", "clipX") => Ok(Callee::Kernel(KernelFn::UiClipX)),
                    ("Ui", "clipY") => Ok(Callee::Kernel(KernelFn::UiClipY)),
                    ("Ui", "scrollbars") => Ok(Callee::Kernel(KernelFn::UiScrollbars)),
                    ("Ui", "scrollbarX") => Ok(Callee::Kernel(KernelFn::UiScrollbarX)),
                    ("Ui", "scrollbarY") => Ok(Callee::Kernel(KernelFn::UiScrollbarY)),
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
                    ("Ui", "colorCss") => Ok(Callee::Kernel(KernelFn::UiColorCss)),
                    // ── M7: Background sub-module ─────────────────────────────
                    ("Background", "color") => Ok(Callee::Kernel(KernelFn::BackgroundColor)),
                    ("Background", "image") => Ok(Callee::Kernel(KernelFn::BackgroundImage)),
                    ("Background", "linearGradient") => {
                        Ok(Callee::Kernel(KernelFn::BackgroundLinearGradient))
                    }
                    // ── M7: Border sub-module ─────────────────────────────────
                    ("Border", "width") => Ok(Callee::Kernel(KernelFn::BorderWidth)),
                    ("Border", "rounded") => Ok(Callee::Kernel(KernelFn::BorderRounded)),
                    ("Border", "color") => Ok(Callee::Kernel(KernelFn::BorderColor)),
                    ("Border", "widthEach") => Ok(Callee::Kernel(KernelFn::BorderWidthEach)),
                    ("Border", "shadow") => Ok(Callee::Kernel(KernelFn::BorderShadow)),
                    ("Border", "glow") => Ok(Callee::Kernel(KernelFn::BorderGlow)),
                    ("Border", "innerShadow") => Ok(Callee::Kernel(KernelFn::BorderInnerShadow)),
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
                    ("Html", "node") => Ok(Callee::Kernel(KernelFn::HtmlNode)),
                    // ── #76: 20-kernel wiring batch — each of these 5 is now its
                    // own dedicated `KernelFn` variant (distinct arity from the
                    // generic arity-3 `Html.node`); the former combined arm
                    // above silently mis-arities them onto `HtmlNode`.
                    ("Html", "voidNode") => Ok(Callee::Kernel(KernelFn::HtmlVoidNode)),
                    ("Html", "doctype") => Ok(Callee::Kernel(KernelFn::HtmlDoctype)),
                    ("Html", "titleNode") => Ok(Callee::Kernel(KernelFn::HtmlTitleNode)),
                    ("Html", "htmlNode") => Ok(Callee::Kernel(KernelFn::HtmlHtmlNode)),
                    ("Html", "headNode") => Ok(Callee::Kernel(KernelFn::HtmlHeadNode)),
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
                    ("Html", "headerNode") => Ok(Callee::Kernel(KernelFn::HtmlHeaderNode)),
                    ("Html", "codeNode") => Ok(Callee::Kernel(KernelFn::HtmlCodeNode)),
                    ("Html", "mainNode") => Ok(Callee::Kernel(KernelFn::HtmlMainNode)),
                    ("Html", "footerNode") => Ok(Callee::Kernel(KernelFn::HtmlFooterNode)),
                    ("Html", "linkNode") => Ok(Callee::Kernel(KernelFn::HtmlLinkNode)),
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
                    ("Html", "title") => Ok(Callee::Kernel(KernelFn::HtmlTitle)),
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
                    ("Attr", "autocomplete") => Ok(Callee::Kernel(KernelFn::HtmlAttrAutocomplete)),
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
                    ("Ui", "onSubmit") => Ok(Callee::Kernel(KernelFn::UiOnSubmit)),
                    ("Ui", "onFile") => Ok(Callee::Kernel(KernelFn::UiOnFile)),
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
                    ("Ui", "cinemascope") => Ok(Callee::Kernel(KernelFn::UiCinemascope)),
                    ("Ui", "aspectRatio") => Ok(Callee::Kernel(KernelFn::UiAspectRatio)),
                    ("Ui", "aspectRatioWH") => Ok(Callee::Kernel(KernelFn::UiAspectRatioWH)),
                    ("Ui", "htmlAttribute") => Ok(Callee::Kernel(KernelFn::UiHtmlAttribute)),
                    ("Ui", "name") => Ok(Callee::Kernel(KernelFn::UiName)),
                    ("Ui", "style") => Ok(Callee::Kernel(KernelFn::UiStyle)),
                    ("Ui", "transitionRaw") => {
                        Ok(Callee::Kernel(KernelFn::UiTransitionRaw))
                    }
                    ("Ui", "gridTracksRaw") => {
                        Ok(Callee::Kernel(KernelFn::UiGridTracksRaw))
                    }
                    // #154: Ui.breakpoint + Breakpoint constants
                    ("Ui", "breakpoint") => Ok(Callee::Kernel(KernelFn::UiBreakpoint)),
                    ("Ui", "mediaQuery") => Ok(Callee::Kernel(KernelFn::UiMediaQuery)),
                    ("Ui", "mobile") => Ok(Callee::Kernel(KernelFn::UiMobile)),
                    ("Ui", "tablet") => Ok(Callee::Kernel(KernelFn::UiTablet)),
                    ("Ui", "desktop") => Ok(Callee::Kernel(KernelFn::UiDesktop)),
                    ("Ui", "darkMode") => Ok(Callee::Kernel(KernelFn::UiDarkMode)),
                    ("Ui", "lightMode") => Ok(Callee::Kernel(KernelFn::UiLightMode)),
                    ("Ui", "reducedMotion") => Ok(Callee::Kernel(KernelFn::UiReducedMotion)),
                    // ── #76: PseudoClass constants + Ui.onPseudo ──────────────
                    ("Ui", "onPseudo") => Ok(Callee::Kernel(KernelFn::UiOnPseudo)),
                    ("Ui", "hover") => Ok(Callee::Kernel(KernelFn::UiHover)),
                    ("Ui", "focus") => Ok(Callee::Kernel(KernelFn::UiFocus)),
                    ("Ui", "focusVisible") => Ok(Callee::Kernel(KernelFn::UiFocusVisible)),
                    ("Ui", "active") => Ok(Callee::Kernel(KernelFn::UiActive)),
                    ("Ui", "disabled") => Ok(Callee::Kernel(KernelFn::UiDisabled)),
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
                    ("Font", "lineThrough") => Ok(Callee::Kernel(KernelFn::FontLineThrough)),
                    ("Font", "letterSpacing") => Ok(Callee::Kernel(KernelFn::FontLetterSpacing)),
                    ("Font", "wordSpacing") => Ok(Callee::Kernel(KernelFn::FontWordSpacing)),
                    ("Font", "alignLeft") => Ok(Callee::Kernel(KernelFn::FontAlignLeft)),
                    ("Font", "alignRight") => Ok(Callee::Kernel(KernelFn::FontAlignRight)),
                    ("Font", "alignCenter") => Ok(Callee::Kernel(KernelFn::FontAlignCenter)),
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
                    ("Attr", "rows")     => Ok(Callee::Kernel(KernelFn::HtmlAttrRows)),
                    // ── #117: Std.Ui.Region sub-module ───────────────────────
                    ("Region", "mainContent") => {
                        Ok(Callee::Kernel(KernelFn::RegionMainContent))
                    }
                    ("Region", "navigation") => Ok(Callee::Kernel(KernelFn::RegionNavigation)),
                    ("Region", "footer") => Ok(Callee::Kernel(KernelFn::RegionFooter)),
                    ("Region", "aside") => Ok(Callee::Kernel(KernelFn::RegionAside)),
                    ("Region", "heading") => Ok(Callee::Kernel(KernelFn::RegionHeading)),
                    ("Region", "label") => Ok(Callee::Kernel(KernelFn::RegionLabel)),
                    ("Region", "announce") => Ok(Callee::Kernel(KernelFn::RegionAnnounce)),
                    ("Region", "announceUrgently") => {
                        Ok(Callee::Kernel(KernelFn::RegionAnnounceUrgently))
                    }
                    // ── Ui.input + Ui.describe + desc* constructors ───────────
                    ("Ui", "input") => Ok(Callee::Kernel(KernelFn::UiInput)),
                    ("Ui", "describe") => Ok(Callee::Kernel(KernelFn::UiDescribe)),
                    ("Ui", "descMain") => Ok(Callee::Kernel(KernelFn::UiDescMain)),
                    ("Ui", "descNavigation") => Ok(Callee::Kernel(KernelFn::UiDescNavigation)),
                    ("Ui", "descContentInfo") => Ok(Callee::Kernel(KernelFn::UiDescContentInfo)),
                    ("Ui", "descComplementary") => {
                        Ok(Callee::Kernel(KernelFn::UiDescComplementary))
                    }
                    ("Ui", "descLivePolite") => Ok(Callee::Kernel(KernelFn::UiDescLivePolite)),
                    ("Ui", "descLiveAssertive") => {
                        Ok(Callee::Kernel(KernelFn::UiDescLiveAssertive))
                    }
                    ("Ui", "descHeading") => Ok(Callee::Kernel(KernelFn::UiDescHeading)),
                    ("Ui", "descLabel") => Ok(Callee::Kernel(KernelFn::UiDescLabel)),
                    // ── Std.Ui.Input (#124) ───────────────────────────────────
                    ("Input", "labelAbove") => Ok(Callee::Kernel(KernelFn::InputLabelAbove)),
                    ("Input", "labelBelow") => Ok(Callee::Kernel(KernelFn::InputLabelBelow)),
                    ("Input", "labelLeft") => Ok(Callee::Kernel(KernelFn::InputLabelLeft)),
                    ("Input", "labelRight") => Ok(Callee::Kernel(KernelFn::InputLabelRight)),
                    ("Input", "labelHidden") => Ok(Callee::Kernel(KernelFn::InputLabelHidden)),
                    ("Input", "placeholder") => Ok(Callee::Kernel(KernelFn::InputPlaceholder)),
                    ("Input", "text") => Ok(Callee::Kernel(KernelFn::InputText)),
                    ("Input", "multiline") => Ok(Callee::Kernel(KernelFn::InputMultiline)),
                    ("Input", "email") => Ok(Callee::Kernel(KernelFn::InputEmail)),
                    ("Input", "username") => Ok(Callee::Kernel(KernelFn::InputUsername)),
                    ("Input", "search") => Ok(Callee::Kernel(KernelFn::InputSearch)),
                    ("Input", "currentPassword") => {
                        Ok(Callee::Kernel(KernelFn::InputCurrentPassword))
                    }
                    ("Input", "newPassword") => Ok(Callee::Kernel(KernelFn::InputNewPassword)),
                    ("Input", "checkbox") => Ok(Callee::Kernel(KernelFn::InputCheckbox)),
                    ("Input", "slider") => Ok(Callee::Kernel(KernelFn::InputSlider)),
                    ("Input", "option") => Ok(Callee::Kernel(KernelFn::InputOption)),
                    ("Input", "radio") => Ok(Callee::Kernel(KernelFn::InputRadio)),
                    ("Input", "radioRow") => Ok(Callee::Kernel(KernelFn::InputRadioRow)),
                    // ── #146: Std.Ui.Lazy sub-module ──────────────────────────
                    ("Lazy", "lazy")  => Ok(Callee::Kernel(KernelFn::LazyLazy)),
                    ("Lazy", "lazy2") => Ok(Callee::Kernel(KernelFn::LazyLazy2)),
                    ("Lazy", "lazy3") => Ok(Callee::Kernel(KernelFn::LazyLazy3)),
                    ("Lazy", "lazy4") => Ok(Callee::Kernel(KernelFn::LazyLazy4)),
                    ("Lazy", "lazy5") => Ok(Callee::Kernel(KernelFn::LazyLazy5)),
                    // ── Std.Ui.Keyed ──────────────────────────────────────────
                    ("Keyed", "column") => Ok(Callee::Kernel(KernelFn::KeyedColumn)),
                    ("Keyed", "row")    => Ok(Callee::Kernel(KernelFn::KeyedRow)),
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
                    // ── #111: Std.Cli / Sky.Cli app-entry kernel ──────────────
                    ("Cli", "program") => Ok(Callee::Kernel(KernelFn::CliProgram)),
                    // ── #111: Std.Auth / Sky.Auth — auth helpers ──────────────
                    ("Auth", "hashPassword") => {
                        Ok(Callee::Kernel(KernelFn::AuthHashPassword))
                    }
                    ("Auth", "hashPasswordCost") => {
                        Ok(Callee::Kernel(KernelFn::AuthHashPasswordCost))
                    }
                    ("Auth", "verifyPassword") => {
                        Ok(Callee::Kernel(KernelFn::AuthVerifyPassword))
                    }
                    ("Auth", "passwordStrength") => {
                        Ok(Callee::Kernel(KernelFn::AuthPasswordStrength))
                    }
                    ("Auth", "signToken") => Ok(Callee::Kernel(KernelFn::AuthSignToken)),
                    ("Auth", "verifyToken") => Ok(Callee::Kernel(KernelFn::AuthVerifyToken)),
                    ("Auth", "register") => Ok(Callee::Kernel(KernelFn::AuthRegister)),
                    ("Auth", "login") => Ok(Callee::Kernel(KernelFn::AuthLogin)),
                    ("Auth", "setRole") => Ok(Callee::Kernel(KernelFn::AuthSetRole)),
                    // ── #111: Sky.Http.Server.Stream — server-side streaming ───
                    ("Stream", "stream") => Ok(Callee::Kernel(KernelFn::StreamStream)),
                    ("Stream", "emit") => Ok(Callee::Kernel(KernelFn::StreamEmit)),
                    ("Stream", "finish") => Ok(Callee::Kernel(KernelFn::StreamFinish)),
                    ("Stream", "withContentType") => {
                        Ok(Callee::Kernel(KernelFn::StreamWithContentType))
                    }
                    // ── #111: Sky.Core.Http.Stream — client-side streaming ─────
                    ("HttpStream", "open") => Ok(Callee::Kernel(KernelFn::HttpStreamOpen)),
                    ("HttpStream", "forEachChunk") => {
                        Ok(Callee::Kernel(KernelFn::HttpStreamForEachChunk))
                    }
                    ("HttpStream", "close") => Ok(Callee::Kernel(KernelFn::HttpStreamClose)),
                    ("HttpStream", "chunks") => Ok(Callee::Kernel(KernelFn::HttpStreamChunks)),
                    // ── #127: Sky.Http.Server.WebSocket (12 kernels) ─────────────
                    ("Ws", "defaultCfg") => Ok(Callee::Kernel(KernelFn::WsDefaultCfg)),
                    ("Ws", "withOnConnect") => Ok(Callee::Kernel(KernelFn::WsWithOnConnect)),
                    ("Ws", "withOnMessage") => Ok(Callee::Kernel(KernelFn::WsWithOnMessage)),
                    ("Ws", "withOnClose") => Ok(Callee::Kernel(KernelFn::WsWithOnClose)),
                    ("Ws", "withOnError") => Ok(Callee::Kernel(KernelFn::WsWithOnError)),
                    ("Ws", "withMaxMessageBytes") => {
                        Ok(Callee::Kernel(KernelFn::WsWithMaxMessageBytes))
                    }
                    ("Ws", "withOriginPatterns") => {
                        Ok(Callee::Kernel(KernelFn::WsWithOriginPatterns))
                    }
                    ("Ws", "upgrade") => Ok(Callee::Kernel(KernelFn::WsUpgrade)),
                    ("Ws", "sendToClient") => Ok(Callee::Kernel(KernelFn::WsSendToClient)),
                    ("Ws", "sendBinaryToClient") => {
                        Ok(Callee::Kernel(KernelFn::WsSendBinaryToClient))
                    }
                    ("Ws", "broadcast") => Ok(Callee::Kernel(KernelFn::WsBroadcast)),
                    ("Ws", "closeClient") => Ok(Callee::Kernel(KernelFn::WsCloseClient)),
                    // ── Std.Decimal ───────────────────────────────────────────
                    ("Decimal", "zero")        => Ok(Callee::Kernel(KernelFn::DecZero)),
                    ("Decimal", "one")         => Ok(Callee::Kernel(KernelFn::DecOne)),
                    ("Decimal", "oneHundred")  => Ok(Callee::Kernel(KernelFn::DecOneHundred)),
                    ("Decimal", "fromString")  => Ok(Callee::Kernel(KernelFn::DecFromString)),
                    ("Decimal", "fromInt")     => Ok(Callee::Kernel(KernelFn::DecFromInt)),
                    ("Decimal", "fromFloat")   => Ok(Callee::Kernel(KernelFn::DecFromFloat)),
                    ("Decimal", "fromMinor")   => Ok(Callee::Kernel(KernelFn::DecFromMinor)),
                    ("Decimal", "toString")    => Ok(Callee::Kernel(KernelFn::DecToString)),
                    ("Decimal", "toStringFixed") => Ok(Callee::Kernel(KernelFn::DecToStringFixed)),
                    ("Decimal", "toFloat")     => Ok(Callee::Kernel(KernelFn::DecToFloat)),
                    ("Decimal", "toInt")       => Ok(Callee::Kernel(KernelFn::DecToInt)),
                    ("Decimal", "toMinor")     => Ok(Callee::Kernel(KernelFn::DecToMinor)),
                    ("Decimal", "add")         => Ok(Callee::Kernel(KernelFn::DecAdd)),
                    ("Decimal", "sub")         => Ok(Callee::Kernel(KernelFn::DecSub)),
                    ("Decimal", "mul")         => Ok(Callee::Kernel(KernelFn::DecMul)),
                    ("Decimal", "div")         => Ok(Callee::Kernel(KernelFn::DecDiv)),
                    ("Decimal", "mod")         => Ok(Callee::Kernel(KernelFn::DecMod)),
                    ("Decimal", "neg")         => Ok(Callee::Kernel(KernelFn::DecNeg)),
                    ("Decimal", "abs")         => Ok(Callee::Kernel(KernelFn::DecAbs)),
                    ("Decimal", "floor")       => Ok(Callee::Kernel(KernelFn::DecFloor)),
                    ("Decimal", "ceil")        => Ok(Callee::Kernel(KernelFn::DecCeil)),
                    ("Decimal", "round")       => Ok(Callee::Kernel(KernelFn::DecRound)),
                    ("Decimal", "roundHalfUp") => Ok(Callee::Kernel(KernelFn::DecRoundHalfUp)),
                    ("Decimal", "truncate")    => Ok(Callee::Kernel(KernelFn::DecTruncate)),
                    ("Decimal", "compare")     => Ok(Callee::Kernel(KernelFn::DecCompare)),
                    ("Decimal", "eq")          => Ok(Callee::Kernel(KernelFn::DecEq)),
                    ("Decimal", "neq")         => Ok(Callee::Kernel(KernelFn::DecNeq)),
                    ("Decimal", "lt")          => Ok(Callee::Kernel(KernelFn::DecLt)),
                    ("Decimal", "lte")         => Ok(Callee::Kernel(KernelFn::DecLte)),
                    ("Decimal", "gt")          => Ok(Callee::Kernel(KernelFn::DecGt)),
                    ("Decimal", "gte")         => Ok(Callee::Kernel(KernelFn::DecGte)),
                    ("Decimal", "min")         => Ok(Callee::Kernel(KernelFn::DecMin)),
                    ("Decimal", "max")         => Ok(Callee::Kernel(KernelFn::DecMax)),
                    ("Decimal", "isZero")      => Ok(Callee::Kernel(KernelFn::DecIsZero)),
                    ("Decimal", "isPositive")  => Ok(Callee::Kernel(KernelFn::DecIsPositive)),
                    ("Decimal", "isNegative")  => Ok(Callee::Kernel(KernelFn::DecIsNegative)),
                    ("Decimal", "percentOf")   => Ok(Callee::Kernel(KernelFn::DecPercentOf)),
                    ("Decimal", "addPercent")  => Ok(Callee::Kernel(KernelFn::DecAddPercent)),
                    ("Decimal", "subPercent")  => Ok(Callee::Kernel(KernelFn::DecSubPercent)),
                    ("Decimal", "formatWith")  => Ok(Callee::Kernel(KernelFn::DecFormatWith)),
                    // A kernel beyond the wired set.
                    // [SKY-L0108, feature: kernels]
                    (q, m) => {
                        let _ = (q, m);
                        Err(unsupported(callee.span, Feature::Kernels))
                    }
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
            // `++` (String only) — the Binop arm in `lower_expr` intercepts
            // "append" first and routes `List _` operands to
            // `KernelFn::ListAppend`; this arm is only reached when the solved
            // type is `String`.
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
    fn lower_payload_pat(&self, p: &canon::Pattern) -> DResult<Pat> {
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
                Box::new(self.lower_payload_pat(inner)?),
                name.value,
            )),
            canon::Pattern_::PTuple(elems) => {
                let subs = elems
                    .iter()
                    .map(|e| self.lower_payload_pat(e))
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
                    .map(|a| self.lower_payload_pat(a))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Pat::Ctor {
                    home: ModPath(home.clone()),
                    ty: *type_name,
                    variant: *name,
                    args: subs,
                })
            }
            // A record sub-pattern nested in a constructor payload (`Ok {name}`).
            // The payload field's complete record type is recovered from the
            // per-sub-pattern region the constraint generator now records (Class 4
            // item C / #158), then `lower_record_pat` builds the complete
            // `Pat::Record` the same way a top-level `case` / `let` binder does.
            // `Pat::Record` nested inside `Pat::Ctor.args` is an already-permitted
            // IR shape and lowers to valid Rust struct-pattern nesting.
            canon::Pattern_::PRecord(fields) => {
                let ty = self.region_ty(p.span).ok_or_else(|| {
                    bug(
                        "sky_lower::lower_payload_pat",
                        "nested record sub-pattern has no solved region type",
                    )
                })?;
                self.lower_record_pat(fields, ty, p.span)
            }
            // List / cons sub-patterns nested in a constructor payload cannot be
            // slice-pattern-matched inline against a `Vec<T>` enum FIELD; the
            // arm-level guard desugaring that handles them lives one level up in
            // `lower_arm_pat` (which owns the whole arm + body). Reaching a
            // PList / PCons HERE means the shape is nested via some OTHER path
            // (two levels deep, `Ok (Just (h :: t))`, out of this item's scope) —
            // fail-closed (SKY-L0116). Class 4 item C / #158.
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
    fn lower_destructure_pat(&self, p: &canon::Pattern) -> DResult<Pat> {
        match &p.value {
            canon::Pattern_::PVar(s) => Ok(Pat::Var(*s)),
            canon::Pattern_::PAnything => Ok(Pat::Wildcard),
            canon::Pattern_::PTuple(elems) => {
                let subs = elems
                    .iter()
                    .map(|e| self.lower_destructure_pat(e))
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
                Box::new(self.lower_destructure_pat(inner)?),
                name.value,
            )),
            // A record pattern nested inside a tuple destructure (`(Ok {name}, y)`
            // single-arm form, `({ x }, y) = e`). The element's complete record
            // type is recovered from the per-sub-pattern region the constraint
            // generator now records (Class 4 item C / #158) — same recovery a
            // top-level record binder uses via `lower_binder_pat`.
            canon::Pattern_::PRecord(fields) => {
                let ty = self.region_ty(p.span).ok_or_else(|| {
                    bug(
                        "sky_lower::lower_destructure_pat",
                        "nested record sub-pattern has no solved region type",
                    )
                })?;
                self.lower_record_pat(fields, ty, p.span)
            }
            // List / cons elements are refutable AND have no `Vec` match lowering
            // in an irrefutable destructure position — fail-closed (SKY-L0116).
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
                let ty = self.region_ty(value.span).ok_or_else(|| {
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
            _ => self.lower_destructure_pat(pat),
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

    /// Can this MULTI-arm product `case` lower to a native Rust tuple `match`?
    ///
    /// The backend emits a tuple `match` by matching a tuple built column-by-
    /// column from the scrutinee's element expressions (`match (e0.as_slice(),
    /// e1) { … }`), so the supported shape is deliberately narrow and every
    /// accepted program is SOUND (skyc-0 ⟹ cargo-0):
    ///
    /// * the scrutinee is a literal tuple `( e0, e1, … )` — the backend needs the
    ///   element expressions to apply the per-column slice/`&str` coercions
    ///   without evaluating the scrutinee twice;
    /// * every arm head is a tuple of the scrutinee's arity, or a `_` wildcard
    ///   catch-all (a whole-value variable / alias binder over a coerced tuple
    ///   would see the wrong element types, so it stays fail-closed); and
    /// * a column matched by a cons / list sub-pattern that BINDS a value must
    ///   have a CONCRETE (`Clone`) element type — a still-generic element would
    ///   make the backend's owned-rebind (`.clone()` / `.to_vec()`) emit Rust
    ///   that fails `cargo` (the same SKY-L0102 gate the flat list path applies).
    ///
    /// Returns `Ok(true)` to proceed (the caller falls through to the general
    /// flat-`match` lowering), `Ok(false)` for an unsupported product shape (the
    /// caller raises SKY-L0115), or `Err` for the polymorphic-element gate
    /// (SKY-L0102) — a precise diagnostic rather than the generic product gap.
    fn tuple_case_supported(
        &self,
        scrut: &canon::Expr,
        branches: &[canon::CaseBranch],
    ) -> DResult<bool> {
        let canon::Expr_::Tuple(elems) = &scrut.value else {
            return Ok(false);
        };
        let arity = elems.len();
        // Every arm is a tuple of the scrutinee's arity, or an irrefutable `_`
        // catch-all. A bare variable / alias whole-tuple binder is rejected: in a
        // coerced-column `match` it would bind the wrong (per-column-coerced)
        // tuple type.
        for br in branches {
            match &br.pat.value {
                canon::Pattern_::PTuple(cols) if cols.len() == arity => {
                    // A nested tuple column would need its OWN per-column slice /
                    // `&str` coercion, which the backend applies only at the top
                    // level; leave nested products fail-closed for now.
                    if cols
                        .iter()
                        .any(|c| matches!(c.value, canon::Pattern_::PTuple(_)))
                    {
                        return Ok(false);
                    }
                }
                canon::Pattern_::PAnything => {}
                _ => return Ok(false),
            }
        }
        // Per-column polymorphic-element gate: a column bound by a cons / list
        // sub-pattern needs a concrete element type so the backend's owned
        // rebind resolves. Mirrors the flat list `case` guard, applied per tuple
        // column against that column's scrutinee element type.
        for (col, elem) in elems.iter().enumerate() {
            let col_binds_list = branches.iter().any(|br| {
                let canon::Pattern_::PTuple(cols) = &br.pat.value else {
                    return false;
                };
                cols.get(col).is_some_and(|c| {
                    matches!(
                        c.value,
                        canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _)
                    ) && Self::pat_binds_canon_value(&c.value)
                })
            });
            if col_binds_list && matches!(self.list_elem_ir(elem.span)?, IrType::Generic(_)) {
                return Err(unsupported(elem.span, Feature::Polymorphism));
            }
        }
        Ok(true)
    }

    /// Does this canonical arm pattern BIND at least one value (a variable /
    /// alias, or any binder nested inside a tuple / cons / list / ctor / record)?
    /// A purely structural pattern (wildcards / literals only) binds nothing, so
    /// a list column matched by `[] -> … ; _ :: _ -> …` needs no owned rebind and
    /// escapes the polymorphic-element gate.
    fn pat_binds_canon_value(pat: &canon::Pattern_) -> bool {
        !collect_arm_pat_pvars(pat).is_empty()
    }

    /// Build a [`Pat::Record`] from a field-pun record pattern and the scrutinee's
    /// solved record type. The pattern names a subset of the record's fields
    /// (`{ x }` on a `{ x, y }` record is legal); the COMPLETE field set is
    /// emitted — each named field a [`Pat::Var`] binder, every other field a
    /// [`Pat::Wildcard`] — so the backend resolves the struct from the full
    /// field-name set, exactly as a record literal does. Entries are ordered by
    /// resolved field name for deterministic output.
    fn lower_record_pat(&self, fields: &[Located<Symbol>], ty: &Ty, span: Span) -> DResult<Pat> {
        let Ty::Record(rec, _tail) = ty else {
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
            self.region_ty(span),
            Some(Ty::Con { name, .. })
                if self.interner.resolve(*name).is_some_and(|n| n == "Task")
        )
    }

    /// Return `Some(IrType::Decoder(inner))` when the solved type at `span` is
    /// `Decoder T`, or `None` for any other type (including unsolvable inner
    /// types, which will surface as diagnostics at emit time).
    ///
    /// Used by [`lower_let`] to decide whether a named binding should be thunked
    /// into a zero-arg lambda so the value can be rebuilt per use (F2 — Decoder
    /// is `!Clone` and `decode_from_json_string` consumes it by value).
    fn decoder_ir_type(&self, span: Span) -> Option<IrType> {
        let ty = self.region_ty(span)?;
        let Ty::Con { name, args, .. } = ty else {
            return None;
        };
        if self.interner.resolve(*name).is_none_or(|n| n != "Decoder") {
            return None;
        }
        let inner_ty = args.first()?;
        // If the inner type cannot be lowered (e.g. it is a polymorphic variable
        // the M0 lowerer cannot yet handle), return None — the binding will
        // proceed through the standard path and surface the real diagnostic.
        self.ir_type_from_ty(inner_ty, span)
            .ok()
            .map(|inner| IrType::Decoder(Box::new(inner)))
    }

    /// Build the [`Expr`] for a destructure-binder `let` / single-arm-`case`
    /// binding, applying the #125 Decoder-thunk generalization when `value`'s
    /// aggregate type contains [`IrType::Decoder`] anywhere. Falls through to
    /// a plain [`Expr::Destructure`] (byte-identical to pre-#125 emission)
    /// when it does not.
    ///
    /// The Decoder path generalizes #89 Fix C to multi-name binders: the
    /// whole `value` is wrapped in a zero-arg thunk lambda and EVERY name the
    /// binder binds gets its free reads rewritten to a fresh, masked
    /// re-destructure of a thunk call
    /// (`{ let (d1, _) = (destr_thunk_N)(); d1 }`) — see
    /// `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` §2.
    /// Sound for the same reason Fix C is sound: Decoders are pure values, so
    /// re-evaluating the construction per read is semantics-neutral.
    /// Uniformly thunking ALL bound names (Decoder-typed or not) mirrors Fix
    /// C's own "unconditional, no use-count gate" decision — mixing bound-
    /// directly and bound-via-thunk names in ONE Rust binding statement is
    /// not representable without literal tuple/field projection, which the IR
    /// deliberately does not have.
    ///
    /// `canon_value` is the CANON (pre-lowering) expression the binding
    /// evaluates — [`Self::captured_locals`] needs it (not the lowered
    /// `value`) for the T3 (#121) capture-clone analysis on the thunk body,
    /// exactly as [`Self::lower_let`]'s `PVar` Decoder arm does.
    fn build_destructure_or_decoder_thunk(
        &self,
        binder: Pat,
        value: Expr,
        value_span: Span,
        body: Expr,
        canon_value: &canon::Expr,
    ) -> DResult<Expr> {
        let value_ir_ty = self
            .region_ty(value_span)
            .and_then(|ty| self.ir_type_from_ty(ty, value_span).ok());
        let Some(ir_ty) = value_ir_ty.filter(ir_type_contains_decoder) else {
            return Ok(Expr::Destructure {
                binder,
                value: Box::new(value),
                body: Box::new(body),
            });
        };

        // T3 (#121)-style capture-clone rewrite on the thunk body, mirroring
        // the PVar arm exactly: the thunk has zero params, so every free
        // VarLocal in `canon_value` is an outer capture.
        let thunk_body = {
            let captures = self.captured_locals(&[], canon_value);
            let mut clone_set: BTreeSet<Symbol> = BTreeSet::new();
            let mut noncl_set: BTreeSet<Symbol> = BTreeSet::new();
            for (sym, ir_ty) in captures {
                match ir_ty.as_ref().map(clone_class) {
                    Some(CloneClass::CloneOk) => {
                        clone_set.insert(sym);
                    }
                    Some(CloneClass::NonClone) => {
                        noncl_set.insert(sym);
                    }
                    Some(CloneClass::CopyLeaf) | None => {}
                }
            }
            rewrite_captured_clones(&clone_set, &noncl_set, value_span, value, 0)?
        };
        let thunk_name = self.fresh_destructure_thunk_symbol()?;
        let thunk = Expr::Lambda {
            params: vec![],
            ret: ir_ty,
            body: Box::new(thunk_body),
        };

        let mut bound: BTreeSet<Symbol> = BTreeSet::new();
        pat_bound_symbols(&binder, &mut bound);
        let mut new_body = body;
        for name in &bound {
            new_body = rewrite_destructure_read(*name, &binder, thunk_name, new_body);
        }
        Ok(Expr::Let {
            name: thunk_name,
            value: Box::new(thunk),
            body: Box::new(new_body),
        })
    }

    fn lower_let(&self, bindings: &[canon::LetBinding], body: &canon::Expr) -> DResult<Expr> {
        let mut acc = self.lower_expr(body)?;
        for b in bindings.iter().rev() {
            let value = self.lower_expr(&b.body)?;
            acc = match &b.pat.value {
                canon::Pattern_::PVar(name) => {
                    // F2 — Decoder thunk: `Decoder` is `!Clone` and every
                    // function that consumes it does so by value.  When the
                    // binding's solved type is `Decoder T`, wrap the value in a
                    // zero-arg lambda so the decoder is rebuilt on each use, and
                    // rewrite every `Var(name)` read in the body to
                    // `Apply(Var(name), [])` (emitted as `(d)()`).
                    if let Some(dec_ty) = self.decoder_ir_type(b.body.span) {
                        // T3 (#121): apply capture-clone rewrite to the thunk
                        // body before wrapping. The thunk has zero params so
                        // all free VarLocals in b.body are outer captures.
                        // CloneOk captures must `.clone()` to keep the thunk
                        // `Fn`; NonClone captures in callee position are fine.
                        let thunk_body = {
                            let captures = self.captured_locals(&[], &b.body);
                            let mut clone_set: BTreeSet<Symbol> = BTreeSet::new();
                            let mut noncl_set: BTreeSet<Symbol> = BTreeSet::new();
                            for (sym, ir_ty) in captures {
                                match ir_ty.as_ref().map(clone_class) {
                                    Some(CloneClass::CloneOk) => {
                                        clone_set.insert(sym);
                                    }
                                    Some(CloneClass::NonClone) => {
                                        noncl_set.insert(sym);
                                    }
                                    Some(CloneClass::CopyLeaf) | None => {}
                                }
                            }
                            rewrite_captured_clones(&clone_set, &noncl_set, b.body.span, value, 0)?
                        };
                        let thunk = Expr::Lambda {
                            params: vec![],
                            ret: dec_ty,
                            body: Box::new(thunk_body),
                        };
                        let thunked_body = rewrite_var_to_apply(*name, acc);
                        Expr::Let {
                            name: *name,
                            value: Box::new(thunk),
                            body: Box::new(thunked_body),
                        }
                    } else {
                        // T5 (#104 / #112): multi-use-clone rewrite for CloneOk
                        // let-bindings.  When the bound value is of `CloneOk` type
                        // (e.g. String) and the body references it more than once,
                        // rewrite all but the last occurrence to `CloneVar(name)`.
                        // This prevents E0382 (use of moved value) in emitted Rust
                        // where each `Var(name)` lowers to a bare identifier that
                        // moves the value.
                        let acc = {
                            // batch-xm rekeyed `types.regions` to `(home, span)`;
                            // `region_ty` builds the composite key from current_home.
                            let ty_opt = self
                                .region_ty(b.body.span)
                                .and_then(|ty| self.ir_type_from_ty(ty, b.body.span).ok());
                            if let Some(ref ir_ty) = ty_opt {
                                if matches!(clone_class(ir_ty), CloneClass::CloneOk) {
                                    let n = count_var_uses(*name, &acc);
                                    if n > 1 {
                                        let mut remaining = n;
                                        rewrite_multiuse_clones(*name, &mut remaining, acc)
                                    } else {
                                        acc
                                    }
                                } else {
                                    // T4 (#90): a fn-carrying, non-Clone
                                    // let-binding has no sound multi-use
                                    // rewrite — fail closed on reuse instead.
                                    reject_fn_value_reuse(*name, ir_ty, &acc, b.body.span)?;
                                    acc
                                }
                            } else {
                                acc
                            }
                        };
                        Expr::Let {
                            name: *name,
                            value: Box::new(value),
                            body: Box::new(acc),
                        }
                    }
                }
                // F1 (auto-force): `let _ = <task>` — if the discarded value is
                // Task-typed, sequence it so the future is awaited rather than
                // silently dropped. Non-Task wildcards keep the plain
                // `Destructure(Wildcard, …)` form (which lowers to `let _ = …;`).
                //
                // Context matters:
                //   • Async function (returns Task): emit `TaskSeq` which lowers
                //     to `task_and_then(effect, |_| rest)` — the whole chain is a
                //     Task value.
                //   • Sync function (non-Task return): emit `TaskSeqSync` which
                //     lowers to `{ let _ = task_run(effect); rest }` — blocks on
                //     the task and discards the result, then continues with rest
                //     in a non-Task context.  This avoids E0308 (type mismatch:
                //     expected Vec<...> / () / Db, found SkyTask<...>).
                canon::Pattern_::PAnything => {
                    if self.is_task_typed(b.body.span) {
                        if self.fn_is_async.get() {
                            Expr::TaskSeq {
                                effect: Box::new(value),
                                rest: Box::new(acc),
                            }
                        } else {
                            Expr::TaskSeqSync {
                                effect: Box::new(value),
                                rest: Box::new(acc),
                            }
                        }
                    } else {
                        Expr::Destructure {
                            binder: self.lower_binder_pat(&b.pat, &b.body)?,
                            value: Box::new(value),
                            body: Box::new(acc),
                        }
                    }
                }
                // #125: a destructure binder (tuple / record / alias) whose
                // bound value's type contains a Decoder anywhere gets the
                // whole-destructure thunk treatment; every other shape falls
                // through to the plain (byte-identical) `Destructure`.
                _ => {
                    let binder = self.lower_binder_pat(&b.pat, &b.body)?;
                    self.build_destructure_or_decoder_thunk(
                        binder,
                        value,
                        b.body.span,
                        acc,
                        &b.body,
                    )?
                }
            };
        }
        Ok(acc)
    }

    // The per-arm loop (T5 clone insertion + #158 C2 nested-cons desugaring)
    // plus the destructure / tuple / enum-cover dispatch pushes this past the
    // 100-line ceiling; splitting on an arbitrary boundary would obscure the
    // single linear lowering flow. The allow is narrow: only this function.
    #[allow(clippy::too_many_lines)]
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
        // a destructure even under one or more `as` aliases.
        if Self::is_destructure_head(&first.pat.value) {
            if branches.len() == 1 {
                // #125: same Decoder-thunk gate as `lower_let`'s destructure
                // catch-all — `case buildPair () of (d1, d2) -> …` reusing a
                // Decoder-typed component is the identical E0382 gap.
                let binder = self.lower_binder_pat(&first.pat, scrut)?;
                let body = self.lower_expr(&first.body)?;
                return self.build_destructure_or_decoder_thunk(
                    binder, scrutinee, scrut.span, body, scrut,
                );
            }
            // A MULTI-arm product `case`. A tuple scrutinee whose arms are tuple
            // heads plus (optionally) a `_` catch-all lowers to a native Rust
            // tuple `match` — Rust's own pattern-match compiler resolves the
            // product discrimination, so no bespoke exhaustiveness lowering is
            // needed (SKY-T0010 already proved coverage before lowering). The
            // supported-shape and per-column soundness gates are in
            // `tuple_case_supported`; when they pass, control falls through to the
            // general flat-`match` path below, which lowers each tuple arm head
            // via `lower_arm_pat` and routes through `Match::new_flat`. Every
            // other product shape (a record head, a non-literal-tuple scrutinee,
            // a whole-value catch-all binder) stays the tuple-pattern gap.
            if !self.tuple_case_supported(scrut, branches)? {
                return Err(unsupported(first.pat.span, Feature::TuplePatternMatch));
            }
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
        // Redundancy demotion (batch-xm): SKY-T0011 is a WARNING, so arms AFTER
        // an irrefutable catch-all can now reach lowering. They are provably
        // unreachable — the exhaustiveness pass already warned — so DROP them
        // here (semantics-preserving; the Go reference compiles the same
        // shape). Without the truncation `Match::new_flat`'s structural
        // backstop sees a non-trailing catch-all and raises a CompilerBug.
        //
        // #136 seal fix: use the canonical `is_irrefutable` predicate, not a
        // hand-rolled `PAnything | PVar` match — the hand-rolled form missed
        // `PAlias` over an irrefutable inner (`_ as w` / `v as w`), which the
        // exhaustiveness pass treats as a catch-all, so post-alias arms
        // survived to `Match::new_flat` and ICE'd on well-typed source.
        let live_end = branches
            .iter()
            .position(|br| br.pat.value.is_irrefutable())
            .map_or(branches.len(), |i| i + 1);
        // `live_end <= branches.len()` by construction; `get` keeps the
        // no-panic lint satisfied with the full slice as the impossible-miss
        // fallback.
        let branches = branches.get(..live_end).unwrap_or(branches);

        let all_ctor = branches
            .iter()
            .all(|br| matches!(br.pat.value, canon::Pattern_::PCtor { .. }));

        let arms = branches
            .iter()
            .map(|br| {
                // #158 C2: a ctor arm head nesting a supportable list / cons
                // sub-pattern (`Just (h :: t)`) OR a string-literal sub-pattern
                // (`Just "live"`) desugars to a fresh binder in that arg slot PLUS
                // an arm guard PLUS (for the list case) body-prelude bindings.
                // `None` → the ordinary `lower_arm_pat` path (which keeps a
                // still-unsupported nested list fail-closed SKY-L0116).
                let (arm_pat, arm_guard, nested_bindings) =
                    match self.desugar_ctor_nested_special_args(&br.pat)? {
                        Some((pat, guard, bindings)) => (pat, guard, bindings),
                        None => (self.lower_arm_pat(&br.pat)?, None, Vec::new()),
                    };
                let mut arm_body = self.lower_expr(&br.body)?;

                // T5 for arm-bound variables.  Each symbol the canon pattern
                // introduces is owned in the arm body (after the backend's
                // `rebind_clone` / `rebind_to_vec` prologue, or after a PCtor
                // destructure).  When a symbol is used more than once, Rust's
                // move semantics would reject the second use (E0382).  Insert
                // `.clone()` for all but the syntactically-last occurrence,
                // exactly as T5 does for function parameters and let-bindings.
                //
                // Type source: the solver records a type for EVERY `VarLocal`
                // use-site.  The first use-site of `sym` in the canon arm body
                // carries the HM type that `ir_type_from_ty` then maps to an
                // IR type.  We skip symbols where the type is unavailable or
                // does not need cloning (`CopyLeaf` = Rust `Copy` scalars,
                // `NonClone` = function-typed / Cmd / Sub — these should not
                // appear in arm patterns in practice).
                for sym in collect_arm_pat_pvars(&br.pat.value) {
                    let n = count_var_uses(sym, &arm_body);
                    // `ir_type_from_ty` can legitimately fail for
                    // unsupported / not-yet-modelled types — treat
                    // any error as "skip T5 for this symbol".
                    if n > 1
                        && let Some(span) =
                            find_first_varlocal_span(sym, &br.body)
                        && let Some(ty) = self.region_ty(span)
                        && let Ok(ir_ty) = self.ir_type_from_ty(ty, span)
                    {
                        match clone_class(&ir_ty) {
                            CloneClass::CloneOk => {
                                let mut remaining = n;
                                arm_body = rewrite_multiuse_clones(
                                    sym,
                                    &mut remaining,
                                    arm_body,
                                );
                            }
                            // T4 (#90): a fn-carrying, non-Clone arm-bound
                            // variable (`case Just f of Just f -> …`) has no
                            // sound multi-use rewrite — fail closed on reuse.
                            // `count_var_uses`'s `n` over-counts a direct-call
                            // position (`f x` borrows, never moves), so
                            // `reject_fn_value_reuse` recomputes the precise
                            // consuming-use count rather than trusting `n`.
                            CloneClass::NonClone if ir_contains_fun(&ir_ty) => {
                                reject_fn_value_reuse(sym, &ir_ty, &arm_body, span)?;
                            }
                            CloneClass::NonClone | CloneClass::CopyLeaf => {}
                        }
                    }
                }

                // #158 C2: prepend the nested-list head / tail bindings as a
                // right-nested `Expr::Let` chain (built after T5 so the
                // multi-use clones in the body are already inserted). Head
                // element clones BORROW the fresh `Vec`; the tail `List.drop`
                // MOVES it — the ordered chain keeps ownership sound. Empty for
                // every non-C2 arm (byte-identical to the prior shape).
                for (sym, value) in nested_bindings.into_iter().rev() {
                    arm_body = Expr::Let {
                        name: sym,
                        value: Box::new(value),
                        body: Box::new(arm_body),
                    };
                }

                Ok(Arm {
                    pat: arm_pat,
                    body: arm_body,
                    guard: arm_guard,
                })
            })
            .collect::<DResult<Vec<_>>>()?;

        // #99 (SKY-L0128): reject dispatch-needing `as`-aliases in by-value
        // match positions before they reach the backend.
        Self::gate_by_value_dispatch_needing_aliases(&arms, branches)?;

        // #158 C2: a guarded arm (the nested-cons desugaring) is REFUTABLE to
        // rustc — its guard may fall through — so the arm set is only Rust-
        // exhaustive when a trailing irrefutable catch-all follows. Every
        // reachable Sky program of the repro shape (`Just (h::t) -> … ; _ -> …`)
        // already has one (its own SKY-T0010 exhaustiveness check requires
        // covering `Just []`). Without a trailing catch-all the emitted `match`
        // would be a rustc non-exhaustive error, so keep that residual shape
        // (e.g. `Just (h::t)` + `Just []` + `Nothing`, exhaustive at Sky level
        // but guard-non-exhaustive at Rust level) fail-closed with its existing
        // clean diagnostic rather than an accept-then-cargo-fail.
        let has_guarded_arm = arms.iter().any(|a| a.guard.is_some());
        if has_guarded_arm && !arms.last().is_some_and(|a| is_irrefutable(&a.pat)) {
            return Err(unsupported(first.pat.span, Feature::NestedCtorDiscrimination));
        }

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

    /// #99: does this BY-VALUE (whole-scrutinee, non-str/non-list) arm
    /// pattern contain an `as`-alias whose inner shape needs Rust-level
    /// runtime dispatch anywhere? Such an alias cannot be honored soundly by
    /// value: `name @ inner` double-moves a non-`Copy` payload (E0382), and
    /// the clone-rebuild repair (`render_arm_pat_alias_safe`, the #96/#99
    /// strategy) is only sound for a dispatch-FREE inner — a failing inner
    /// check may have discarded data the alias binder needs. Walks every
    /// nested position (ctor payloads, tuple elements, record fields) since
    /// by-value binding modes propagate all the way down. A dispatch-free
    /// alias inner cannot itself contain a dispatch-needing alias
    /// ([`is_dispatch_free`] recurses through `Alias`), so the alias arm
    /// needs no further recursion on the true branch.
    fn arm_has_dispatch_needing_alias(pat: &Pat) -> bool {
        match pat {
            Pat::Alias(inner, _) => !is_dispatch_free(inner),
            Pat::Tuple(elems) => elems.iter().any(Self::arm_has_dispatch_needing_alias),
            Pat::Record(fields) => fields
                .iter()
                .any(|(_, p)| Self::arm_has_dispatch_needing_alias(p)),
            Pat::Ctor { args, .. } => args.iter().any(Self::arm_has_dispatch_needing_alias),
            Pat::Slice { prefix, rest } => {
                prefix.iter().any(Self::arm_has_dispatch_needing_alias)
                    || rest
                        .as_deref()
                        .is_some_and(Self::arm_has_dispatch_needing_alias)
            }
            Pat::Var(_)
            | Pat::Wildcard
            | Pat::Int(_)
            | Pat::Bool(_)
            | Pat::Char(_)
            | Pat::Str(_) => false,
        }
    }

    /// #99 fail-closed gate (SKY-L0128). Mirrors the backend's per-match mode
    /// decision (`emit_match_scrutinee` / `tuple_col_modes`) EXACTLY: STR and
    /// LIST modes match the scrutinee by REFERENCE (`.as_str()` /
    /// `.as_slice()`), where Rust's default binding modes make `name @ inner`
    /// a borrow — sound for any inner, no gate. Only the by-VALUE positions
    /// (WHOLE mode with neither flag, and non-str/non-list tuple columns) are
    /// gated: a dispatch-needing alias inner there is rejected at lowering
    /// with a clean diagnostic rather than reaching the backend, where it
    /// would either double-move (the pre-#99 E0382 seal hole) or require a
    /// by-reference arm redesign. Dispatch-free aliases pass through — the
    /// backend's `render_arm_pat_alias_safe` repairs those.
    fn gate_by_value_dispatch_needing_aliases(
        arms: &[Arm],
        branches: &[canon::CaseBranch],
    ) -> DResult<()> {
        // Tuple mode: per-column str/list flags (bare `Str` / `Slice` sub —
        // the same predicate `tuple_col_modes` uses).
        if let Some(arity) = arms.iter().find_map(|a| match &a.pat {
            Pat::Tuple(elems) => Some(elems.len()),
            _ => None,
        }) {
            let mut col_by_ref = vec![false; arity];
            for arm in arms {
                if let Pat::Tuple(elems) = &arm.pat {
                    for (c, sub) in elems.iter().enumerate() {
                        if matches!(sub, Pat::Str(_) | Pat::Slice { .. })
                            && let Some(slot) = col_by_ref.get_mut(c)
                        {
                            *slot = true;
                        }
                    }
                }
            }
            for (arm, br) in arms.iter().zip(branches.iter()) {
                if let Pat::Tuple(elems) = &arm.pat {
                    for (c, sub) in elems.iter().enumerate() {
                        if !col_by_ref.get(c).copied().unwrap_or(false)
                            && Self::arm_has_dispatch_needing_alias(sub)
                        {
                            return Err(unsupported(
                                br.pat.span,
                                Feature::AliasOverRefutablePayload,
                            ));
                        }
                    }
                }
            }
            return Ok(());
        }
        // Whole mode: by-ref iff some arm's TOP pattern is a bare `Str` /
        // `Slice` (the same predicate `emit_match_scrutinee` uses).
        let by_ref = arms
            .iter()
            .any(|a| matches!(a.pat, Pat::Str(_) | Pat::Slice { .. }));
        if by_ref {
            return Ok(());
        }
        for (arm, br) in arms.iter().zip(branches.iter()) {
            if Self::arm_has_dispatch_needing_alias(&arm.pat) {
                return Err(unsupported(
                    br.pat.span,
                    Feature::AliasOverRefutablePayload,
                ));
            }
        }
        Ok(())
    }

    /// Lower a `case`-arm HEAD pattern to its IR [`Pat`]. Handles the full M3b-3
    /// refutable head set — variable / wildcard binders, the literal leaves
    /// (`0` / `True` / `'a'` / `"hi"`), an alias / `as` binder, and a
    /// constructor pattern (whose payload sub-patterns recurse through
    /// [`Self::lower_payload_pat`]). A tuple / record head is the destructure
    /// path (handled by the single-arm branch of [`Self::lower_case`]); reaching
    /// it here is a multi-arm product `case`, the tuple-pattern gap (SKY-L0115).
    fn lower_arm_pat(&self, p: &canon::Pattern) -> DResult<Pat> {
        match &p.value {
            canon::Pattern_::PVar(s) => Ok(Pat::Var(*s)),
            canon::Pattern_::PAnything => Ok(Pat::Wildcard),
            canon::Pattern_::PInt(n) => Ok(Pat::Int(*n)),
            canon::Pattern_::PBool(b) => Ok(Pat::Bool(*b)),
            canon::Pattern_::PChar(c) => Ok(Pat::Char(c.clone())),
            canon::Pattern_::PStr(s) => Ok(Pat::Str(s.clone())),
            canon::Pattern_::PAlias(inner, name) => Ok(Pat::Alias(
                Box::new(self.lower_arm_pat(inner)?),
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
                    .map(|a| self.lower_payload_pat(a))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Pat::Ctor {
                    home: ModPath(home.clone()),
                    ty: *type_name,
                    variant: *name,
                    args: sub,
                })
            }
            // A tuple case-arm head lowers element-by-element to [`Pat::Tuple`], so
            // a multi-arm product `case` (`case (xs, ys) of (a :: as, b :: bs) ->
            // … ; _ -> …`) becomes a native Rust tuple `match`. Each element
            // recurses through this same refutable arm lowerer, so a column may be
            // a variable / wildcard / literal / constructor / cons / nested tuple.
            // The scrutinee-shape and per-column soundness gates live in
            // [`Self::tuple_case_supported`]; reaching here means those passed.
            canon::Pattern_::PTuple(elems) => {
                let subs = elems
                    .iter()
                    .map(|e| self.lower_arm_pat(e))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Pat::Tuple(subs))
            }
            // A record case-arm head in a multi-arm `case` still needs the
            // scrutinee's record type to recover the complete field set; the
            // multi-arm record shape remains the tuple-pattern gap (SKY-L0115).
            canon::Pattern_::PRecord(_) => Err(unsupported(p.span, Feature::TuplePatternMatch)),
            // A list (`[a, b]`) or cons (`x :: xs`) case-arm head flattens to the
            // slice-shaped IR [`Pat::Slice`] (M4a).
            canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _) => self.lower_list_arm_pat(p),
        }
    }

    /// Class 4 item C2 (#158) — desugar a `case`-arm HEAD that nests, DIRECTLY
    /// inside a constructor payload, either:
    ///
    /// * a list / cons sub-pattern (`Just (h :: t)`, `Ok [a, b]`) into a fresh
    ///   `Vec` binder in that ctor-arg slot PLUS an arm-level length guard PLUS
    ///   a body-prelude that recovers the named head / tail bindings by index /
    ///   `List.drop`; or
    /// * a string-literal sub-pattern (`Just "live"`, `Ok "done"`) into a fresh
    ///   `String` binder PLUS an arm-level equality guard (`binder == "live"`)
    ///   — the sibling shape: Rust cannot literal-match a `&str` pattern
    ///   against an owned `String` ctor FIELD (`SkyMaybe::Just(String)`) the
    ///   way it can coerce a top-level `String` SCRUTINEE to `&str`
    ///   (`m0-24-tui-kitchen-sink` SEAL violation — `SkyMaybe::Just("live")`
    ///   is E0308, `expected String, found &str`).
    ///
    /// Both shapes share one root cause: an enum FIELD (`Vec<T>` / `String`)
    /// cannot be pattern-matched inline the way a raw scrutinee of that type
    /// can (via `.as_slice()` / `.as_str()` coercion at the `match` head), so
    /// both desugar to a fresh binder in that ctor-arg slot plus an arm guard
    /// that re-derives the refutability the bare pattern would have expressed.
    ///
    /// Returns `Ok(None)` when `p` is not a `PCtor` head OR none of its direct
    /// args is a supportable nested list / string literal (the caller then
    /// lowers the arm the ordinary way — a nested list reached via
    /// `lower_payload_pat` stays fail-closed SKY-L0116, e.g. two levels deep).
    /// Returns `Ok(Some((ir_pat, guard, bindings)))` otherwise, where
    /// `bindings` are prepended to the arm body as a right-nested `Expr::Let`
    /// chain in order (head element clones, which BORROW the fresh `Vec`,
    /// precede the tail `List.drop`, which MOVES it — so ownership is sound;
    /// the string-literal shape contributes no `bindings`, only a guard).
    ///
    /// SUPPORTED nested list shapes are exactly the two verified repros: a
    /// `PCons` chain / `PList` whose every element sub-pattern AND open tail
    /// is a plain `PVar` / `PAnything` binder. Any refutable element (a
    /// literal / ctor / deeper list) makes that argument NOT desugarable here
    /// — it falls back to `lower_payload_pat` and stays fail-closed (residual
    /// scope, documented in the spec). The string-literal shape has no such
    /// residual scope — every direct `PStr` ctor arg desugars.
    #[allow(clippy::type_complexity)]
    fn desugar_ctor_nested_special_args(
        &self,
        p: &canon::Pattern,
    ) -> DResult<Option<(Pat, Option<Expr>, Vec<(Symbol, Expr)>)>> {
        let canon::Pattern_::PCtor {
            home,
            type_name,
            name,
            args,
            ..
        } = &p.value
        else {
            return Ok(None);
        };
        // Does any direct arg nest a SUPPORTABLE list / cons sub-pattern, or a
        // string-literal sub-pattern? If not, leave the arm to the ordinary
        // `lower_arm_pat` path.
        let has_desugarable = args.iter().any(|a| {
            Self::simple_nested_list(a).is_some() || matches!(a.value, canon::Pattern_::PStr(_))
        });
        if !has_desugarable {
            return Ok(None);
        }
        let mut ir_args = Vec::with_capacity(args.len());
        let mut guards: Vec<Expr> = Vec::new();
        let mut bindings: Vec<(Symbol, Expr)> = Vec::new();
        for a in args {
            if let canon::Pattern_::PStr(lit) = &a.value {
                // A string-literal ctor payload (`Just "live"`) cannot be a
                // bare Rust literal pattern against the owned `String` field
                // (E0308: expected String, found &str). Desugar to a fresh
                // `String` binder plus an arm guard comparing it to the
                // literal — `String`'s std `PartialEq<&str>` impl makes the
                // guard's `==` valid Rust without any extra `.as_str()`.
                let fresh = self.fresh_nested_strlit_binder()?;
                ir_args.push(Pat::Var(fresh));
                guards.push(Expr::BinOp {
                    op: BinOp::Eq,
                    lhs: Box::new(Expr::Var(fresh)),
                    rhs: Box::new(Expr::Str(lit.clone())),
                });
            } else if let Some(flat) = Self::simple_nested_list(a) {
                // Binding a head element (`h`) or the tail (`t`) needs the
                // element type to be `Clone` — `ListIndexClone` clones an element
                // and `List.drop` returns an owned `Vec`. Every CONCRETE element
                // derives `Clone`; a still-generic element carries no such bound,
                // so binding one would emit Rust that fails `cargo` — the same
                // SKY-L0102 polymorphic-element gate the top-level list path
                // applies (here the list lives at the ctor sub-pattern's span,
                // whose `List T` region the constraint generator now records).
                if flat.binds_a_value()
                    && matches!(self.list_elem_ir(a.span)?, IrType::Generic(_))
                {
                    return Err(unsupported(a.span, Feature::Polymorphism));
                }
                // Replace this ctor arg with a fresh `Vec` binder and record the
                // guard + per-element prelude bindings against it.
                let fresh = self.fresh_nested_cons_binder()?;
                ir_args.push(Pat::Var(fresh));
                let prefix_len = flat.prefix.len();
                guards.push(Expr::ListLenCheck {
                    list: Box::new(Expr::Var(fresh)),
                    len: prefix_len,
                    exact: flat.closed(),
                });
                // Head-element binders BORROW `fresh` (index + clone), so they
                // precede the tail binder that MOVES it.
                for (idx, elem) in flat.prefix.iter().enumerate() {
                    if let NestedBinder::Named(sym) = elem {
                        bindings.push((
                            *sym,
                            Expr::ListIndexClone {
                                list: Box::new(Expr::Var(fresh)),
                                index: idx,
                            },
                        ));
                    }
                }
                if let NestedTail::Rest(NestedBinder::Named(rest_sym)) = flat.tail {
                    // `t = List.drop(prefix_len, fresh)` — the remaining list; a
                    // wildcard tail binds nothing and is dropped.
                    bindings.push((
                        rest_sym,
                        Expr::Call {
                            callee: Callee::Kernel(KernelFn::ListDrop),
                            args: vec![
                                Expr::Int(i64::try_from(prefix_len).unwrap_or(i64::MAX)),
                                Expr::Var(fresh),
                            ],
                        },
                    ));
                }
            } else {
                // A non-list arg (or a nested list we don't desugar) lowers the
                // ordinary way; a still-unsupported nested list stays fail-closed.
                ir_args.push(self.lower_payload_pat(a)?);
            }
        }
        let ir_pat = Pat::Ctor {
            home: ModPath(home.clone()),
            ty: *type_name,
            variant: *name,
            args: ir_args,
        };
        // Combine per-arg guards with `&&`; there is at least one (has_desugarable).
        let guard = guards.into_iter().reduce(|acc, g| Expr::BinOp {
            op: BinOp::And,
            lhs: Box::new(acc),
            rhs: Box::new(g),
        });
        Ok(Some((ir_pat, guard, bindings)))
    }

    /// Classify a constructor-arg sub-pattern as a SUPPORTABLE nested list for
    /// the #158 C2 desugaring: a `PList` / `PCons` whose every element AND open
    /// tail is a plain `PVar` / `PAnything`. Returns the flattened prefix (each
    /// entry `Some(sym)` for a named binder, `None` for a wildcard), whether it
    /// is `closed` (a `PList` literal / a cons chain ending in `[]`), and the
    /// open `rest` tail binder (`Some(Some(sym))` named, `Some(None)` wildcard,
    /// `None` when closed). Any refutable element makes it NOT desugarable →
    /// `None` (the caller keeps it fail-closed).
    fn simple_nested_list(a: &canon::Pattern) -> Option<FlatNestedList> {
        // Only a directly-nested list / cons is in this item's scope.
        if !matches!(
            a.value,
            canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _)
        ) {
            return None;
        }
        let mut prefix: Vec<NestedBinder> = Vec::new();
        let mut cur = a;
        loop {
            match &cur.value {
                canon::Pattern_::PList(elems) => {
                    // A closed list literal: every element must be a simple binder.
                    for e in elems {
                        prefix.push(nested_simple_binder(e)?);
                    }
                    return Some(FlatNestedList {
                        prefix,
                        tail: NestedTail::Closed,
                    });
                }
                canon::Pattern_::PCons(head, tail) => {
                    prefix.push(nested_simple_binder(head)?);
                    match &tail.value {
                        canon::Pattern_::PCons(_, _) | canon::Pattern_::PList(_) => cur = tail,
                        // A variable / wildcard tail is the open rest binder; a
                        // refutable / aliased tail is out of scope (not desugarable).
                        _ => {
                            let rest = nested_simple_binder(tail)?;
                            return Some(FlatNestedList {
                                prefix,
                                tail: NestedTail::Rest(rest),
                            });
                        }
                    }
                }
                _ => return None,
            }
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
    fn lower_list_arm_pat(&self, p: &canon::Pattern) -> DResult<Pat> {
        let mut prefix = Vec::new();
        let mut cur = p;
        loop {
            match &cur.value {
                // A closed list literal terminates the prefix with no open tail.
                canon::Pattern_::PList(elems) => {
                    for e in elems {
                        prefix.push(self.lower_payload_pat(e)?);
                    }
                    return Ok(Pat::Slice { prefix, rest: None });
                }
                canon::Pattern_::PCons(head, tail) => {
                    prefix.push(self.lower_payload_pat(head)?);
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

/// Recursively collect every [`IrType::Generic`] symbol that appears
/// structurally in `ty`.
///
/// Used by [`lower_def`]'s `Def::Typed` arm to compute the set of type
/// parameters that are actually referenced in the resolved `params` and `ret`
/// of a [`Func`] — the principled definition of [`Func::type_params`].
///
/// This fixes Bug-28 (`init : any -> (Model, Cmd Msg)`): `any` in PARAM
/// position leaves `IrType::Generic(any_sym)` in `params`, so `any_sym`
/// appears in `used_generics` and therefore in `type_params`.  The old blind
/// filter (`resolve(v) != "any"`) over-removed `any_sym` even when it was
/// structurally necessary.
///
/// See the Bug-28 / Bug-29 fix comments in [`lower_def`] for full motivation.
fn collect_ir_generic_syms(ty: &IrType, out: &mut BTreeSet<Symbol>) {
    match ty {
        IrType::Generic(sym) => {
            out.insert(*sym);
        }
        IrType::Task(inner)
        | IrType::Maybe(inner)
        | IrType::List(inner)
        | IrType::Set(inner)
        | IrType::Cmd(inner)
        | IrType::Sub(inner)
        | IrType::Decoder(inner)
        | IrType::LiveRoute(inner) => {
            collect_ir_generic_syms(inner, out);
        }
        IrType::Result(a, b) | IrType::Dict(a, b) => {
            collect_ir_generic_syms(a, out);
            collect_ir_generic_syms(b, out);
        }
        IrType::Enum { args, .. } => {
            for a in args {
                collect_ir_generic_syms(a, out);
            }
        }
        IrType::Tuple(elems) => {
            for e in elems {
                collect_ir_generic_syms(e, out);
            }
        }
        IrType::Record(fields) => {
            for v in fields.values() {
                collect_ir_generic_syms(v, out);
            }
        }
        IrType::Fun(params, ret) => {
            for p in params {
                collect_ir_generic_syms(p, out);
            }
            collect_ir_generic_syms(ret, out);
        }
        IrType::Ui { msg, .. } => {
            collect_ir_generic_syms(msg, out);
        }
        // Leaf types — carry no nested IrType.
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        | IrType::Db
        | IrType::Order
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        // Nominal error-payload leaves (SEAL fix 2026-07-11) — monomorphic,
        // no generics to collect.
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        | IrType::StreamWriter
        | IrType::HttpRequest
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        | IrType::UiPlain(_)
        | IrType::Decimal
        | IrType::LiveReq
        | IrType::SqlFragment
        | IrType::Secret => {}
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

    use super::{BuiltinCtors, Lowerer, SymbolPools};

    /// Intern every constructor / ADT-payload name [`Lowerer::new`] needs to
    /// seed `enum_variants` (`Maybe`/`Result`/`SqlValue`/`SqlField`/`Order`/
    /// `Error`/`ErrorKind` and their payload variants), and return the
    /// resulting [`BuiltinCtors`].
    ///
    /// Shared by every test in this module that needs a minimal-but-valid
    /// [`Lowerer`] — [`decl_equiv_legacy_match`] and
    /// [`callee_arity_matches_decl_arity`] both call this rather than
    /// hand-rolling their own copy of the ~35-symbol interning block (see
    /// `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md`
    /// Item 3, Step 1: "Reuse the exact `BuiltinCtors` construction ...
    /// verbatim — do not hand-roll a second copy").
    fn build_test_builtin_ctors(interner: &mut Interner) -> BuiltinCtors {
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
        // ── Order ADT (#123) ─────────────────────────────────────────────────
        let order = interner.intern("Order").unwrap();
        let lt = interner.intern("LT").unwrap();
        let eq = interner.intern("EQ").unwrap();
        let gt = interner.intern("GT").unwrap();
        // ── Error / ErrorKind ADTs (E-12, #152) ─────────────────────────────
        let error = interner.intern("Error").unwrap();
        let errorkind = interner.intern("ErrorKind").unwrap();
        let ek_io = interner.intern("Io").unwrap();
        let ek_network = interner.intern("Network").unwrap();
        let ek_ffi = interner.intern("Ffi").unwrap();
        let ek_decode = interner.intern("Decode").unwrap();
        let ek_timeout = interner.intern("Timeout").unwrap();
        let ek_not_found = interner.intern("NotFound").unwrap();
        let ek_permission_denied = interner.intern("PermissionDenied").unwrap();
        let ek_invalid_input = interner.intern("InvalidInput").unwrap();
        let ek_conflict = interner.intern("Conflict").unwrap();
        let ek_unavailable = interner.intern("Unavailable").unwrap();
        let ek_unexpected = interner.intern("Unexpected").unwrap();
        // ── ErrorDetails ADT (backlog #85 follow-up) ─────────────────────────
        let errordetails = interner.intern("ErrorDetails").unwrap();
        let ed_ffi_panic = interner.intern("FfiPanic").unwrap();
        let ed_type_mismatch = interner.intern("TypeMismatch").unwrap();
        let ed_http_status = interner.intern("HttpStatus").unwrap();
        let ed_json_decode = interner.intern("JsonDecode").unwrap();
        let ed_custom = interner.intern("Custom").unwrap();

        BuiltinCtors {
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
            // ── Order ADT (#123) ─────────────────────────────────────────────
            order,
            lt,
            eq,
            gt,
            // ── Error / ErrorKind (E-12, #152) ───────────────────────────────
            error,
            errorkind,
            ek_io,
            ek_network,
            ek_ffi,
            ek_decode,
            ek_timeout,
            ek_not_found,
            ek_permission_denied,
            ek_invalid_input,
            ek_conflict,
            ek_unavailable,
            ek_unexpected,
            // ── ErrorDetails (backlog #85 follow-up) ─────────────────────────
            errordetails,
            ed_ffi_panic,
            ed_type_mismatch,
            ed_http_status,
            ed_json_decode,
            ed_custom,
        }
    }

    /// A minimal, empty [`SolvedTypes`] — every field this module's tests
    /// need is populated at [`Lowerer`] construction time from `module`, not
    /// from `types`; the tests below only exercise paths that don't consult
    /// solved region/env types.
    fn empty_solved_types() -> SolvedTypes {
        SolvedTypes {
            env: BTreeMap::new(),
            regions: BTreeMap::new(),
            bounds: BTreeMap::new(),
            warnings: Vec::new(),
            poly_var_map: BTreeMap::new(),
            untyped_type_params: BTreeMap::new(),
        }
    }

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
        let builtins = build_test_builtin_ctors(&mut interner);

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

        let module = canon::Module {
            name: vec![],
            unions: vec![],
            defs: vec![],
        };
        let types = empty_solved_types();

        // Immutable borrow of interner starts here — no more intern() calls.
        let lowerer = Lowerer::new(
            &module,
            &types,
            &interner,
            SymbolPools {
                eta_params: vec![],
                cap_params: vec![],
                param_binders: vec![],
                any_param_binders: vec![],
                destructure_thunk_binders: vec![],
                nested_cons_binders: vec![],
                nested_strlit_binders: vec![],
            },
            &builtins,
        );

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

    /// #70 — `callee_arity`'s hand-written per-variant arity buckets must
    /// agree with `StdlibKernel::decl().arity` (the same enum, aliased as
    /// `KernelFn`).
    ///
    /// `constrain_var_kernel` (`sky_types::constrain`) types a call against
    /// `stdlib_scheme`'s arrow count, while `callee_arity` independently
    /// governs how many arguments the IR actually saturates / eta-expands
    /// against at lowering time (its call sites decide eta-expansion,
    /// argument saturation, and TEA default-arg elision). Rust's
    /// exhaustiveness checker guarantees `callee_arity`'s match covers every
    /// `KernelFn` variant (a *missing* arm is a compile error), but nothing
    /// previously caught a *wrong* arity value inside one of the buckets —
    /// that silent drift is the exit-0-then-cargo-fail class `#70` names: a
    /// program can pass `skyc`'s type-check against `decl().arity`'s arrow
    /// count and still emit a Rust call with the wrong argument count,
    /// caught only by `cargo`, never by `skyc`.
    ///
    /// This test is the mechanical cross-check: for every `KernelFn::ALL`
    /// variant, `callee_arity(Callee::Kernel(sk))` must equal
    /// `sk.decl().arity`. A future kernel addition (or a copy-paste slot
    /// into the wrong arity bucket) that gets this wrong now fails
    /// `cargo nextest run --workspace` immediately instead of shipping a
    /// latent bug.
    #[test]
    fn callee_arity_matches_decl_arity() {
        let mut interner = Interner::new();
        let builtins = build_test_builtin_ctors(&mut interner);
        let module = canon::Module {
            name: vec![],
            unions: vec![],
            defs: vec![],
        };
        let types = empty_solved_types();

        // `callee_arity`'s `Callee::Kernel` arm reads only the match subject
        // (`self.interner`/`self.m` are consulted solely by the
        // `Callee::Func` arm) — a minimal Lowerer built the same way
        // `decl_equiv_legacy_match` builds one is a faithful fixture.
        let lowerer = Lowerer::new(
            &module,
            &types,
            &interner,
            SymbolPools {
                eta_params: vec![],
                cap_params: vec![],
                param_binders: vec![],
                any_param_binders: vec![],
                destructure_thunk_binders: vec![],
                nested_cons_binders: vec![],
                nested_strlit_binders: vec![],
            },
            &builtins,
        );

        let mut mismatches = Vec::new();
        for &sk in KernelFn::ALL {
            let decl_arity = usize::from(sk.decl().arity);
            match lowerer.callee_arity(&Callee::Kernel(sk)) {
                Ok(computed) if computed == decl_arity => {}
                Ok(computed) => mismatches.push(format!(
                    "{sk:?}: decl().arity={decl_arity} but callee_arity={computed}"
                )),
                Err(e) => mismatches.push(format!("{sk:?}: callee_arity() errored: {e:?}")),
            }
        }
        assert!(
            mismatches.is_empty(),
            "decl().arity / callee_arity drift found ({} entries):\n{}",
            mismatches.len(),
            mismatches.join("\n"),
        );
    }
}

