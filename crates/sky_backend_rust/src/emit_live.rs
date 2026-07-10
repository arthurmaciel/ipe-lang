//! Emission for `Std.Live` / `Sky.Live` app-entry kernels (Phase-1b).
//!
//! Wires three of the four Live kernels:
//!
//! * [`KernelFn::LiveApp`] — `Live.app cfg` → `sky_runtime::live::live_app(…)`
//!   for single-page apps, or `live_app_routed(…)` when the Model carries a
//!   `page` field (#108 T5 emit branch — the six-field cfg scheme with
//!   `routes` / `notFound`).
//! * [`KernelFn::LiveRoute`] — `Live.route pattern ctor` →
//!   `sky_runtime::live::route::Route::new(…)`.
//! * [`KernelFn::LiveRenderStatic`] — `Live.renderStatic view model` →
//!   `sky_runtime::live::live_render_static(…)`.
//!
//! # Correctness constraints (MAKE INVALID STATES UNREPRESENTABLE)
//!
//! * Required cfg fields are looked up with `lookup_field` (fail-closed on miss).
//! * Route params are accessed via `.get(i).cloned().unwrap_or_default()` — never
//!   by index (panic vector eliminated).
//! * Store kind / path are read from process env at call time, not compiled in.
//! * `Live.appRouted` is a vestigial alias routed through the same
//!   `lower_app_entry_cfg` path as `Live.app` (#108 T4); its arm here is a
//!   defensive invariant check.

use sky_diagnostics::{DResult, Diagnostic, LowerError, Span};
use sky_ir::{Callee, Expr, IrType, KernelFn};

use crate::EmitCtx;
use crate::emit_expr::emit_expr_at;
use crate::emit_types::{render_type, GenericScope};

/// Dispatch a `Std.Live` / `Sky.Live` kernel call.
///
/// Returns `Some(emitted)` for all four Live kernels; `None` for any variant
/// that is not a Live kernel (the caller routes to this function only for
/// `k.is_live()` variants, but a defensive `None` for unknown variants avoids
/// a catchall `_` arm).
///
/// Called from `emit_ui_call` after the Phase-0 stubs were removed.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn emit_live_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };

    match k {
        // ── Live.app { init, update, view, subscriptions, routes, notFound } ──
        //
        // The six-field cfg scheme (#108 T3). `emit_live_app_inner` branches
        // on the Model's `page` field: routed apps take `live_app_routed`
        // (routes + notFound + set_page); single-page apps take `live_app`.
        KernelFn::LiveApp => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_live_call::LiveApp",
                    detail: format!("Live.app requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with SKY-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `LiveAppRouted` precedent.
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_live_call::LiveApp",
                    detail: "Live.app cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with SKY-L0119"
                        .into(),
                });
            };
            emit_live_app_inner(ctx, fields, indent, child, generics)
        }

        // ── Live.appRouted — vestigial alias of `Live.app` (#108 T4) ───────
        //
        // The lower stage routes `Live.appRouted` through the same
        // `lower_app_entry_cfg` path as `Live.app` (the reference has ONE
        // `Live.app` that branches at emit time), so the alias takes the same
        // `emit_live_app_inner` branch here.  A non-literal cfg is rejected at
        // lower with SKY-L0119 exactly as for `Live.app`; the guard below is
        // the same defensive invariant.
        KernelFn::LiveAppRouted => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_live_call::LiveAppRouted",
                    detail: format!("Live.appRouted requires 1 argument, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_live_call::LiveAppRouted",
                    detail: "Live.appRouted cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with SKY-L0119"
                        .into(),
                });
            };
            emit_live_app_inner(ctx, fields, indent, child, generics)
        }

        // ── Live.route pattern ctor ─────────────────────────────────────────
        KernelFn::LiveRoute => emit_live_route(ctx, args, indent, child, generics),

        // ── Live.renderStatic view model ────────────────────────────────────
        //
        // `Live.renderStatic : (Model -> Html Msg) -> Model -> Task Error ()`
        //
        // Emits: `sky_runtime::live::live_render_static(view, model)`
        KernelFn::LiveRenderStatic => {
            let [view_e, model_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_live_call::LiveRenderStatic",
                    detail: format!("Live.renderStatic requires 2 arguments, got {}", args.len()),
                });
            };
            let view_s = emit_expr_at(ctx, view_e, indent, child, generics)?;
            let model_s = emit_expr_at(ctx, model_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::live::live_render_static({view_s}, {model_s})"
            )))
        }

        // Any non-Live kernel variant: let the standard path handle it.
        _ => Ok(None),
    }
}

// ── `Live.route` ──────────────────────────────────────────────────────────────

/// Emit `Live.route pattern builder` → `Route::new(&pattern, closure)`.
///
/// `Live.route : String -> builder -> LiveRoute page` (#106 / #108 round 4).
/// The builder argument is one of:
///
/// * A page-constructor reference ([`Expr::Ctor`], nullary or partial — the
///   lower-tier `lower_route_builder` peephole carries a bare payload ctor
///   through as a zero-arg `Ctor`).  A nullary/fully-applied ctor wraps as a
///   constant closure; a partial ctor emits one type-directed `params.get(i)`
///   conversion per declared payload field (T6, [`route_param_get`]):
///
///   | field | conversion |
///   |-------|------------|
///   | `String` | `.cloned().unwrap_or_default()` |
///   | `Int`    | `.and_then(\|s\| s.parse::<i64>().ok()).unwrap_or_default()` |
///   | `Float`  | `.and_then(\|s\| s.parse::<f64>().ok()).unwrap_or_default()` |
///   | `Bool`   | `.map(\|s\| s == "true").unwrap_or_default()` |
///   | other    | compile-time error (unsupported payload type) |
///
/// * A named function or inline lambda ([`builder_fn_params`]) — same
///   per-parameter conversion table; a single `List String` parameter is the
///   raw-params builder shape and receives the whole vec.
///
/// * Anything else — fail CLOSED (see the final arm).
///
/// Parity note: the reference backend assumes all payloads are String
/// (`ExprEmitter.hs:1823`). The type-directed path is a sanctioned divergence —
/// strictly safer (catches Int/Float/Bool mismatches at compile time instead
/// of emitting E0308). See `docs/divergences-from-sky.md` §B-route-param.
fn emit_live_route(
    ctx: &EmitCtx,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let [pattern_e, builder_e] = args else {
        return Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_live_call::LiveRoute",
            detail: format!("Live.route requires 2 arguments, got {}", args.len()),
        });
    };
    let pattern_s = emit_expr_at(ctx, pattern_e, indent, child, generics)?;

    let build_closure = if let Expr::Ctor {
        home,
        ty,
        variant,
        args: ctor_args,
    } = builder_e
    {
        // T6: the full field-type slice (not just the count) so each slot can
        // emit a type-directed conversion.
        let variant_tys = ctx.variant_fields(home, *ty, *variant)?;
        let ctor_s = emit_expr_at(ctx, builder_e, indent, child, generics)?;
        if variant_tys.is_empty() || !ctor_args.is_empty() {
            // Nullary ctor or fully-applied ctor: hoist the page value out of
            // the closure and clone it per call (`ExprEmitter.hs:1809` parity
            // — `{ let __c = ctor; move |_p| __c.clone() }`).  Constructing it
            // inside the body would move any captured payload out of an `Fn`
            // closure (E0507); every page ADT derives `Clone`.
            format!(
                "{{ let __c = {ctor_s}; \
                 move |_params: ::std::vec::Vec<::std::string::String>| __c.clone() }}"
            )
        } else {
            // Partial-ctor with N payload fields.
            //
            // Item 1 (#120): static arity check — count ':param' segments in
            // the pattern and compare against the constructor's payload count.
            // A mismatch is a compile-time error (SKY-L0122): the route can
            // never deliver the right arguments.  Only checked when the pattern
            // is a string literal (the only shape the parser accepts for a
            // route pattern); other shapes are left to cargo for now.
            if let Expr::Str(pat_s) = pattern_e {
                let param_count = pat_s
                    .split('/')
                    .filter(|seg| seg.starts_with(':'))
                    .count();
                let ctor_payload_count = variant_tys.len();
                if param_count != ctor_payload_count {
                    return Err(Diagnostic::Lower {
                        span: Span::DUMMY,
                        msg: LowerError::RouteParamCountMismatch {
                            pattern: pat_s.as_str().into(),
                            param_count,
                            ctor_payload_count,
                        },
                    });
                }
            }
            // Emit type-directed `params.get(i)` conversions matching each
            // field's IrType.
            let mut param_gets = Vec::with_capacity(variant_tys.len());
            for (i, field_ty) in variant_tys.iter().enumerate() {
                param_gets.push(route_param_get(field_ty, i)?);
            }
            format!(
                "move |params: ::std::vec::Vec<::std::string::String>| \
                 {ctor_s}({})",
                param_gets.join(", ")
            )
        }
    } else if let Some(param_tys) = builder_fn_params(builder_e) {
        // A named function or inline lambda used as the page builder. Its
        // parameter types are the `:param` payload slots — one type-directed
        // conversion per parameter.  (Special case: a single `List String`
        // parameter is the raw-params builder shape `List String -> Page`;
        // pass the whole vec through.)
        // Hoist the builder value OUT of the route closure (`let __b = …;`):
        // constructing it inside the body would re-evaluate per call AND, for
        // a capturing lambda, move captured state out of an `Fn` closure
        // (E0507).  Calling through the binding (`(__b)(…)`) goes via `&__b`,
        // which `Box<F: Fn>` / any `Fn` value supports.
        let builder_s = emit_expr_at(ctx, builder_e, indent, child, generics)?;
        if matches!(param_tys.as_slice(),
            [IrType::List(elem)] if matches!(elem.as_ref(), IrType::Str))
        {
            format!(
                "{{ let __b = {builder_s}; \
                 move |params: ::std::vec::Vec<::std::string::String>| \
                 (__b)(params) }}"
            )
        } else {
            let mut param_gets = Vec::with_capacity(param_tys.len());
            for (i, field_ty) in param_tys.iter().enumerate() {
                param_gets.push(route_param_get(field_ty, i)?);
            }
            format!(
                "{{ let __b = {builder_s}; \
                 move |params: ::std::vec::Vec<::std::string::String>| \
                 (__b)({}) }}",
                param_gets.join(", ")
            )
        }
    } else {
        // Any other builder shape (a `Var` referencing a local, a call
        // result, …) carries no recoverable parameter types, so the builder
        // closure cannot be emitted soundly — fail CLOSED with SKY-L0123.
        // Pre-round-4 this arm emitted `(builder)(params)` untyped, which
        // cargo-failed (E0308/E0618) for every shape except the rare raw
        // `List String -> Page` builder — a silent seal hole.
        return Err(Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::RouteBuilderUnsupportedShape,
        });
    };

    // `Route::new` takes `pattern: &str`.  Sky string literals emit as
    // `"…".to_string()` (an owned `String`); prepend `&` so the `&String`
    // deref-coerces to `&str` at the call site.  Variable references also
    // type as `String`, so `&var` is equally correct.
    Ok(Some(format!(
        "sky_runtime::live::route::Route::new(&{pattern_s}, {build_closure})"
    )))
}

// ── Non-routed `live_app` ──────────────────────────────────────────────────────

/// Emit `sky_runtime::live::live_app(init, update, view, subs, store, path)`.
///
/// The `init` function is passed directly — after B1 constrain, the solver pins
/// its first parameter type to `LiveReq`, so the emitted Rust function already
/// has signature `fn(_req: LiveReq) -> (Model, SkyCmd<Msg>)`.
///
/// `update` is `Fn(Msg, Model) -> (Model, SkyCmd<Msg>)` — multi-param Sky
/// functions are lowered as uncurried Rust fns, matching the runtime bound.
///
/// Store kind and path are read from process env at call time (never compiled in)
/// so a single binary can switch stores via env without recompilation.
///
/// # Function-field emission
///
/// `live_app`'s generic parameters carry `+ Send + Sync + 'static` bounds on
/// the function arguments.  A named Rust `fn` item satisfies these bounds
/// implicitly (the compiler's blanket impl covers all `fn` pointers and
/// non-capturing function items).  By contrast, a `Box<dyn Fn(...)>` as emitted
/// by the general `emit_expr_at` / `emit_func_value` path does NOT carry these
/// bounds without explicit annotation — `Box<dyn Fn(...) + Send + Sync>` is a
/// different type from `Box<dyn Fn(...)>`.
///
/// For this reason, `emit_live_fn` is used instead of `emit_expr_at` for the
/// four function-typed cfg fields: it emits a raw function name for
/// `FuncValue` expressions, satisfying the bound directly.
fn emit_live_app_inner(
    ctx: &EmitCtx,
    fields: &[(sky_intern::Symbol, Expr)],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let init_e = lookup_field(ctx, fields, "init")?;
    let update_e = lookup_field(ctx, fields, "update")?;
    let view_e = lookup_field(ctx, fields, "view")?;
    let subs_e = lookup_field(ctx, fields, "subscriptions")?;

    // #91 seal: gate the Model type against `live_app`'s serde+Clone+PartialEq
    // bound BEFORE emitting. A non-serialisable Model (e.g. a field of type
    // `Cmd`/`Sub`/`Task`/`Decoder`/`Db`/function, or `Html`/`Element`/`Color`)
    // would otherwise `skyc`-succeed and then `cargo`-fail on the missing trait.
    // The gate converts that into a fail-closed `SKY-L0120` diagnostic.
    if let Some(model_ty) = crate::emit_model_gate::model_ty_of_view(view_e) {
        crate::emit_model_gate::check_admissible_model(
            ctx,
            model_ty,
            sky_diagnostics::AppShape::Live,
        )?;
    }

    // #94 seal: gate the Msg type against `live_app`'s Clone+Send+Sync+Debug
    // bound. The predicate is ir_type_is_derivable (NOT serde) — Msg is never
    // persisted, so Html-carrying Msg is accepted. A Cmd/Sub/Task/function in
    // Msg would cargo-fail; the gate makes it a fail-closed SKY-L0122 error.
    if let Some(msg_ty) = crate::emit_model_gate::msg_ty_of_update(update_e) {
        crate::emit_model_gate::check_admissible_msg(
            ctx,
            msg_ty,
            sky_diagnostics::AppShape::Live,
        )?;
    }

    let init_s = emit_live_fn(ctx, init_e, indent, child, generics)?;
    let update_s = emit_live_fn(ctx, update_e, indent, child, generics)?;
    let view_s = emit_live_fn(ctx, view_e, indent, child, generics)?;
    let subs_s = emit_live_fn(ctx, subs_e, indent, child, generics)?;

    // T5 emit branch — parity with ExprEmitter.hs:1670.
    //
    // Recover the Model from `view : Model -> Html Msg`'s first parameter.
    // If the Model record has a `page` field, this is a routed app → emit
    // `live_app_routed` with `routes`, `notFound`, and a generated `set_page`
    // closure.  Otherwise drop routes/notFound and emit the single-page
    // `live_app`.
    //
    // `routes` and `notFound` are always present as required cfg fields
    // (constrain.rs kernel scheme) but are only forwarded to the runtime in the
    // routed branch — single-page apps pass them as structural no-ops.
    if let Some((model_ty, page_ty)) = routed_page_field(ctx, view_e) {
        let routes_e = lookup_field(ctx, fields, "routes")?;
        let not_found_e = lookup_field(ctx, fields, "notFound")?;
        let routes_s = emit_expr_at(ctx, routes_e, indent, child, generics)?;
        let not_found_s = emit_expr_at(ctx, not_found_e, indent, child, generics)?;
        let model_ty_s = render_type(ctx, model_ty, generics)?;
        let page_ty_s = render_type(ctx, page_ty, generics)?;
        // `set_page` mirrors ExprEmitter.hs:1721-1733: a Rust closure that
        // updates the `page` field of the Model using struct-update syntax.
        // The field identifier "page" needs no keyword-mangling ("page" is not
        // a Rust reserved word).
        let set_page = format!(
            "move |__page: {page_ty_s}, __model: {model_ty_s}| \
             {model_ty_s} {{ page: __page, ..__model }}"
        );
        return Ok(Some(format!(
            "sky_runtime::live::live_app_routed(\
             {init_s}, \
             {update_s}, \
             {view_s}, \
             {subs_s}, \
             {routes_s}, \
             {not_found_s}, \
             {set_page}, \
             ::std::env::var(\"SKY_LIVE_STORE\").unwrap_or_else(|_| \"memory\".to_string()), \
             ::std::env::var(\"SKY_LIVE_STORE_PATH\").unwrap_or_else(|_| ::std::string::String::new())\
             )"
        )));
    }

    // Single-page (non-routed) path — `routes`/`notFound` are structurally
    // present in the cfg but not forwarded to the runtime entry.
    //
    // The store kind and path come from env at call time so a single binary can
    // switch stores without recompilation (`SKY_LIVE_STORE` / `SKY_LIVE_STORE_PATH`).
    Ok(Some(format!(
        "sky_runtime::live::live_app(\
         {init_s}, \
         {update_s}, \
         {view_s}, \
         {subs_s}, \
         ::std::env::var(\"SKY_LIVE_STORE\").unwrap_or_else(|_| \"memory\".to_string()), \
         ::std::env::var(\"SKY_LIVE_STORE_PATH\").unwrap_or_else(|_| ::std::string::String::new())\
         )"
    )))
}

/// Emit a `params.get(i)` expression that decodes the `i`-th route `:param`
/// string into the Rust type corresponding to `field_ty` (T6).
///
/// Route captures are always URL strings; this function converts them to the
/// expected Rust primitive with a graceful `unwrap_or_default` fallback on
/// parse failure so a malformed URL segment never panics the runtime.
///
/// Supported types and their decode expressions:
///
/// | `IrType`  | emitted expression |
/// |-----------|-------------------|
/// | `Str`     | `.cloned().unwrap_or_default()` |
/// | `Int`     | `.and_then(|s| s.parse::<i64>().ok()).unwrap_or_default()` |
/// | `Float`   | `.and_then(|s| s.parse::<f64>().ok()).unwrap_or_default()` |
/// | `Bool`    | `.map(|s| s == "true").unwrap_or_default()` |
/// | other     | compile-time [`Diagnostic::CompilerBug`] (unsupported payload) |
///
/// This is a sanctioned divergence from the Go/Haskell reference, which assumes
/// all route payloads are `String`. See `docs/divergences-from-sky.md`.
fn route_param_get(field_ty: &IrType, i: usize) -> DResult<String> {
    Ok(match field_ty {
        IrType::Str => format!("params.get({i}).cloned().unwrap_or_default()"),
        IrType::Int => format!(
            "params.get({i}).and_then(|s| s.parse::<i64>().ok()).unwrap_or_default()"
        ),
        IrType::Float => format!(
            "params.get({i}).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default()"
        ),
        IrType::Bool => format!(
            "params.get({i}).map(|s| s == \"true\").unwrap_or_default()"
        ),
        other => {
            // Item 3b (#120): upgrade to SKY-L0123. Item 4 (#120): replace
            // `{other:?}` (which leaks internal IR representation like
            // `Enum { home: ModPath([…]) }`) with a user-facing type name.
            return Err(Diagnostic::Lower {
                span: Span::DUMMY,
                msg: LowerError::RouteParamUnsupportedType {
                    field_index: i,
                    type_name: ir_type_display_name(other).into(),
                },
            });
        }
    })
}

/// The parameter types of a function-shaped route page builder — a named
/// function reference ([`Expr::FuncValue`]) or an inline lambda
/// ([`Expr::Lambda`]).  Both carry concrete, solved parameter [`IrType`]s
/// (the same property `emit_model_gate::fn_param_ty` relies on).  `None` for
/// any other expression shape — the caller fails closed.
fn builder_fn_params(e: &Expr) -> Option<Vec<&IrType>> {
    match e {
        Expr::FuncValue {
            ty: IrType::Fun(params, _),
            ..
        } => Some(params.iter().collect()),
        Expr::Lambda { params, .. } => Some(params.iter().map(|(_, ty)| ty).collect()),
        _ => None,
    }
}

/// Detect whether the Model type (recovered from `view`'s first parameter)
/// has a `page` field — the compile-time signal for a routed app.
///
/// Returns `Some((model_ty, page_ty))` when:
/// - `view` is an `Expr::FuncValue` OR an `Expr::Lambda` whose first parameter
///   type is recoverable (`emit_model_gate::fn_param_ty` — the #95 fix; a
///   lambda `view` previously fell through here and a ROUTED app silently
///   emitted the non-routed `live_app`, discarding `routes`/`notFound`),
/// - the Model is an `IrType::Record`, and
/// - one of its fields resolves to the Sky identifier `"page"`.
///
/// Returns `None` for single-page apps or when the Model type cannot be
/// structurally recovered (treated as "unrouted" — never false-blocks a
/// well-formed program, mirrors the same "cannot prove inadmissible" policy
/// as `emit_model_gate`).  Sharing `model_ty_of_view` with the #91 Model gate
/// keeps the type-tier `RoutedLiveCheck` and this emit-tier detection in
/// agreement on every cfg-field shape the gate can see.
fn routed_page_field<'a>(ctx: &EmitCtx, view_e: &'a Expr) -> Option<(&'a IrType, &'a IrType)> {
    let model_ty = crate::emit_model_gate::model_ty_of_view(view_e)?;
    let IrType::Record(record_fields) = model_ty else {
        return None;
    };
    for (sym, ty) in record_fields {
        if ctx.resolve_ident(*sym).ok()? == "page" {
            return Some((model_ty, ty));
        }
    }
    None
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Emit a cfg-field expression for `live_app`.
///
/// For a named function reference ([`Expr::FuncValue`]), emits the raw callee
/// name (e.g. `Main_init`) rather than a boxed closure.  A named function item
/// satisfies `Fn(...) + Send + Sync + 'static` via the compiler's blanket impl,
/// whereas a `Box<dyn Fn(...) + Send + Sync>` would be a distinct type requiring
/// explicit bounds that the `emit_func_value` path does not add.
///
/// For any other expression (lambda, local variable, etc.) falls back to the
/// general [`emit_expr_at`] emitter.  A lambda that does not capture `Send`
/// data will surface a cargo error with a clear trait-bound message — the correct
/// fail-closed behaviour.
fn emit_live_fn(
    ctx: &EmitCtx,
    e: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<String> {
    if let Expr::FuncValue { callee, .. } = e {
        // Raw function-item reference: satisfies Send + Sync + 'static implicitly.
        return crate::emit_expr::callee_name(ctx, callee);
    }
    // Fallback: general emitter (lambda, local var, etc.).
    emit_expr_at(ctx, e, indent, child, generics)
}

/// Find a record field by its Sky source name in an IR field list.
///
/// Returns the field's value expression.  Fail-closed: a missing required field
/// surfaces a [`Diagnostic::CompilerBug`] rather than silently emitting wrong
/// code (MAKE INVALID STATES UNREPRESENTABLE).
fn lookup_field<'f>(
    ctx: &EmitCtx,
    fields: &'f [(sky_intern::Symbol, Expr)],
    name: &str,
) -> DResult<&'f Expr> {
    for (sym, expr) in fields {
        if ctx.resolve_ident(*sym)? == name {
            return Ok(expr);
        }
    }
    Err(Diagnostic::CompilerBug {
        where_: "sky_backend_rust::emit_live_call",
        detail: format!(
            "required Live.app cfg field `{name}` not found; \
             available fields: [{}]",
            fields
                .iter()
                .filter_map(|(s, _)| ctx.resolve_ident(*s).ok())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// A short, user-facing display name for an [`IrType`] used in diagnostic
/// messages. Avoids leaking the internal `Debug` representation of the IR
/// (which includes interned `Symbol` IDs and `ModPath` vectors that are
/// meaningless to the user).
///
/// This is intentionally coarse — diagnostics only need enough detail to
/// direct the user to the right fix, not a full type-printer.
const fn ir_type_display_name(ty: &IrType) -> &'static str {
    match ty {
        IrType::Int => "Int",
        IrType::Float => "Float",
        IrType::Str => "String",
        IrType::Bool => "Bool",
        IrType::Char => "Char",
        IrType::Unit => "Unit",
        IrType::Task(_) => "Task",
        IrType::Enum { .. } => "ADT",
        IrType::Maybe(_) => "Maybe",
        IrType::Result(_, _) => "Result",
        IrType::List(_) => "List",
        IrType::Tuple(_) => "Tuple",
        IrType::Record(_) => "record",
        IrType::Fun(_, _) => "function",
        IrType::Generic(_) => "generic",
        IrType::Dict(_, _) => "Dict",
        IrType::Set(_) => "Set",
        IrType::Bytes => "Bytes",
        IrType::Json => "Json",
        IrType::Decoder(_) => "Decoder",
        IrType::Db => "Db",
        IrType::Cmd(_) => "Cmd",
        IrType::Sub(_) => "Sub",
        IrType::ServerRequest => "Request",
        IrType::ServerResponse => "Response",
        IrType::ServerRoute => "ServerRoute",
        IrType::ServerCookie => "Cookie",
        IrType::StreamWriter => "StreamWriter",
        IrType::HttpRequest => "HttpRequest",
        // #127: Sky.Http.Server.WebSocket opaque handles.
        IrType::WebSocketServer => "WebSocketServer",
        IrType::WebSocketServerCfg => "WebSocketServerCfg",
        IrType::Ui { .. } => "Element",
        IrType::UiPlain(_) => "UiAttribute",
        IrType::LiveReq => "LiveReq",
        IrType::LiveRoute(_) => "LiveRoute",
        IrType::Order => "Order",
        IrType::Decimal => "Decimal",
        IrType::ErrorKind => "ErrorKind",
        IrType::Error => "Error",
    }
}
