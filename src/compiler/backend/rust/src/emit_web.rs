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
        KernelFn::WebApp => {
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
            emit_web_app_inner(ctx, fields, indent, child, generics)
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
            emit_web_app_inner(ctx, fields, indent, child, generics)
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
            let Some(app_s) = emit_web_app_inner(ctx, fields, indent, child, generics)? else {
                return Ok(None);
            };
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
/// of emitting E0308). See `misc/docs/divergences-from-sky.md` §B-route-param.
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
            "{{ {tag_const} \
             ipe_runtime::web::web_app_routed(\
             {init_s}, \
             {update_s}, \
             {view_s}, \
             {subs_s}, \
             {routes_s}, \
             {not_found_s}, \
             {set_page}, \
             ::std::env::var(\"IPE_LIVE_STORE\").unwrap_or_else(|_| \"memory\".to_string()), \
             ::std::env::var(\"IPE_LIVE_STORE_PATH\").unwrap_or_else(|_| ::std::string::String::new()), \
             IPE_WEB_MODEL_SCHEMA_TAG\
             ) }}"
        )));
    }

    // Single-page (non-routed) path — `routes`/`notFound` are structurally
    // present in the cfg but not forwarded to the runtime entry.
    //
    // The store kind and path come from env at call time so a single binary can
    // switch stores without recompilation (`IPE_LIVE_STORE` / `IPE_LIVE_STORE_PATH`).
    Ok(Some(format!(
        "{{ {tag_const} \
         ipe_runtime::web::web_app(\
         {init_s}, \
         {update_s}, \
         {view_s}, \
         {subs_s}, \
         ::std::env::var(\"IPE_LIVE_STORE\").unwrap_or_else(|_| \"memory\".to_string()), \
         ::std::env::var(\"IPE_LIVE_STORE_PATH\").unwrap_or_else(|_| ::std::string::String::new()), \
         IPE_WEB_MODEL_SCHEMA_TAG\
         ) }}"
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
/// This is a sanctioned divergence from the Go/Haskell reference, which assumes
/// all route payloads are `String` and silently substitutes zero-values on
/// decode failure. See `misc/docs/divergences-from-sky.md §B-route-param`.
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
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_principal: false,
                uses_websocket: false,
                uses_email: false,
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
