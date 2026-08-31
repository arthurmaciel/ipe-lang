//! Emission for `Ipe.Web` / `Ipe.Web` app-entry kernels.
//!
//! Wires three of the four Web kernels:
//!
//! * [`KernelFn::WebApp`] — `Web.app cfg` → `ipe_runtime::web::web_app(…)`
//!   for single-page apps, or `web_app_routed(…)` when the Model carries a
//!   `page` field (the six-field cfg scheme with `routes` / `notFound`).
//! * [`KernelFn::WebRoute`] — `Web.route pattern ctor` →
//!   `ipe_runtime::web::route::Route::new(…)`.
//! * [`KernelFn::WebRenderStatic`] — `Html.renderStatic view model` →
//!   `ipe_runtime::web::web_render_static(…)`.
//!
//! # Correctness constraints (MAKE INVALID STATES UNREPRESENTABLE)
//!
//! * Required cfg fields are looked up with `lookup_field` (fail-closed on miss).
//! * Route params are accessed via fallible `.get(i)…?` expressions — never
//!   by index (panic vector eliminated). A decode failure returns `None` from
//!   the `Option<Page>` builder so `match_routes` routes to `not_found`
//!   rather than silently substituting a zero-value default (§B-route-param).
//! * Store kind / path are read from process env at call time, not compiled in.
//! * `Web.appRouted` is a vestigial alias routed through the same
//!   `lower_app_entry_cfg` path as `Web.app`; its arm here is a
//!   defensive invariant check.

use ipe_diagnostics::{DResult, Diagnostic, LowerError, Span};
use ipe_ir::{Callee, Expr, IrType, KernelFn};

use crate::EmitCtx;
use crate::emit_expr::emit_expr_at;
use crate::emit_types::{GenericScope, render_type};

/// Wrap an emitted `view : Model -> Element Msg` so its result type is the
/// `Html` the runtime sink mounts: the emitted `Element`-returning view is
/// closed over and threaded through `ui_layout` (empty attrs, the `Ui.layout []`
/// framework wrap). Raw HTML is reached through the `Ui.html` node inside this
/// `Element` view, not through a separate pass-through entry.
pub fn wrap_view(view_s: &str) -> String {
    // A `move` closure capturing the emitted view (a named `fn` item or a
    // fall-through expr) is `Fn(Model) -> Html + Send + Sync + 'static`
    // whenever the captured view is — the same bound the runtime entry
    // requires. `::std::vec::Vec::new()` is the empty `Ui.layout []` attr
    // list; `ui_layout` renders `Element` → `Html`.
    format!(
        "{{ let __view = {view_s}; \
         move |__model| ipe_runtime::ui::render::ui_layout(::std::vec::Vec::new(), __view(__model)) }}"
    )
}

/// Dispatch a `Ipe.Web` / `Ipe.Web` kernel call.
///
/// Returns `Some(emitted)` for the Web app-entry kernels; `None` for any
/// variant that is not one (the caller routes to this function only for
/// `k.is_web()` variants, but a defensive `None` for unknown variants avoids
/// a catchall `_` arm).
///
/// Called from `emit_ui_call`.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn emit_web_call(
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
        // ── Web.app { init, update, view, subscriptions, routes, notFound } ──
        //
        // The six-field cfg scheme. `emit_web_app_inner` branches
        // on the Model's `page` field: routed apps take `web_app_routed`
        // (routes + notFound + set_page); single-page apps take `web_app`.
        // `Web.embed` builds the same `WebApp` leaf from the same six-field cfg
        // as `Web.app` — it shares this emit path exactly. The only difference is
        // the handle kind: `Web.app` → `WebAppKind::Standalone` (binds its own
        // listener); `Web.embed` → `WebAppKind::Mountable` (carries a router
        // builder for `Server.mountApp` to nest on the shared port).
        KernelFn::WebApp | KernelFn::WebEmbed => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_web_call::WebApp",
                    detail: format!("Web.app requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with IPE-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `WebAppRouted` precedent.
            let Expr::Record { fields, .. } = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_web_call::WebApp",
                    detail: "Web.app cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with IPE-L0119"
                        .into(),
                });
            };
            let mountable = matches!(k, KernelFn::WebEmbed);
            emit_web_app_inner(ctx, fields, indent, child, generics, mountable)
        }

        // ── Web.appRouted — vestigial alias of `Web.app` ─────────────────
        //
        // The lower stage routes `Web.appRouted` through the same
        // `lower_app_entry_cfg` path as `Web.app` (the reference has ONE
        // `Web.app` that branches at emit time), so the alias takes the same
        // `emit_web_app_inner` branch here.  A non-literal cfg is rejected at
        // lower with IPE-L0119 exactly as for `Web.app`; the guard below is
        // the same defensive invariant.
        KernelFn::WebAppRouted => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_web_call::WebAppRouted",
                    detail: format!("Web.appRouted requires 1 argument, got {}", args.len()),
                });
            };
            let Expr::Record { fields, .. } = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_web_call::WebAppRouted",
                    detail: "Web.appRouted cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with IPE-L0119"
                        .into(),
                });
            };
            emit_web_app_inner(ctx, fields, indent, child, generics, false)
        }

        // ── Web.appWith settings cfg ─────────────────────────────────────
        //
        // `Web.appWith : List (Setting Web) -> cfg -> Task ()`. The settings
        // list is resolved into the process-wide runtime config (one
        // precedence: env > settings-in-code > built-in fallback) BEFORE the
        // same `emit_web_app_inner` task the plain `Web.app` produces runs — so
        // the host-bind / log-level / db-url a `Web.app` cannot set are in place
        // when the server binds. A non-literal cfg is rejected at lower with
        // IPE-L0119 exactly as for `Web.app`.
        KernelFn::WebAppWith => {
            let [settings_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_web_call::WebAppWith",
                    detail: format!("Web.appWith requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record { fields, .. } = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_web_call::WebAppWith",
                    detail: "Web.appWith cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with IPE-L0119"
                        .into(),
                });
            };
            let settings_s = emit_expr_at(ctx, settings_e, indent, child, generics)?;
            let Some(app_s) = emit_web_app_inner(ctx, fields, indent, child, generics, false)?
            else {
                return Ok(None);
            };
            // `app_s` is already wrapped in `WebApp(...)` by `emit_web_app_inner`.
            Ok(Some(format!(
                "{{ ipe_runtime::app_config::install_web({settings_s}); {app_s} }}"
            )))
        }

        // ── Web.route pattern ctor ─────────────────────────────────────────
        KernelFn::WebRoute => emit_web_route(ctx, args, indent, child, generics),

        // ── Html.renderStatic view model ────────────────────────────────────
        //
        // `Html.renderStatic : (Model -> Html Msg) -> Model -> Task Error ()`
        //
        // Emits: `ipe_runtime::web::web_render_static(view, model)`
        KernelFn::WebRenderStatic => {
            let [view_e, model_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_web_call::WebRenderStatic",
                    detail: format!("Html.renderStatic requires 2 arguments, got {}", args.len()),
                });
            };
            let view_s = emit_expr_at(ctx, view_e, indent, child, generics)?;
            let model_s = emit_expr_at(ctx, model_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::web::web_render_static({view_s}, {model_s})"
            )))
        }

        // Any non-Web kernel variant: let the standard path handle it.
        _ => Ok(None),
    }
}

// ── `Web.route` ──────────────────────────────────────────────────────────────

/// Emit `Web.route pattern builder` → `Route::new(&pattern, closure)`.
///
/// `Web.route : String -> builder -> WebRoute page`.
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
/// of emitting E0308). Sanctioned divergence §B-route-param.
fn emit_web_route(
    ctx: &EmitCtx,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let [pattern_e, builder_e] = args else {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_web_call::WebRoute",
            detail: format!("Web.route requires 2 arguments, got {}", args.len()),
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
        // The full field-type slice (not just the count) so each slot can
        // emit a type-directed conversion.
        let variant_tys = ctx.variant_fields(home, *ty, *variant)?;
        let ctor_s = emit_expr_at(ctx, builder_e, indent, child, generics)?;
        if variant_tys.is_empty() || !ctor_args.is_empty() {
            // Nullary ctor or fully-applied ctor: hoist the page value out of
            // the closure and clone it per call (`ExprEmitter.hs:1809` parity
            // — `{ let __c = ctor; move |_p| __c.clone() }`).  Constructing it
            // inside the body would move any captured payload out of an `Fn`
            // closure (E0507); every page ADT derives `Clone`.
            //
            // Returns `Some(…)` — the builder signature is now
            // `Fn(Vec<String>) -> Option<Page>` so decode failures in other
            // routes fall through to `not_found` (§B-route-param).
            format!(
                "{{ let __c = {ctor_s}; \
                 move |_params: ::std::vec::Vec<::std::string::String>| \
                 ::std::option::Option::Some(__c.clone()) }}"
            )
        } else {
            // Partial-ctor with N payload fields.
            //
            // Item 1: static arity check — count ':param' segments in
            // the pattern and compare against the constructor's payload count.
            // A mismatch is a compile-time error (IPE-L0122): the route can
            // never deliver the right arguments.  Only checked when the pattern
            // is a string literal (the only shape the parser accepts for a
            // route pattern); other shapes are left to cargo for now.
            if let Expr::Str(pat_s) = pattern_e {
                let param_count = pat_s.split('/').filter(|seg| seg.starts_with(':')).count();
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
            // Emit type-directed fallible `params.get(i)` conversions using
            // `?`-propagation inside a closure that returns `Option<Page>`.
            // A decode failure for any slot returns `None`, which `match_routes`
            // maps to `not_found` (§B-route-param — sanctioned divergence from
            // the reference which silently substitutes a default value).
            let mut param_gets = Vec::with_capacity(variant_tys.len());
            for (i, field_ty) in variant_tys.iter().enumerate() {
                param_gets.push(route_param_get(field_ty, i)?);
            }
            format!(
                "move |params: ::std::vec::Vec<::std::string::String>| \
                 ::std::option::Option::Some({ctor_s}({}))",
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
                 ::std::option::Option::Some((__b)(params)) }}"
            )
        } else {
            let mut param_gets = Vec::with_capacity(param_tys.len());
            for (i, field_ty) in param_tys.iter().enumerate() {
                param_gets.push(route_param_get(field_ty, i)?);
            }
            format!(
                "{{ let __b = {builder_s}; \
                 move |params: ::std::vec::Vec<::std::string::String>| \
                 ::std::option::Option::Some((__b)({})) }}",
                param_gets.join(", ")
            )
        }
    } else {
        // Any other builder shape (a `Var` referencing a local, a call
        // result, …) carries no recoverable parameter types, so the builder
        // closure cannot be emitted soundly — fail CLOSED with IPE-L0123.
        // Pre-round-4 this arm emitted `(builder)(params)` untyped, which
        // cargo-failed (E0308/E0618) for every shape except the rare raw
        // `List String -> Page` builder — a silent seal hole.
        return Err(Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::RouteBuilderUnsupportedShape,
        });
    };

    // `Route::new` takes `pattern: &str`.  Ipê string literals emit as
    // `"…".to_string()` (an owned `String`); prepend `&` so the `&String`
    // deref-coerces to `&str` at the call site.  Variable references also
    // type as `String`, so `&var` is equally correct.
    Ok(Some(format!(
        "ipe_runtime::web::route::Route::new(&{pattern_s}, {build_closure})"
    )))
}

// ── Non-routed `web_app` ──────────────────────────────────────────────────────

/// Emit `ipe_runtime::web::web_app(init, update, view, subs, store, path)`.
///
/// The `init` function is passed directly — after B1 constrain, the solver pins
/// its first parameter type to `WebReq`, so the emitted Rust function already
/// has signature `fn(_req: WebReq) -> (Model, IpeCmd<Msg>)`.
///
/// `update` is `Fn(Msg, Model) -> (Model, IpeCmd<Msg>)` — multi-param Ipê
/// functions are lowered as uncurried Rust fns, matching the runtime bound.
///
/// Store kind and path are read from process env at call time (never compiled in)
/// so a single binary can switch stores via env without recompilation.
///
/// # Function-field emission
///
/// `web_app`'s generic parameters carry `+ Send + Sync + 'static` bounds on
/// the function arguments.  A named Rust `fn` item satisfies these bounds
/// implicitly (the compiler's blanket impl covers all `fn` pointers and
/// non-capturing function items).  By contrast, a `Box<dyn Fn(...)>` as emitted
/// by the general `emit_expr_at` / `emit_func_value` path does NOT carry these
/// bounds without explicit annotation — `Box<dyn Fn(...) + Send + Sync>` is a
/// different type from `Box<dyn Fn(...)>`.
///
/// For this reason, `emit_web_fn` is used instead of `emit_expr_at` for the
/// four function-typed cfg fields: it emits a raw function name for
/// `FuncValue` expressions, satisfying the bound directly.
fn emit_web_app_inner(
    ctx: &EmitCtx,
    fields: &[(ipe_intern::Symbol, Expr)],
    indent: usize,
    child: u16,
    generics: GenericScope,
    // `true` for `Web.embed` — build a `WebAppKind::Mountable` handle carrying
    // BOTH the standalone `serve` task AND a router-builder for `Server.mountApp`
    // to nest. `false` for `Web.app` — a `WebAppKind::Standalone` bind-your-own-
    // listener handle.
    mountable: bool,
) -> DResult<Option<String>> {
    let init_e = lookup_field(ctx, fields, "init")?;
    let update_e = lookup_field(ctx, fields, "update")?;
    let view_e = lookup_field(ctx, fields, "view")?;
    let subs_e = lookup_field(ctx, fields, "subscriptions")?;

    // seal: gate the Model type against `web_app`'s serde+Clone+PartialEq
    // bound BEFORE emitting. A non-serialisable Model (e.g. a field of type
    // `Cmd`/`Sub`/`Task`/`Decoder`/`Db`/function, or `Html`/`Element`/`Color`)
    // would otherwise `ipe`-succeed and then `cargo`-fail on the missing trait.
    // The gate converts that into a fail-closed `IPE-L0120` diagnostic.
    if let Some(model_ty) = crate::emit_model_gate::model_ty_of_view(view_e) {
        crate::emit_model_gate::check_admissible_model(
            ctx,
            model_ty,
            ipe_diagnostics::AppShape::Web,
        )?;
    }

    // seal: gate the Msg type against `web_app`'s Clone+Send+Sync+Debug
    // bound. The predicate is ir_type_is_derivable (NOT serde) — Msg is never
    // persisted, so Html-carrying Msg is accepted. A Cmd/Sub/Task/function in
    // Msg would cargo-fail; the gate makes it a fail-closed IPE-L0122 error.
    if let Some(msg_ty) = crate::emit_model_gate::msg_ty_of_update(update_e) {
        crate::emit_model_gate::check_admissible_msg(ctx, msg_ty, ipe_diagnostics::AppShape::Web)?;
    }

    let tag_const = schema_tag_const(ctx, view_e)?;

    let init_s = emit_web_fn(ctx, init_e, indent, child, generics)?;
    let update_s = emit_web_fn(ctx, update_e, indent, child, generics)?;
    let view_raw_s = emit_web_fn(ctx, view_e, indent, child, generics)?;
    // `Web.app`'s `view : Model -> Element Msg` — the framework applies
    // `Ui.layout` internally, turning the portable `Element` into the `Html`
    // the runtime sink mounts. The wrap closes over the emitted view (a named
    // `fn` item or the fall-through expr), so it inherits the same
    // `Fn(Model) -> Html + Send + Sync + 'static` shape the runtime requires.
    let view_s = wrap_view(&view_raw_s);
    let subs_s = emit_web_fn(ctx, subs_e, indent, child, generics)?;

    // Browser target: the same cfg drives the client sink. No session store
    // (the model lives in the tab), so no store args and no schema tag.
    if ctx.target == ipe_ir::Target::WasmClient {
        if let Some((model_ty, page_ty)) = routed_page_field(ctx, view_e) {
            // Routed app: emit `wasm_app_routed` with routes, notFound, and
            // a generated `set_page` closure. The History-API `popstate`
            // listener is installed by the runtime entry point.
            let routes_e = lookup_field(ctx, fields, "routes")?;
            let not_found_e = lookup_field(ctx, fields, "notFound")?;
            let routes_s = emit_expr_at(ctx, routes_e, indent, child, generics)?;
            let not_found_s = emit_expr_at(ctx, not_found_e, indent, child, generics)?;
            let model_ty_s = render_type(ctx, model_ty, generics)?;
            let page_ty_s = render_type(ctx, page_ty, generics)?;
            let set_page = set_page_closure(
                ctx,
                fields,
                update_e,
                &page_ty_s,
                &model_ty_s,
                indent,
                child,
                generics,
            )?;
            return Ok(Some(format!(
                "ipe_runtime::wasm::wasm_app_routed(\
                 {init_s}, \
                 {update_s}, \
                 {view_s}, \
                 {subs_s}, \
                 {routes_s}, \
                 {not_found_s}, \
                 {set_page}\
                 )"
            )));
        }
        return Ok(Some(format!(
            "ipe_runtime::wasm::wasm_app({init_s}, {update_s}, {view_s}, {subs_s})"
        )));
    }

    // T5 emit branch — parity with ExprEmitter.hs:1670.
    //
    // Recover the Model from `view : Model -> Html Msg`'s first parameter.
    // If the Model record has a `page` field, this is a routed app → emit
    // `web_app_routed` with `routes`, `notFound`, and a generated `set_page`
    // closure.  Otherwise drop routes/notFound and emit the single-page
    // `web_app`.
    //
    // `routes` and `notFound` are always present as required cfg fields
    // (constrain.rs kernel scheme) but are only forwarded to the runtime in the
    // routed branch — single-page apps pass them as structural no-ops.
    if let Some((model_ty, page_ty)) = routed_page_field(ctx, view_e) {
        return emit_routed_web_leaf(
            ctx, fields, &tag_const, mountable, model_ty, page_ty, update_e, &init_s, &update_s,
            &view_s, &subs_s, indent, child, generics,
        );
    }

    // Single-page (non-routed) path — `routes`/`notFound` are structurally
    // present in the cfg but not forwarded to the runtime entry. Delegated to a
    // helper to keep this function within the line budget.
    emit_single_page_web_leaf(
        ctx, &tag_const, mountable, init_e, update_e, view_e, subs_e, &init_s, &update_s, &view_s,
        &subs_s, indent, child, generics,
    )
}

/// Emit the routed (`Model` has a `page` field) `WebApp` leaf — a `Standalone`
/// `web_app_routed` handle. Routing (routes table + `notFound` + generated
/// `set_page`) is forwarded to the runtime entry.
///
/// A routed `Web.embed` needs a routed mount router-builder
/// (`web_embed_router_routed`), which is not yet implemented, so `mountable`
/// here is rejected fail-closed at emit — never a mis-emit. A single-page
/// `Web.embed` mounts today; a routed one is a follow-up.
#[allow(clippy::too_many_arguments)] // threads the pre-emitted callback strings + their source exprs + the solved model/page types
fn emit_routed_web_leaf(
    ctx: &EmitCtx,
    fields: &[(ipe_intern::Symbol, Expr)],
    tag_const: &str,
    mountable: bool,
    model_ty: &ipe_ir::IrType,
    page_ty: &ipe_ir::IrType,
    update_e: &Expr,
    init_s: &str,
    update_s: &str,
    view_s: &str,
    subs_s: &str,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let routes_e = lookup_field(ctx, fields, "routes")?;
    let not_found_e = lookup_field(ctx, fields, "notFound")?;
    let routes_s = emit_expr_at(ctx, routes_e, indent, child, generics)?;
    let not_found_s = emit_expr_at(ctx, not_found_e, indent, child, generics)?;
    let model_ty_s = render_type(ctx, model_ty, generics)?;
    let page_ty_s = render_type(ctx, page_ty, generics)?;
    let set_page = set_page_closure(
        ctx,
        fields,
        update_e,
        &page_ty_s,
        &model_ty_s,
        indent,
        child,
        generics,
    )?;
    if mountable {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_web_call::WebEmbed",
            detail: "Web.embed of a routed app (Model with a `page` field) is \
                     not yet supported for Server.mountApp; embed a single-page \
                     Web app, or serve the routed app standalone with Web.app"
                .into(),
        });
    }
    Ok(Some(format!(
        "{{ {tag_const} \
         ipe_runtime::tea::WebApp(ipe_runtime::tea::WebAppKind::Standalone(\
         ipe_runtime::web::web_app_routed(\
         {init_s}, \
         {update_s}, \
         {view_s}, \
         {subs_s}, \
         {routes_s}, \
         {not_found_s}, \
         {set_page}, \
         ::std::env::var(\"IPE_WEB_STORE\").unwrap_or_else(|_| \"memory\".to_string()), \
         ::std::env::var(\"IPE_WEB_STORE_PATH\").unwrap_or_else(|_| ::std::string::String::new()), \
         IPE_WEB_MODEL_SCHEMA_TAG\
         ))) }}"
    )))
}

/// Emit the single-page (non-routed) `WebApp` leaf: a `Standalone` handle for
/// `Web.app`, or a `Mountable` handle (standalone `serve` task + a
/// `web_embed_router` router builder) for `Web.embed`.
///
/// The store kind/path come from env at call time so one binary can switch
/// stores without recompilation. For `Web.embed` the four callbacks are emitted
/// a SECOND time for the router builder — each `emit_web_fn` yields a named `fn`
/// item / pure closure, so re-emitting is a fresh reference, not a move.
#[allow(clippy::too_many_arguments)] // threads the already-emitted callback strings + their source exprs
fn emit_single_page_web_leaf(
    ctx: &EmitCtx,
    tag_const: &str,
    mountable: bool,
    init_e: &Expr,
    update_e: &Expr,
    view_e: &Expr,
    subs_e: &Expr,
    init_s: &str,
    update_s: &str,
    view_s: &str,
    subs_s: &str,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let store_args = "::std::env::var(\"IPE_WEB_STORE\").unwrap_or_else(|_| \"memory\".to_string()), \
         ::std::env::var(\"IPE_WEB_STORE_PATH\").unwrap_or_else(|_| ::std::string::String::new()), \
         IPE_WEB_MODEL_SCHEMA_TAG";
    let serve_call = format!(
        "ipe_runtime::web::web_app({init_s}, {update_s}, {view_s}, {subs_s}, {store_args})"
    );
    if mountable {
        let init_s2 = emit_web_fn(ctx, init_e, indent, child, generics)?;
        let update_s2 = emit_web_fn(ctx, update_e, indent, child, generics)?;
        let view_raw_s2 = emit_web_fn(ctx, view_e, indent, child, generics)?;
        let view_s2 = wrap_view(&view_raw_s2);
        let subs_s2 = emit_web_fn(ctx, subs_e, indent, child, generics)?;
        let router_call = format!(
            "ipe_runtime::web::web_embed_router({init_s2}, {update_s2}, {view_s2}, {subs_s2}, {store_args})"
        );
        return Ok(Some(format!(
            "{{ {tag_const} \
             ipe_runtime::tea::WebApp(ipe_runtime::tea::WebAppKind::Mountable {{ \
             serve: {serve_call}, router: {router_call} }}) }}"
        )));
    }
    Ok(Some(format!(
        "{{ {tag_const} \
         ipe_runtime::tea::WebApp(ipe_runtime::tea::WebAppKind::Standalone({serve_call})) }}"
    )))
}

/// Emit a `params.get(i)` expression that decodes the `i`-th route `:param`
/// string into the Rust type corresponding to `field_ty`, using `?`-propagation
/// so a failed decode returns `None` from the enclosing `Option<Page>` closure
/// rather than silently substituting a default value.
///
/// The generated expressions are valid inside a closure that returns
/// `Option<Page>` — each slot is a `?`-terminated sub-expression so any
/// decode failure short-circuits the closure to `None`, which `match_routes`
/// maps to `not_found` (§B-route-param).
///
/// Supported types and their emitted decode expressions:
///
/// | `IrType`  | emitted expression (inside `Option<Page>` closure) |
/// |-----------|---------------------------------------------------|
/// | `Str`     | `params.get({i}).cloned()?` |
/// | `Int`     | `params.get({i}).and_then(\|s\| s.parse::<i64>().ok())?` |
/// | `Float`   | `params.get({i}).and_then(\|s\| s.parse::<f64>().ok())?` |
/// | `Bool`    | `params.get({i})?.parse::<bool>().ok()?` |
/// | other     | compile-time error (unsupported payload type) |
///
/// This is a sanctioned divergence, which assumes
/// all route payloads are `String` and silently substitutes zero-values on
/// decode failure. Sanctioned divergence §B-route-param.
fn route_param_get(field_ty: &IrType, i: usize) -> DResult<String> {
    Ok(match field_ty {
        IrType::Str => format!("params.get({i}).cloned()?"),
        IrType::Int => {
            format!("params.get({i}).and_then(|s| s.parse::<i64>().ok())?")
        }
        IrType::Float => {
            format!("params.get({i}).and_then(|s| s.parse::<f64>().ok())?")
        }
        // `parse::<bool>()` accepts "true"/"false" exactly (Rust stdlib),
        // which matches the Ipê runtime's Bool string convention.
        IrType::Bool => format!("params.get({i})?.parse::<bool>().ok()?"),
        other => {
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
///   type is recoverable (`emit_model_gate::fn_param_ty` handles both shapes,
///   so a routed lambda `view` is not misrouted to the non-routed `web_app`,
///   which would discard `routes`/`notFound`),
/// - the Model is an `IrType::Record`, and
/// - one of its fields resolves to the Ipê identifier `"page"`.
///
/// Returns `None` for single-page apps or when the Model type cannot be
/// structurally recovered (treated as "unrouted" — never false-blocks a
/// well-formed program, mirrors the same "cannot prove inadmissible" policy
/// as `emit_model_gate`).  Sharing `model_ty_of_view` with the Model gate
/// keeps the type-tier `RoutedWebCheck` and this emit-tier detection in
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

/// Build the `const IPE_WEB_MODEL_SCHEMA_TAG: [u8; 32] = [...]` declaration the
/// routed/non-routed `web_app*` call carries as its final argument.
///
/// H24: the tag is computed from the SAME recovered Model type the
/// admissibility gate checked, so the session store can reject a persisted
/// checkpoint whose tag differs BEFORE deserializing it. An unrecoverable
/// `view` shape (the gate's documented fail-open residual) gets the all-zero
/// sentinel: every same-sentinel checkpoint is treated as same-schema — exactly
/// the pre-tag behaviour for those shapes, never a new rejection.
fn schema_tag_const(ctx: &EmitCtx, view_e: &Expr) -> DResult<String> {
    let schema_tag: [u8; 32] = match crate::emit_model_gate::model_ty_of_view(view_e) {
        Some(model_ty) => crate::emit_model_schema::model_schema_tag(ctx, model_ty)?,
        None => [0u8; 32],
    };
    let tag_bytes = schema_tag
        .iter()
        .map(|b| format!("0x{b:02x}"))
        .collect::<Vec<_>>()
        .join(", ");
    // The byte value is the whole point — the identifier just keeps the emitted
    // call arg readable at its single use site.
    Ok(format!(
        "const IPE_WEB_MODEL_SCHEMA_TAG: [u8; 32] = [{tag_bytes}];"
    ))
}

/// Build the `set_page : Fn(Page, Model) -> Model` closure passed to the routed
/// runtime entry, which the runtime calls to reconcile the model to the page a
/// URL matched.
///
/// The URL-driven navigation event flows through the app's `update` exactly when
/// the cfg supplies an `onNavigate : page -> msg` field — the explicit-in-config
/// navigation form:
///
/// * **`onNavigate` present** — the matched page becomes a `Msg` via the
///   supplied handler, and that `Msg` is dispatched through `update`:
///   `move |page, model| { let (m, _cmd) = update(onNavigate(page), model); m }`.
///   The app owns navigation — the new page reaches the model only through the
///   `update` arm the author writes for the `onNavigate`-produced `Msg`. The
///   `update` command is discarded here (URL reconcile is a synchronous
///   model-only step, mirroring the implicit form's `Cmd.none`).
///
/// * **`onNavigate` absent** — the implicit `\p -> __SetPage p` desugaring whose
///   `update` arm is `({ model | page = p }, Cmd.none)`: the runtime writes the
///   matched page straight into the model's `page` field via struct update. This
///   is the historical magic-page behaviour, reproduced byte-for-byte.
#[allow(clippy::too_many_arguments)]
fn set_page_closure(
    ctx: &EmitCtx,
    fields: &[(ipe_intern::Symbol, Expr)],
    update_e: &Expr,
    page_ty_s: &str,
    model_ty_s: &str,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<String> {
    match lookup_optional_field(ctx, fields, "onNavigate")? {
        // Explicit-in-config navigation: the matched page is turned into a Msg
        // and dispatched through `update`, so the author owns the page
        // transition in `update` rather than the runtime mutating `page`.
        Some(on_navigate_e) => {
            let update_s = emit_web_fn(ctx, update_e, indent, child, generics)?;
            let on_navigate_s = emit_web_fn(ctx, on_navigate_e, indent, child, generics)?;
            Ok(format!(
                "{{ let __update = {update_s}; let __on_navigate = {on_navigate_s}; \
                 move |__page: {page_ty_s}, __model: {model_ty_s}| {{ \
                 let (__next, _cmd) = (__update)((__on_navigate)(__page), __model); __next }} }}"
            ))
        }
        // The absent-field desugaring: struct-update the `page` field directly.
        // Byte-identical to the historical magic-page emission — never change
        // this string without re-baselining every routed-live golden.
        None => Ok(format!(
            "move |__page: {page_ty_s}, __model: {model_ty_s}| \
             {model_ty_s} {{ page: __page, ..__model }}"
        )),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Emit a cfg-field expression for `web_app` / `web_app_routed`.
///
/// The runtime entry's four function-typed slots (`FInit`/`FUpdate`/`FView`/
/// `FSubs`) are GENERIC type parameters bounded `Fn(...) -> R + Send + Sync +
/// 'static` — NOT `Box<dyn Fn>` slots. rustc monomorphizes each to the concrete
/// argument type, so the emitted value must satisfy `Send + Sync` *by its own
/// type*, not by an annotation.
///
/// Two shapes satisfy that via the compiler's blanket impl:
/// - A named function reference ([`Expr::FuncValue`]) — emit the raw callee name
///   (e.g. `main_init`); a `fn` item is `Send + Sync + 'static` implicitly.
/// - An inline lambda ([`Expr::Lambda`]/[`Expr::SharedLambda`]) — emit the
///   *unboxed* closure (`move |p: T| -> R { body }`), letting rustc infer
///   `Send + Sync` from its captures. This mirrors how the sibling `set_page`
///   closure and each `Route::new` page-builder closure are emitted in this same
///   call — all passed unboxed into a generic slot.
///
/// The general [`emit_expr_at`] path is WRONG for these slots: it pins a lambda
/// to `Box<dyn Fn(...) -> R + Send + 'static>` (see `emit_lambda`). That trait
/// object carries `Send` but NOT `Sync`, so it fails the slot's `Sync` bound —
/// an exit-0-then-cargo-fail SEAL break (`E0277`: subscription
/// `\_ -> Sub.none` in `examples/10-live-component`). Emitting the closure
/// unboxed keeps the concrete closure type, whose auto-derived `Send + Sync`
/// satisfies the bound.
///
/// For any other expression shape (a local variable holding a first-class
/// function value, etc.) falls back to [`emit_expr_at`]. Such a value is already
/// typed by its binding site; if it is not `Send + Sync` the cargo error carries
/// a clear trait-bound message — the correct fail-closed behaviour, and a shape
/// the reference frontend also cannot produce for these slots.
fn emit_web_fn(
    ctx: &EmitCtx,
    e: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<String> {
    match e {
        Expr::FuncValue { callee, .. } => {
            // Raw function-item reference: satisfies Send + Sync + 'static implicitly.
            crate::emit_expr::callee_name(ctx, callee)
        }
        // Inline lambda: emit the UNBOXED closure so it monomorphizes the generic
        // slot to its concrete closure type (auto Send + Sync from captures),
        // never the Sync-erasing `Box<dyn Fn + Send>` the general path would pin.
        Expr::Lambda { params, ret, body } | Expr::SharedLambda { params, ret, body } => {
            crate::emit_expr::emit_lambda_unboxed(ctx, params, ret, body, indent, child, generics)
        }
        // Fallback: general emitter (local var holding a function value, etc.).
        _ => emit_expr_at(ctx, e, indent, child, generics),
    }
}

/// Find a record field by its Ipê source name in an IR field list.
///
/// Returns the field's value expression.  Fail-closed: a missing required field
/// surfaces a [`Diagnostic::CompilerBug`] rather than silently emitting wrong
/// code (MAKE INVALID STATES UNREPRESENTABLE).
fn lookup_field<'f>(
    ctx: &EmitCtx,
    fields: &'f [(ipe_intern::Symbol, Expr)],
    name: &str,
) -> DResult<&'f Expr> {
    for (sym, expr) in fields {
        if ctx.resolve_ident(*sym)? == name {
            return Ok(expr);
        }
    }
    Err(Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::emit_web_call",
        detail: format!(
            "required Web.app cfg field `{name}` not found; \
             available fields: [{}]",
            fields
                .iter()
                .filter_map(|(s, _)| ctx.resolve_ident(*s).ok())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// Find an OPTIONAL cfg field by its Ipê source name, returning `None` when the
/// field is absent rather than failing.
///
/// The Web cfg record is row-open (constrain.rs `K::WebApp` scheme): optional
/// fields such as `onNavigate` are absorbed by the row tail and may be omitted.
/// A field whose Ipê name cannot be resolved is skipped — it can never be the
/// name being looked up.
fn lookup_optional_field<'f>(
    ctx: &EmitCtx,
    fields: &'f [(ipe_intern::Symbol, Expr)],
    name: &str,
) -> DResult<Option<&'f Expr>> {
    for (sym, expr) in fields {
        if ctx.resolve_ident(*sym)? == name {
            return Ok(Some(expr));
        }
    }
    Ok(None)
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
        IrType::Fun(_, _) | IrType::SharedFun(_, _) | IrType::FnOnceChain(_, _) => "function",
        IrType::Generic(_) => "generic",
        IrType::RowGeneric(_) => "row",
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
        // Ipe.Http.Server.WebSocket opaque handles.
        IrType::WebSocketServer => "WebSocketServer",
        IrType::WebSocketServerCfg => "WebSocketServerCfg",
        IrType::Ui { .. } => "Element",
        IrType::UiPlain(_) => "UiAttribute",
        IrType::WebReq => "WebReq",
        IrType::WebRoute(_) => "WebRoute",
        IrType::CustomElement { .. } => "CustomElement",
        IrType::BackoffStrategy => "BackoffStrategy",
        IrType::Order => "Order",
        IrType::HttpMethod => "HttpMethod",
        IrType::Decimal => "Decimal",
        IrType::ErrorKind => "ErrorKind",
        IrType::Error => "Error",
        IrType::ErrorDetails => "ErrorDetails",
        IrType::ErrorInfo => "ErrorInfo",
        IrType::PanicInfo => "PanicInfo",
        IrType::TypeInfo => "TypeInfo",
        IrType::SqlFragment => "SqlFragment",
        IrType::Secret => "Secret",
        IrType::Path => "Path",
        IrType::Regex => "Regex",
        IrType::CacheCfg => "CacheCfg",
        IrType::CacheStats => "CacheStats",
        IrType::CsvDoc => "Csv",
        IrType::WebSocketClientCfg => "WebSocketCfg",
        IrType::EmailMessage => "EmailMessage",
        IrType::EmailAttachment => "EmailAttachment",
        IrType::EmailSesConfig => "SesConfig",
        IrType::EmailSmtpConfig => "SmtpConfig",
        IrType::EmailProvider => "EmailProvider",
        // `ProcessRunWithCfg` — kernel-boundary non-serde input record; not a web-surface type.
        IrType::ProcessRunWithCfg => "ProcessRunWithCfg",
        // `ProcessRunInPtyCfg` — kernel-boundary non-serde input record; not a web-surface type.
        IrType::ProcessRunInPtyCfg => "ProcessRunInPtyCfg",
        // Typed-key newtypes.
        IrType::CryptoKey => "Key",
        IrType::CryptoMac => "Mac",
        IrType::EmailAddress => "EmailAddress",
        IrType::Url => "Url",
        IrType::Dsn => "Dsn",
        IrType::Connection => "Connection",
        IrType::ConnReadOnly => "ReadOnly",
        IrType::ConnReadWrite => "ReadWrite",
        IrType::Setting => "Setting",
        IrType::ShapeWeb => "Web",
        IrType::ShapeWebView => "WebView",
        IrType::ShapeTerminal => "Terminal",
        IrType::Locale => "Locale",
        IrType::Principal => "Principal",
        IrType::AuthConfig => "AuthConfig",
        IrType::TokenSource => "TokenSource",
        IrType::WebApp => "WebApp",
        IrType::WebViewApp => "WebViewApp",
        IrType::TuiApp => "TuiApp",
        IrType::CliApp => "CliApp",
    }
}

#[cfg(test)]
mod schema_tag_tests {
    use std::collections::BTreeMap;

    use ipe_diagnostics::DResult;
    use ipe_intern::Interner;
    use ipe_ir::{Callee, Expr, Func, FuncId, IrType, KernelFn, ModPath, Module, Program, UiCtor};

    use crate::emit_types::GenericScope;
    use crate::{DbDriver, EmitCtx};

    fn fn_item(interner: &mut Interner, id: u32, name: &str) -> DResult<Func> {
        Ok(Func {
            id: FuncId::from_raw(id),
            name: interner.intern(name)?,
            home: ModPath(vec![]),
            type_params: vec![],
            row_params: vec![],
            params: vec![],
            ret: IrType::Int,
            body: Expr::Int(0),
        })
    }

    fn func_value(id: u32, ty: IrType) -> Expr {
        Expr::FuncValue {
            callee: Callee::Func(FuncId::from_raw(id)),
            ty,
        }
    }

    /// The `{ init, update, view, subscriptions }` config record a single-page
    /// TEA program passes to `web_app`, over the given `model` type. `syms` are
    /// the field symbols in declaration order (init, update, view, subscriptions).
    fn single_page_web_cfg(model: &IrType, syms: [ipe_intern::Symbol; 4]) -> Expr {
        let [init_sym, update_sym, view_sym, subs_sym] = syms;
        let cmd_int = || IrType::Cmd(Box::new(IrType::Int));
        let pair = || IrType::Tuple(vec![model.clone(), cmd_int()]);
        Expr::Record {
            ty: None,
            fields: vec![
                (
                    init_sym,
                    func_value(0, IrType::Fun(vec![IrType::WebReq], Box::new(pair()))),
                ),
                (
                    update_sym,
                    func_value(
                        1,
                        IrType::Fun(vec![IrType::Int, model.clone()], Box::new(pair())),
                    ),
                ),
                (
                    view_sym,
                    func_value(
                        2,
                        IrType::Fun(
                            vec![model.clone()],
                            Box::new(IrType::Ui {
                                ctor: UiCtor::Html,
                                msg: Box::new(IrType::Int),
                            }),
                        ),
                    ),
                ),
                (
                    subs_sym,
                    func_value(
                        3,
                        IrType::Fun(
                            vec![model.clone()],
                            Box::new(IrType::Sub(Box::new(IrType::Int))),
                        ),
                    ),
                ),
            ],
        }
    }

    /// The emitted `web_app(...)` call carries the compile-time Model schema
    /// tag: a `const IPE_WEB_MODEL_SCHEMA_TAG: [u8; 32] = [...]` declaration
    /// plus that identifier as the call's new final argument (H24 — the
    /// session store rejects a checkpoint whose tag differs BEFORE
    /// deserializing it).
    #[test]
    fn web_app_emits_schema_tag_const_and_final_argument() -> DResult<()> {
        let mut interner = Interner::new();
        let init_fn = fn_item(&mut interner, 0, "init")?;
        let update_fn = fn_item(&mut interner, 1, "update")?;
        let view_fn = fn_item(&mut interner, 2, "view")?;
        let subs_fn = fn_item(&mut interner, 3, "subscriptions")?;
        let init_sym = init_fn.name;
        let update_sym = update_fn.name;
        let view_sym = view_fn.name;
        let subs_sym = subs_fn.name;
        let count = interner.intern("count")?;
        let main_mod = interner.intern("Main")?;

        let program = Program {
            imports_unsafe_submodule: false,
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![init_fn, update_fn, view_fn, subs_fn],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_http: false,
                uses_config: false,
                uses_compression: false,
                uses_csv: false,
                uses_cache: false,
                uses_encoding: false,
                uses_regex: false,
                uses_uuid: false,
                uses_random: false,
                uses_log: false,
                uses_decimal: false,
                uses_char_category: false,
                uses_crypto_core: false,
                uses_secret: false,
                uses_json: false,
                uses_crypto: false,
                uses_jwt: false,
                uses_url: false,
                uses_ui: false,
                uses_web: true,
                uses_tui: false,
                uses_console: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_principal: false,
                uses_websocket: false,
                uses_email: false,
                uses_locale: false,
                uses_time: false,
                uses_env_public: false,
                uses_debug: false,
                uses_ffi: false,
                uses_async_runtime: false,
            }],
        };
        let ctx = EmitCtx::build(
            &interner,
            &program,
            DbDriver::Sqlite,
            None,
            ipe_ir::Target::Native,
            Vec::new(),
            false,
            None,
            false,
            String::new(),
            false,
        )?;

        // Model = { count : Int } (no `page` field → the single-page branch).
        let model = IrType::Record(BTreeMap::from([(count, IrType::Int)]));
        let cfg = single_page_web_cfg(&model, [init_sym, update_sym, view_sym, subs_sym]);

        let out = super::emit_web_call(
            &ctx,
            &Callee::Kernel(KernelFn::WebApp),
            &[cfg],
            0,
            0,
            GenericScope::new(&[]),
        )?
        .expect("WebApp must emit");

        assert!(
            out.contains("const IPE_WEB_MODEL_SCHEMA_TAG: [u8; 32] = ["),
            "the emission must declare the schema-tag const, got:\n{out}"
        );
        assert!(
            out.contains("IPE_WEB_MODEL_SCHEMA_TAG)"),
            "the emitted web_app call must pass the tag identifier as its \
             final argument, got:\n{out}"
        );
        assert!(
            out.contains("ipe_runtime::web::web_app("),
            "single-page cfg must still route to web_app, got:\n{out}"
        );
        Ok(())
    }
}

/// Dev appearance hot-swap emit conformance (perf Step 1, style slice).
///
/// The load-bearing property: emitting a `view` whose style-value literals are
/// routed through a per-view `LiteralTable` (flag ON) must render the same as
/// the same view emitted with direct literals (flag OFF) — the baked defaults
/// are exactly the source values, so a `__ipe_lit.get(N)` read is
/// indistinguishable from the direct literal (dev == prod). A mis-tag would
/// surface as a table default that differs from the source literal, caught here.
#[cfg(test)]
mod hot_appearance_tests {
    use ipe_diagnostics::DResult;
    use ipe_intern::Interner;
    use ipe_ir::{
        CallPin, Callee, Expr, Func, FuncId, IrType, KernelFn, ModPath, Module, OnFormKind,
        Program, UiCtor,
    };

    use crate::RustBackend;

    /// A single-module program holding one function `view` whose body is `body`.
    /// `uses_web` is set so the emit runs in the web shape the hot-swap targets
    /// (the `LiteralTable` is a web-runtime type).
    fn one_view_program(interner: &mut Interner, body: Expr) -> DResult<(Program, Func)> {
        let view = Func {
            id: FuncId::from_raw(0),
            name: interner.intern("view")?,
            home: ModPath(vec![]),
            type_params: vec![],
            row_params: vec![],
            params: vec![],
            ret: IrType::Ui {
                ctor: UiCtor::UiAttribute,
                msg: Box::new(IrType::Int),
            },
            body,
        };
        let module = Module {
            name: ModPath(vec![interner.intern("Main")?]),
            types: vec![],
            funcs: vec![view.clone()],
            entry: None,
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
            uses_cache: false,
            uses_encoding: false,
            uses_regex: false,
            uses_uuid: false,
            uses_random: false,
            uses_log: false,
            uses_decimal: false,
            uses_char_category: false,
            uses_crypto_core: false,
            uses_secret: false,
            uses_json: false,
            uses_crypto: false,
            uses_jwt: false,
            uses_url: false,
            uses_ui: true,
            uses_web: true,
            uses_tui: false,
            uses_console: false,
            uses_webview: false,
            uses_css: false,
            uses_auth: false,
            uses_principal: false,
            uses_websocket: false,
            uses_email: false,
            uses_locale: false,
            uses_time: false,
            uses_env_public: false,
            uses_debug: false,
            uses_ffi: false,
            uses_async_runtime: false,
        };
        let program = Program {
            imports_unsafe_submodule: false,
            modules: vec![module],
        };
        Ok((program, view))
    }

    /// Emit the lone `view` function of `program` with the hot-appearance flag
    /// set to `hot`.
    fn emit_view(
        interner: &Interner,
        program: &Program,
        view: &Func,
        hot: bool,
    ) -> DResult<String> {
        let backend = RustBackend::new(interner).with_hot_appearance(hot);
        let ctx = backend.emit_ctx_for_tests(program)?;
        crate::emit_expr::emit_func(&ctx, view)
    }

    /// `Font.family "monospace"` — a single hoist-eligible style literal.
    fn font_family_call() -> Expr {
        Expr::Call {
            callee: Callee::Kernel(KernelFn::FontFamily),
            args: vec![Expr::Str("monospace".to_string())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    /// With the flag OFF the emit is the direct-literal form: the source string
    /// appears inline and no `LiteralTable` is introduced.
    #[test]
    fn flag_off_emits_direct_literal() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(&mut interner, font_family_call())?;
        let out = emit_view(&interner, &program, &view, false)?;
        assert!(
            out.contains("\"monospace\".to_string()"),
            "flag-off emit must carry the direct string literal, got:\n{out}"
        );
        assert!(
            !out.contains("__ipe_lit"),
            "flag-off emit must introduce no literal table, got:\n{out}"
        );
        Ok(())
    }

    /// With the flag ON the literal is hoisted: a per-view table bakes the
    /// source value as its default and the call site reads `__ipe_lit.get(0)`.
    #[test]
    fn flag_on_hoists_style_literal_into_table() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(&mut interner, font_family_call())?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            out.contains(
                "let __ipe_lit = ipe_runtime::web::LiteralTable::from_defaults(&[\"monospace\"]);"
            ),
            "flag-on emit must bake the source value as the table default, got:\n{out}"
        );
        assert!(
            out.contains("__ipe_lit.get(0).to_string()"),
            "the hoisted call site must read its table slot, got:\n{out}"
        );
        assert!(
            !out.contains("ui_font_family_(\"monospace\""),
            "the direct literal must be replaced by the table read, got:\n{out}"
        );
        Ok(())
    }

    /// Conformance: the flag-ON table default, spliced back into the direct-emit
    /// position, reproduces the flag-OFF emission byte-for-byte. This is the
    /// dev == prod guarantee at the emit level — the baked default IS the source
    /// literal, so reading it renders identically. A mis-tagged literal (a
    /// default that differed from the source) would break this equality.
    #[test]
    fn baked_default_matches_direct_literal_bytes() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(&mut interner, font_family_call())?;
        let off = emit_view(&interner, &program, &view, true)?;
        // The default array renders each literal with the same `{:?}` escaping a
        // direct `Expr::Str` uses, so the baked default's bytes equal the direct
        // literal's bytes.
        let direct = format!("{:?}", "monospace");
        assert!(
            off.contains(&format!("from_defaults(&[{direct}])")),
            "the baked default must be byte-identical to the direct literal \
             {direct}, got:\n{off}"
        );
        Ok(())
    }

    /// `Ui.style "color" "red"` — both String positions are style values, so
    /// both hoist, into consecutive table slots, defaults in emit order.
    #[test]
    fn ui_style_hoists_both_positions() -> DResult<()> {
        let mut interner = Interner::new();
        let call = Expr::Call {
            callee: Callee::Kernel(KernelFn::UiStyle),
            args: vec![Expr::Str("color".to_string()), Expr::Str("red".to_string())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        let (program, view) = one_view_program(&mut interner, call)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            out.contains(
                "let __ipe_lit = ipe_runtime::web::LiteralTable::from_defaults(&[\"color\", \"red\"]);"
            ),
            "both style-string positions must bake as ordered defaults, got:\n{out}"
        );
        assert!(
            out.contains("__ipe_lit.get(0).to_string()")
                && out.contains("__ipe_lit.get(1).to_string()"),
            "both positions must read their table slots, got:\n{out}"
        );
        Ok(())
    }

    /// The literal hoists even when the style call is nested deep inside another
    /// call's argument list (the real view shape) — not only at the body's top.
    #[test]
    fn nested_style_literal_still_hoists() -> DResult<()> {
        use ipe_ir::CallPin as P;
        let mut interner = Interner::new();
        // `ui_node_(desc, [Font.family "monospace"], [])` — the style call sits
        // inside a list that is an argument to an outer kernel call.
        let list_of_attr = Expr::List {
            elem: IrType::Ui {
                ctor: UiCtor::UiAttribute,
                msg: Box::new(IrType::Int),
            },
            items: vec![font_family_call()],
        };
        let empty_children = Expr::List {
            elem: IrType::Ui {
                ctor: UiCtor::Element,
                msg: Box::new(IrType::Int),
            },
            items: vec![],
        };
        let body = Expr::Call {
            callee: Callee::Kernel(KernelFn::UiNode),
            args: vec![
                Expr::Call {
                    callee: Callee::Kernel(KernelFn::UiDescNone),
                    args: vec![],
                    pin: P::None,
                    on_form: OnFormKind::NotForm,
                },
                list_of_attr,
                empty_children,
            ],
            pin: P::None,
            on_form: OnFormKind::NotForm,
        };
        let (program, view) = one_view_program(&mut interner, body)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            out.contains("from_defaults(&[\"monospace\"])"),
            "a nested style literal must still hoist, got:\n{out}"
        );
        assert!(
            out.contains("__ipe_lit.get(0).to_string()"),
            "the nested hoisted site must read its table slot, got:\n{out}"
        );
        Ok(())
    }

    /// A non-web shape never hoists (the `LiteralTable` is a web-runtime type):
    /// the emit stays the direct-literal form even with the flag on.
    #[test]
    fn non_web_shape_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let (mut program, view) = one_view_program(&mut interner, font_family_call())?;
        // Flip the shape to a non-web build.
        let module = program
            .modules
            .first_mut()
            .expect("the one-view program has a module");
        module.uses_web = false;
        module.uses_tui = true;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a non-web shape must not hoist, got:\n{out}"
        );
        assert!(
            out.contains("\"monospace\".to_string()"),
            "a non-web shape keeps the direct literal, got:\n{out}"
        );
        Ok(())
    }

    // ── Typed style values (Int / Float — padding, colour channels) ───────────

    /// `Ui.padding 16` — a single hoist-eligible typed `Int` style literal.
    fn padding_call(n: i64) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(KernelFn::UiPadding),
            args: vec![Expr::Int(n)],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    /// A `padding` literal with the flag OFF emits the direct `i64` argument and
    /// no table — byte-identical to today's typed-style emit.
    #[test]
    fn typed_padding_flag_off_emits_direct_int() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(&mut interner, padding_call(16))?;
        let out = emit_view(&interner, &program, &view, false)?;
        assert!(
            out.contains("ui_padding_(16i64)"),
            "flag-off emit must carry the direct i64 literal, got:\n{out}"
        );
        assert!(
            !out.contains("__ipe_lit"),
            "flag-off emit must introduce no literal table, got:\n{out}"
        );
        Ok(())
    }

    /// With the flag ON a `padding` literal hoists: the table bakes the canonical
    /// decimal string and the call site reads it back via a total `parse` whose
    /// fallback is the original literal — so the built `Attribute` and its CSS are
    /// identical to the direct emit (dev == prod), and no patch can panic.
    #[test]
    fn typed_padding_flag_on_hoists_and_reparses() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(&mut interner, padding_call(16))?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            out.contains(
                "let __ipe_lit = ipe_runtime::web::LiteralTable::from_defaults(&[\"16\"]);"
            ),
            "flag-on emit must bake the canonical decimal string as the default, got:\n{out}"
        );
        assert!(
            out.contains("ui_padding_(__ipe_lit.get(0).parse::<i64>().unwrap_or(16i64))"),
            "the hoisted padding must read its slot and re-parse with the literal fallback, \
             got:\n{out}"
        );
        Ok(())
    }

    /// Conformance: the flag-ON baked default is the canonical decimal string of
    /// the source `Int`, and the `unwrap_or` fallback is the same literal — both
    /// parse to the identical `i64`, so reading the slot yields the exact value
    /// the direct emit passed. A mis-tag would surface as a default or fallback
    /// that differs from the source literal, caught here.
    #[test]
    fn typed_padding_baked_default_matches_source_int() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(&mut interner, padding_call(12))?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            out.contains("from_defaults(&[\"12\"])"),
            "the baked default must be the source int's canonical string, got:\n{out}"
        );
        assert!(
            out.contains(".unwrap_or(12i64)"),
            "the total fallback must be the source literal, got:\n{out}"
        );
        Ok(())
    }

    /// `Ui.rgb 255 0 0` — every colour channel is a hoist-eligible `Int`, so all
    /// three hoist into consecutive slots, canonical strings in emit order, each
    /// read back with its own literal fallback.
    #[test]
    fn typed_rgb_channels_all_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let call = Expr::Call {
            callee: Callee::Kernel(KernelFn::UiRgb),
            args: vec![Expr::Int(255), Expr::Int(0), Expr::Int(0)],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        let (program, view) = one_view_program(&mut interner, call)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            out.contains(
                "let __ipe_lit = ipe_runtime::web::LiteralTable::from_defaults(&[\"255\", \"0\", \"0\"]);"
            ),
            "all three colour channels must bake as ordered canonical strings, got:\n{out}"
        );
        let expected = "ui_rgb_(\
            __ipe_lit.get(0).parse::<i64>().unwrap_or(255i64), \
            __ipe_lit.get(1).parse::<i64>().unwrap_or(0i64), \
            __ipe_lit.get(2).parse::<i64>().unwrap_or(0i64))";
        assert!(
            out.contains(expected),
            "each channel must read its slot and re-parse with the literal fallback, got:\n{out}"
        );
        Ok(())
    }

    // ── Refusal gaps (guardian-flagged on Step 1): the constant-only + closure
    //    fences must hold for the typed path too. ──────────────────────────────

    /// A `Model`-dependent typed style value — `Ui.padding` applied to a bound
    /// variable rather than a literal — is logic, not data. It must NOT hoist:
    /// only a direct `Expr::Int` is constant, so the `Var` argument emits
    /// directly and the table stays empty.
    #[test]
    fn model_dependent_typed_style_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let amount = interner.intern("amount")?;
        let call = Expr::Call {
            callee: Callee::Kernel(KernelFn::UiPadding),
            args: vec![Expr::Var(amount)],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        let (program, view) = one_view_program(&mut interner, call)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a Model-dependent (non-literal) style value must not hoist, got:\n{out}"
        );
        Ok(())
    }

    /// A typed style literal inside a `move` lambda body must NOT hoist into the
    /// outer view's table: the closure captures `__ipe_lit` by move, so hoisting
    /// there would bind a table the enclosing scope never introduced. The closure
    /// fence suppresses hoisting for the whole lambda body; the literal emits
    /// directly.
    #[test]
    fn typed_style_inside_move_lambda_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let ignored = interner.intern("_evt")?;
        // `\_evt -> Ui.padding 16` — the style literal lives in the lambda body.
        let body = Expr::Lambda {
            params: vec![(ignored, IrType::Int)],
            ret: IrType::Ui {
                ctor: UiCtor::UiAttribute,
                msg: Box::new(IrType::Int),
            },
            body: Box::new(padding_call(16)),
        };
        let (program, view) = one_view_program(&mut interner, body)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a style literal inside a move closure must not hoist into the outer table, got:\n{out}"
        );
        assert!(
            out.contains("ui_padding_(16i64)"),
            "the fenced literal must emit directly, got:\n{out}"
        );
        Ok(())
    }

    /// A non-web shape never hoists the typed path either (the `LiteralTable` is a
    /// web-runtime type): a `padding` literal stays the direct `i64` emit.
    #[test]
    fn typed_style_non_web_shape_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let (mut program, view) = one_view_program(&mut interner, padding_call(16))?;
        let module = program
            .modules
            .first_mut()
            .expect("the one-view program has a module");
        module.uses_web = false;
        module.uses_tui = true;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a non-web shape must not hoist the typed path, got:\n{out}"
        );
        assert!(
            out.contains("ui_padding_(16i64)"),
            "a non-web shape keeps the direct i64 literal, got:\n{out}"
        );
        Ok(())
    }

    // ── Widened surface: attribute values, static text, Html, Css (Step 5/5b) ──
    //
    // Every kind below is a `String`-valued appearance position marked in the
    // single `appearance_literal_args` registry. The hoist and read shape is the
    // same one the style-string surface uses (`get(N).to_string()`); the runtime
    // helper escapes/sanitises the passed `String` at render, identically for a
    // direct or a hoisted literal, so the baked default renders byte-identically
    // to the direct emit (dev == prod). Each kind gets: a flag-off direct-literal
    // assertion, a flag-on hoist+conformance assertion, and — where the value can
    // be `Model`-dependent — a refusal that a non-literal does not hoist.

    /// A single-`String`-arg kernel whose only argument is a literal `value`.
    fn str_arg_call(k: KernelFn, value: &str) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(k),
            args: vec![Expr::Str(value.to_string())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    /// A two-`String`-arg kernel (`key`, `value`) — the generic-attribute shape.
    fn key_value_call(k: KernelFn, key: &str, value: &str) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(k),
            args: vec![Expr::Str(key.to_string()), Expr::Str(value.to_string())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    /// Assert that `body` hoists exactly `defaults` (in order) with the flag ON,
    /// and emits none of them inline with the flag OFF — the flag-off/flag-on
    /// byte-identical + conformance pair, for a `String`-valued surface.
    fn assert_str_hoist(
        interner: &Interner,
        program: &Program,
        view: &Func,
        defaults: &[&str],
        direct_off_marker: &str,
    ) -> DResult<()> {
        let off = emit_view(interner, program, view, false)?;
        assert!(
            !off.contains("__ipe_lit"),
            "flag-off emit must introduce no literal table, got:\n{off}"
        );
        assert!(
            off.contains(direct_off_marker),
            "flag-off emit must carry the direct literal {direct_off_marker:?}, got:\n{off}"
        );
        let on = emit_view(interner, program, view, true)?;
        let baked = defaults
            .iter()
            .map(|d| format!("{d:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(
            on.contains(&format!("from_defaults(&[{baked}])")),
            "flag-on emit must bake {defaults:?} as ordered defaults (dev == prod), got:\n{on}"
        );
        for slot in 0..defaults.len() {
            assert!(
                on.contains(&format!("__ipe_lit.get({slot}).to_string()")),
                "slot {slot} must be read back as a String, got:\n{on}"
            );
        }
        Ok(())
    }

    /// `Ui.text "Hello"` — a static text node's content hoists as a string value.
    #[test]
    fn ui_text_node_hoists() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) =
            one_view_program(&mut interner, str_arg_call(KernelFn::UiText, "Hello"))?;
        assert_str_hoist(&interner, &program, &view, &["Hello"], "ui_text_(\"Hello\"")
    }

    /// A static text literal with a character that HTML-escapes (`<`) still bakes
    /// verbatim: the runtime `ui_text_` escapes the passed `String` at render, so
    /// a direct and a hoisted `"a < b"` render identically (baked == direct).
    #[test]
    fn ui_text_escaping_baked_equals_direct() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) =
            one_view_program(&mut interner, str_arg_call(KernelFn::UiText, "a < b"))?;
        let on = emit_view(&interner, &program, &view, true)?;
        // The baked default is the raw source string, byte-identical to the direct
        // `Expr::Str` emit; escaping happens once, inside the runtime helper.
        assert!(
            on.contains(&format!("from_defaults(&[{:?}])", "a < b")),
            "the text default must be the raw source string (escaping is a runtime \
             concern), got:\n{on}"
        );
        Ok(())
    }

    /// A `Model`-dependent text node — `Ui.text` applied to a bound variable —
    /// is logic, not data, and must NOT hoist.
    #[test]
    fn model_dependent_text_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let label = interner.intern("label")?;
        let call = Expr::Call {
            callee: Callee::Kernel(KernelFn::UiText),
            args: vec![Expr::Var(label)],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        let (program, view) = one_view_program(&mut interner, call)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a Model-dependent text node must not hoist, got:\n{out}"
        );
        Ok(())
    }

    /// `Ui.name "widget-id"` — an attribute *value* hoists.
    #[test]
    fn ui_name_attr_value_hoists() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) =
            one_view_program(&mut interner, str_arg_call(KernelFn::UiName, "widget-id"))?;
        assert_str_hoist(
            &interner,
            &program,
            &view,
            &["widget-id"],
            "ui_name_(\"widget-id\"",
        )
    }

    /// `Ui.htmlAttribute "class" "card"` — only the *value* (position 1) hoists;
    /// the *key* (position 0) is structural and stays inline.
    #[test]
    fn ui_html_attribute_hoists_value_not_key() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(
            &mut interner,
            key_value_call(KernelFn::UiHtmlAttribute, "class", "card"),
        )?;
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            on.contains(&format!("from_defaults(&[{:?}])", "card")),
            "only the attribute value must bake as a default, got:\n{on}"
        );
        assert!(
            on.contains("ui_html_attribute_(\"class\".to_string(), __ipe_lit.get(0).to_string())"),
            "the key must stay a direct literal and the value read its slot, got:\n{on}"
        );
        assert!(
            !on.contains(&format!("from_defaults(&[{:?}, {:?}])", "class", "card"))
                && !on.contains(&format!("from_defaults(&[{:?}", "class")),
            "the attribute key must never be hoisted, got:\n{on}"
        );
        Ok(())
    }

    /// `Html.text "Hi"` — an `Ipe.Html` static text node hoists.
    #[test]
    fn html_text_node_hoists() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) =
            one_view_program(&mut interner, str_arg_call(KernelFn::HtmlTextNode, "Hi"))?;
        assert_str_hoist(
            &interner,
            &program,
            &view,
            &["Hi"],
            "html_text_node_(\"Hi\"",
        )
    }

    /// `Html.titleNode "Home"` — the `<title>` text hoists.
    #[test]
    fn html_title_node_hoists() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) =
            one_view_program(&mut interner, str_arg_call(KernelFn::HtmlTitleNode, "Home"))?;
        assert_str_hoist(
            &interner,
            &program,
            &view,
            &["Home"],
            "html_title_node_(\"Home\"",
        )
    }

    /// `Attr.attribute "data-x" "1"` — the `Ipe.Html` generic attribute hoists
    /// only its *value* (position 1); the *key* (position 0) stays inline.
    #[test]
    fn html_attribute_hoists_value_not_key() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(
            &mut interner,
            key_value_call(KernelFn::HtmlAttribute, "data-x", "1"),
        )?;
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            on.contains(&format!("from_defaults(&[{:?}])", "1")),
            "only the attribute value must bake as a default, got:\n{on}"
        );
        assert!(
            on.contains("html_named_attr_(\"data-x\".to_string(), __ipe_lit.get(0).to_string())"),
            "the key must stay a direct literal and the value read its slot, got:\n{on}"
        );
        Ok(())
    }

    /// `Html.styleNode [] ".card color red"` — the inline CSS *body* (position 1)
    /// hoists; the attribute-list argument (position 0) is not a literal.
    #[test]
    fn html_style_node_body_hoists() -> DResult<()> {
        let mut interner = Interner::new();
        let empty_attrs = Expr::List {
            elem: IrType::Ui {
                ctor: UiCtor::HtmlAttribute,
                msg: Box::new(IrType::Int),
            },
            items: vec![],
        };
        let call = Expr::Call {
            callee: Callee::Kernel(KernelFn::HtmlStyleNode),
            args: vec![empty_attrs, Expr::Str(".card color red".to_string())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        let (program, view) = one_view_program(&mut interner, call)?;
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            on.contains(&format!("from_defaults(&[{:?}])", ".card color red")),
            "the inline CSS body must bake as a default, got:\n{on}"
        );
        assert!(
            on.contains("__ipe_lit.get(0).to_string()"),
            "the CSS body must read its slot, got:\n{on}"
        );
        Ok(())
    }

    /// `Ipe.Css` is deferred: its value sanitizer `CssSafety.safeValue` is a
    /// `Pure` kernel emitted through the generic kernel-call path, not this
    /// UI-plan hoist site, so it does NOT hoist here (it recompiles) — and the
    /// registry has no dead arm for it. The selector sanitizer stays out
    /// permanently (a selector is structure, not appearance). Wiring the generic
    /// path is a distinct guardian-gated follow-up.
    #[test]
    fn css_kernels_are_not_appearance_hoist_consumers() -> DResult<()> {
        let mut interner = Interner::new();
        let (program, view) = one_view_program(
            &mut interner,
            str_arg_call(KernelFn::CssSafetySafeValue, "16px"),
        )?;
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            !on.contains("__ipe_lit"),
            "the Css value sanitizer must not hoist through the UI-plan path (deferred), got:\n{on}"
        );
        assert!(
            crate::emit_ui_plan::appearance_literal_args(KernelFn::CssSafetySafeValue).is_empty(),
            "the Css value sanitizer must carry no dead registry arm"
        );
        assert!(
            crate::emit_ui_plan::appearance_literal_args(KernelFn::CssSafetySafeSelector)
                .is_empty(),
            "the Css selector sanitizer must never be an appearance-hoist consumer"
        );
        Ok(())
    }

    /// The widened `String` surface stays fenced by the same closure guard as the
    /// style surface: an attribute value inside a `move` lambda must NOT hoist
    /// into the outer view's table.
    #[test]
    fn widened_value_inside_move_lambda_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let ignored = interner.intern("_evt")?;
        let body = Expr::Lambda {
            params: vec![(ignored, IrType::Int)],
            ret: IrType::Ui {
                ctor: UiCtor::UiAttribute,
                msg: Box::new(IrType::Int),
            },
            body: Box::new(str_arg_call(KernelFn::UiName, "x")),
        };
        let (program, view) = one_view_program(&mut interner, body)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a value inside a move closure must not hoist into the outer table, got:\n{out}"
        );
        Ok(())
    }

    /// The widened surface never hoists in a non-web shape (the `LiteralTable` is
    /// a web-runtime type): a `Ui.text` literal stays the direct string emit.
    #[test]
    fn widened_value_non_web_shape_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let (mut program, view) =
            one_view_program(&mut interner, str_arg_call(KernelFn::UiText, "Hello"))?;
        let module = program
            .modules
            .first_mut()
            .expect("the one-view program has a module");
        module.uses_web = false;
        module.uses_tui = true;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a non-web shape must not hoist the widened surface, got:\n{out}"
        );
        assert!(
            out.contains("ui_text_(\"Hello\".to_string())"),
            "a non-web shape keeps the direct string literal, got:\n{out}"
        );
        Ok(())
    }

    // ── Record-native cfg fields: Ui.image { src, description } ────────────────
    //
    // `Ui.image` is a record-native kernel: it builds its call from an inline
    // `{ src, description }` config through `emit_cfg_record_call`, NOT the
    // positional-hoist path, so its appearance fields are named in the companion
    // `appearance_literal_record_fields` registry. Both fields are inert `<img>`
    // attribute values (`src=`, `alt=`), escaped identically at render, so a direct
    // literal in either hoists as a `Str` and reads its slot byte-identically to the
    // direct emit (dev == prod). The same fences hold: `Model`-dependent field →
    // recompile; non-web shape → no table; inside a `move` lambda → no hoist.

    /// A `Ui.image [<attrs>] { src = <src>, description = <description> }` call.
    /// `attrs` is an empty attribute list; `src`/`description` are arbitrary field
    /// value expressions so a test can supply a literal or a `Model`-dependent one.
    fn image_call(
        src_sym: ipe_intern::Symbol,
        src: Expr,
        desc_sym: ipe_intern::Symbol,
        description: Expr,
    ) -> Expr {
        let attrs = Expr::List {
            elem: IrType::Ui {
                ctor: UiCtor::UiAttribute,
                msg: Box::new(IrType::Int),
            },
            items: vec![],
        };
        let cfg = Expr::Record {
            ty: None,
            fields: vec![(src_sym, src), (desc_sym, description)],
        };
        Expr::Call {
            callee: Callee::Kernel(KernelFn::UiImage),
            args: vec![attrs, cfg],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    /// Both `src` and `description` literals hoist, into consecutive slots in field
    /// order (`src` first). Conformance: the baked defaults are the raw source
    /// strings and each field local reads its slot as a `String`, so a prod build
    /// (never patched) renders exactly as the direct emit.
    #[test]
    fn ui_image_src_and_description_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let src_sym = interner.intern("src")?;
        let desc_sym = interner.intern("description")?;
        let body = image_call(
            src_sym,
            Expr::Str("a.png".to_string()),
            desc_sym,
            Expr::Str("alt text".to_string()),
        );
        let (program, view) = one_view_program(&mut interner, body)?;
        let off = emit_view(&interner, &program, &view, false)?;
        assert!(
            !off.contains("__ipe_lit"),
            "flag-off emit must introduce no literal table, got:\n{off}"
        );
        assert!(
            off.contains("\"a.png\".to_string()") && off.contains("\"alt text\".to_string()"),
            "flag-off emit must carry both direct string literals, got:\n{off}"
        );
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            on.contains(&format!(
                "from_defaults(&[{:?}, {:?}])",
                "a.png", "alt text"
            )),
            "both fields must bake as ordered defaults (src first), got:\n{on}"
        );
        assert!(
            on.contains("__ipe_lit.get(0).to_string()")
                && on.contains("__ipe_lit.get(1).to_string()"),
            "each field must read back its table slot as a String, got:\n{on}"
        );
        Ok(())
    }

    /// A `description` literal with an HTML-escaping char bakes verbatim: the
    /// runtime `ui_image_` sets it as the `alt` attribute value, escaped once at
    /// render, so a direct and a hoisted `"a < b"` render identically (baked ==
    /// direct). The same holds for `src` on this path (no URL/data-URI validation
    /// happens at this emit boundary — the helper only sets the `src` attribute).
    #[test]
    fn ui_image_field_escaping_baked_equals_direct() -> DResult<()> {
        let mut interner = Interner::new();
        let src_sym = interner.intern("src")?;
        let desc_sym = interner.intern("description")?;
        let body = image_call(
            src_sym,
            Expr::Str("a.png".to_string()),
            desc_sym,
            Expr::Str("a < b".to_string()),
        );
        let (program, view) = one_view_program(&mut interner, body)?;
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            on.contains(&format!("from_defaults(&[{:?}, {:?}])", "a.png", "a < b")),
            "the description default must be the raw source string (escaping is a \
             runtime concern), got:\n{on}"
        );
        Ok(())
    }

    /// A `Model`-dependent `src` (a bound variable) is logic, not data: only the
    /// `description` literal hoists, and it lands in slot 0 (the `src` field emits
    /// directly and never occupies a slot).
    #[test]
    fn ui_image_model_dependent_src_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let src_sym = interner.intern("src")?;
        let desc_sym = interner.intern("description")?;
        let dynamic_src = interner.intern("dynSrc")?;
        let body = image_call(
            src_sym,
            Expr::Var(dynamic_src),
            desc_sym,
            Expr::Str("alt text".to_string()),
        );
        let (program, view) = one_view_program(&mut interner, body)?;
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            on.contains(&format!("from_defaults(&[{:?}])", "alt text")),
            "only the literal description must hoist (src is Model-dependent), got:\n{on}"
        );
        assert!(
            on.contains("__ipe_lit.get(0).to_string()"),
            "the description must read slot 0, got:\n{on}"
        );
        Ok(())
    }

    /// A `Model`-dependent `description` (a bound variable) does not hoist: only
    /// the literal `src` does.
    #[test]
    fn ui_image_model_dependent_description_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let src_sym = interner.intern("src")?;
        let desc_sym = interner.intern("description")?;
        let dynamic_desc = interner.intern("dynDesc")?;
        let body = image_call(
            src_sym,
            Expr::Str("a.png".to_string()),
            desc_sym,
            Expr::Var(dynamic_desc),
        );
        let (program, view) = one_view_program(&mut interner, body)?;
        let on = emit_view(&interner, &program, &view, true)?;
        assert!(
            on.contains(&format!("from_defaults(&[{:?}])", "a.png")),
            "only the literal src must hoist (description is Model-dependent), got:\n{on}"
        );
        Ok(())
    }

    /// A non-web shape never hoists the image fields (the `LiteralTable` is a
    /// web-runtime type): both literals stay direct string emits.
    #[test]
    fn ui_image_non_web_shape_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let src_sym = interner.intern("src")?;
        let desc_sym = interner.intern("description")?;
        let body = image_call(
            src_sym,
            Expr::Str("a.png".to_string()),
            desc_sym,
            Expr::Str("alt text".to_string()),
        );
        let (mut program, view) = one_view_program(&mut interner, body)?;
        let module = program
            .modules
            .first_mut()
            .expect("the one-view program has a module");
        module.uses_web = false;
        module.uses_tui = true;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "a non-web shape must not hoist the image fields, got:\n{out}"
        );
        assert!(
            out.contains("\"a.png\".to_string()") && out.contains("\"alt text\".to_string()"),
            "a non-web shape keeps both direct string literals, got:\n{out}"
        );
        Ok(())
    }

    /// An image literal inside a `move` lambda must NOT hoist into the outer view's
    /// table: the closure captures `__ipe_lit` by move, so the closure fence
    /// suppresses hoisting for the whole lambda body; both fields emit directly.
    #[test]
    fn ui_image_inside_move_lambda_does_not_hoist() -> DResult<()> {
        let mut interner = Interner::new();
        let src_sym = interner.intern("src")?;
        let desc_sym = interner.intern("description")?;
        let ignored = interner.intern("_evt")?;
        let image = image_call(
            src_sym,
            Expr::Str("a.png".to_string()),
            desc_sym,
            Expr::Str("alt text".to_string()),
        );
        let body = Expr::Lambda {
            params: vec![(ignored, IrType::Int)],
            ret: IrType::Ui {
                ctor: UiCtor::Element,
                msg: Box::new(IrType::Int),
            },
            body: Box::new(image),
        };
        let (program, view) = one_view_program(&mut interner, body)?;
        let out = emit_view(&interner, &program, &view, true)?;
        assert!(
            !out.contains("__ipe_lit"),
            "an image literal inside a move closure must not hoist into the outer table, got:\n{out}"
        );
        assert!(
            out.contains("\"a.png\".to_string()") && out.contains("\"alt text\".to_string()"),
            "the fenced image fields must emit directly, got:\n{out}"
        );
        Ok(())
    }

    // ── Registry-driven enforcement ───────────────────────────────────────────
    //
    // The two tests below do NOT name kernels: they *iterate* the appearance
    // registry, so every arm is proven by construction. A new arm added to
    // `appearance_literal_args` is auto-covered here — there is no per-kernel
    // hand-written test to forget. The per-kernel tests above stay as readable,
    // pinned examples; these are the completeness net over the *whole* registry.

    use crate::emit_ui_plan::{LitKind, appearance_literal_args};

    /// The synthesised literal for a registry position of the given kind, paired
    /// with the exact `String` the emitter bakes as that slot's default. Str bakes
    /// itself; a typed `Int`/`Float` bakes its canonical decimal string — the read
    /// path re-parses it, so the baked default must equal that canonical form.
    fn arm_literal(kind: LitKind) -> (Expr, String) {
        match kind {
            // A value with a char that HTML-escapes, to prove escaping stays a
            // runtime concern (the baked default is the raw source string).
            LitKind::Str => (Expr::Str("a < b".to_string()), "a < b".to_string()),
            LitKind::Int => (Expr::Int(37), "37".to_string()),
            // A finite float (the emitter fences non-finite floats out of the hoist).
            LitKind::Float => (Expr::Float(0.5), "0.5".to_string()),
        }
    }

    /// Synthesise a minimal `Kernel(k)` call of `k`'s declared arity: each
    /// registry-marked position carries a direct literal of its kind; every other
    /// position carries an inert filler string. The call is the smallest emit that
    /// exercises exactly this kernel's appearance positions, with the baked
    /// defaults expected at each marked slot (in slot order).
    fn synth_arm_call(k: KernelFn, positions: &[(usize, LitKind)]) -> (Expr, Vec<String>) {
        let arity = usize::from(k.def().arity);
        let mut args = Vec::with_capacity(arity);
        let mut baked = Vec::new();
        for i in 0..arity {
            match positions
                .iter()
                .find_map(|&(pos, kind)| (pos == i).then_some(kind))
            {
                Some(kind) => {
                    let (lit, default) = arm_literal(kind);
                    args.push(lit);
                    baked.push(default);
                }
                // An unmarked position is structural (an attribute key, a CSS
                // selector, a list arg): a filler literal keeps the call well-formed
                // for emit and must never itself hoist.
                None => args.push(Expr::Str(format!("__unmarked{i}"))),
            }
        }
        let call = Expr::Call {
            callee: Callee::Kernel(k),
            args,
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        (call, baked)
    }

    /// The registry's wired appearance arms: every `KernelFn` in `ALL` with a
    /// non-empty `appearance_literal_args`. Iterating this is what makes each arm
    /// auto-covered — the registry is the single source the tests read.
    fn wired_appearance_arms() -> Vec<(KernelFn, &'static [(usize, LitKind)])> {
        KernelFn::ALL
            .iter()
            .copied()
            .filter_map(|k| {
                let p = appearance_literal_args(k);
                (!p.is_empty()).then_some((k, p))
            })
            .collect()
    }

    /// Registry-driven completeness/conformance: for EVERY appearance arm, a
    /// minimal synthesised call emits flag-off with no table (direct literal) and
    /// flag-on with each marked position baked as its slot default and read back
    /// from `__ipe_lit` — the baked default is byte-identical to the source literal
    /// (dev == prod). Derived from the registry, so no arm can exist unproven.
    #[test]
    fn every_registry_arm_hoists_and_conforms() -> DResult<()> {
        let arms = wired_appearance_arms();
        assert!(
            !arms.is_empty(),
            "the registry must have wired appearance arms to prove"
        );
        for (k, positions) in arms {
            let mut interner = Interner::new();
            let (call, baked) = synth_arm_call(k, positions);
            let (program, view) = one_view_program(&mut interner, call)?;

            let off = emit_view(&interner, &program, &view, false)?;
            assert!(
                !off.contains("__ipe_lit"),
                "{k:?}: flag-off emit must introduce no literal table, got:\n{off}"
            );

            let on = emit_view(&interner, &program, &view, true)?;
            let defaults = baked
                .iter()
                .map(|d| format!("{d:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            assert!(
                on.contains(&format!("from_defaults(&[{defaults}])")),
                "{k:?}: flag-on emit must bake {baked:?} as ordered slot defaults \
                 (dev == prod), got:\n{on}"
            );
            for slot in 0..baked.len() {
                assert!(
                    on.contains(&format!("__ipe_lit.get({slot})")),
                    "{k:?}: marked slot {slot} must read its table entry, got:\n{on}"
                );
            }
        }
        Ok(())
    }

    /// Registry-driven refusal: for EVERY appearance arm, neither a `Model`-
    /// dependent argument at a marked position nor a literal inside a `move` lambda
    /// hoists. The first proves only a *direct literal* is data (a bound `Var` is
    /// logic → recompile); the second proves the closure fence holds arm-wide. Both
    /// iterate the registry, so a new arm is auto-fenced.
    #[test]
    fn every_registry_arm_refuses_model_dependent_and_lambda() -> DResult<()> {
        let arms = wired_appearance_arms();
        for (k, positions) in arms {
            // (a) A `Model`-dependent (bound `Var`) arg at EVERY marked position is
            // logic, not data — none of them may hoist, so the table stays absent.
            // Every marked slot carries the `Var` (not just one) to prove a
            // multi-position arm refuses at each of its positions independently.
            {
                let mut interner = Interner::new();
                let dynamic = interner.intern("__model_value")?;
                let arity = usize::from(k.def().arity);
                let args = (0..arity)
                    .map(|i| {
                        if positions.iter().any(|&(pos, _)| pos == i) {
                            Expr::Var(dynamic)
                        } else {
                            Expr::Str(format!("__unmarked{i}"))
                        }
                    })
                    .collect();
                let call = Expr::Call {
                    callee: Callee::Kernel(k),
                    args,
                    pin: CallPin::None,
                    on_form: OnFormKind::NotForm,
                };
                let (program, view) = one_view_program(&mut interner, call)?;
                let out = emit_view(&interner, &program, &view, true)?;
                assert!(
                    !out.contains("__ipe_lit"),
                    "{k:?}: a Model-dependent arg at a marked position must not hoist, got:\n{out}"
                );
            }
            // (b) The whole call inside a `move` lambda body: the closure fence
            // suppresses hoisting into the outer view's table.
            {
                let mut interner = Interner::new();
                let ignored = interner.intern("_evt")?;
                let (call, _) = synth_arm_call(k, positions);
                let body = Expr::Lambda {
                    params: vec![(ignored, IrType::Int)],
                    ret: IrType::Ui {
                        ctor: UiCtor::UiAttribute,
                        msg: Box::new(IrType::Int),
                    },
                    body: Box::new(call),
                };
                let (program, view) = one_view_program(&mut interner, body)?;
                let out = emit_view(&interner, &program, &view, true)?;
                assert!(
                    !out.contains("__ipe_lit"),
                    "{k:?}: a literal inside a move lambda must not hoist into the outer table, \
                     got:\n{out}"
                );
            }
        }
        Ok(())
    }

    // ── Record-native cfg-field registry: self-enforcing net ───────────────────
    //
    // The companion `appearance_literal_record_fields` registry (keyed by cfg field
    // name, for record-native kernels like `Ui.image`) gets the same construction
    // proof as the positional registry: iterate it, synthesise a call whose marked
    // fields carry direct literals, and prove each hoists + conforms with the flag
    // on and none hoist with it off. A future record-native appearance field is
    // auto-covered — no per-field hand-written test to forget.

    use crate::emit_ui_plan::appearance_literal_record_fields;

    /// The record-native kernels whose cfg records this test knows how to build,
    /// paired with the field-value expression the emitter passes for each named
    /// field. Extending `appearance_literal_record_fields` with a new kernel adds a
    /// row here; the `Str`-only literal shape matches every field kind registered
    /// today.
    fn wired_record_field_arms() -> Vec<(KernelFn, &'static [(&'static str, LitKind)])> {
        KernelFn::ALL
            .iter()
            .copied()
            .filter_map(|k| {
                let fields = appearance_literal_record_fields(k);
                (!fields.is_empty()).then_some((k, fields))
            })
            .collect()
    }

    /// Build the minimal record-native call for `k` with every registered field a
    /// direct `Str` literal, returning the call and the baked defaults (in field
    /// order). Only `Ui.image` is record-native today; a new kernel needs its arm
    /// here so the registry-driven net can synthesise it.
    fn synth_record_field_call(
        interner: &mut Interner,
        k: KernelFn,
        fields: &[(&str, LitKind)],
    ) -> DResult<(Expr, Vec<String>)> {
        let mut baked = Vec::with_capacity(fields.len());
        let mut record_fields = Vec::with_capacity(fields.len());
        for &(name, kind) in fields {
            let sym = interner.intern(name)?;
            let value = format!("__field_{name}");
            // Every record field registered today is `Str`; the record-path hoist
            // only handles `Str` literals. A future non-`Str` field kind needs both
            // a hoist arm in `emit_cfg_record_call` and a distinct synth shape here.
            debug_assert_eq!(kind, LitKind::Str, "only Str record fields are wired");
            record_fields.push((sym, Expr::Str(value.clone())));
            baked.push(value);
        }
        let call = match k {
            KernelFn::UiImage => {
                let attrs = Expr::List {
                    elem: IrType::Ui {
                        ctor: UiCtor::UiAttribute,
                        msg: Box::new(IrType::Int),
                    },
                    items: vec![],
                };
                Expr::Call {
                    callee: Callee::Kernel(k),
                    args: vec![
                        attrs,
                        Expr::Record {
                            ty: None,
                            fields: record_fields,
                        },
                    ],
                    pin: CallPin::None,
                    on_form: OnFormKind::NotForm,
                }
            }
            other => {
                return Err(ipe_diagnostics::Diagnostic::CompilerBug {
                    where_: "emit_web::synth_record_field_call",
                    detail: format!("no record-native call builder for {other:?}"),
                });
            }
        };
        Ok((call, baked))
    }

    /// Every wired record-field arm hoists each registered field and conforms
    /// (baked default == source, byte-identical), and none hoists with the flag off.
    #[test]
    fn every_record_field_arm_hoists_and_conforms() -> DResult<()> {
        let arms = wired_record_field_arms();
        assert!(
            !arms.is_empty(),
            "the record-field registry must have wired arms to prove"
        );
        for (k, fields) in arms {
            let mut interner = Interner::new();
            let (call, baked) = synth_record_field_call(&mut interner, k, fields)?;
            let (program, view) = one_view_program(&mut interner, call)?;

            let off = emit_view(&interner, &program, &view, false)?;
            assert!(
                !off.contains("__ipe_lit"),
                "{k:?}: flag-off emit must introduce no literal table, got:\n{off}"
            );

            let on = emit_view(&interner, &program, &view, true)?;
            let defaults = baked
                .iter()
                .map(|d| format!("{d:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            assert!(
                on.contains(&format!("from_defaults(&[{defaults}])")),
                "{k:?}: flag-on emit must bake {baked:?} as ordered field defaults \
                 (dev == prod), got:\n{on}"
            );
            for slot in 0..baked.len() {
                assert!(
                    on.contains(&format!("__ipe_lit.get({slot}).to_string()")),
                    "{k:?}: field slot {slot} must read its table entry as a String, got:\n{on}"
                );
            }
        }
        Ok(())
    }

    /// Registry-driven refusal for the record-field arms: a `Model`-dependent field
    /// at any marked position does not hoist, and the whole call inside a `move`
    /// lambda does not hoist into the outer table.
    #[test]
    fn every_record_field_arm_refuses_model_dependent_and_lambda() -> DResult<()> {
        for (k, fields) in wired_record_field_arms() {
            // (a) Every marked field a bound `Var` — none may hoist.
            {
                let mut interner = Interner::new();
                let dynamic = interner.intern("__model_value")?;
                let (mut call, _) = synth_record_field_call(&mut interner, k, fields)?;
                // Replace every cfg-record field value with a `Model`-dependent
                // bound `Var` in place — a marked field that is no longer a direct
                // literal must not hoist.
                if let Expr::Call { args, .. } = &mut call
                    && let Some(Expr::Record { fields: rec, .. }) = args.last_mut()
                {
                    for (_, value) in rec.iter_mut() {
                        *value = Expr::Var(dynamic);
                    }
                }
                let (program, view) = one_view_program(&mut interner, call)?;
                let out = emit_view(&interner, &program, &view, true)?;
                assert!(
                    !out.contains("__ipe_lit"),
                    "{k:?}: a Model-dependent field must not hoist, got:\n{out}"
                );
            }
            // (b) The whole call inside a `move` lambda body.
            {
                let mut interner = Interner::new();
                let ignored = interner.intern("_evt")?;
                let (call, _) = synth_record_field_call(&mut interner, k, fields)?;
                let body = Expr::Lambda {
                    params: vec![(ignored, IrType::Int)],
                    ret: IrType::Ui {
                        ctor: UiCtor::Element,
                        msg: Box::new(IrType::Int),
                    },
                    body: Box::new(call),
                };
                let (program, view) = one_view_program(&mut interner, body)?;
                let out = emit_view(&interner, &program, &view, true)?;
                assert!(
                    !out.contains("__ipe_lit"),
                    "{k:?}: a record-field literal inside a move lambda must not hoist, got:\n{out}"
                );
            }
        }
        Ok(())
    }
}
