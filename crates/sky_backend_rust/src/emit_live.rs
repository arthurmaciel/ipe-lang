//! Emission for `Std.Live` / `Sky.Live` app-entry kernels (Phase-1b).
//!
//! Wires three of the four Live kernels:
//!
//! * [`KernelFn::LiveApp`] — `Live.app cfg` → `sky_runtime::live::live_app(…)`.
//!   Only the 4-field cfg scheme (init / update / view / subscriptions) is
//!   supported.  `Live.appRouted` is gated at lower (SKY-L0118) before it
//!   reaches this module.
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
//! * `Live.appRouted` is unreachable here: the lower gate (SKY-L0118) rejects it
//!   before the emit stage; the arm returns a `CompilerBug` as a defensive
//!   invariant check.

use sky_diagnostics::{DResult, Diagnostic};
use sky_ir::{Callee, Expr, KernelFn};

use crate::EmitCtx;
use crate::emit_expr::emit_expr_at;
use crate::emit_types::GenericScope;

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
        // ── Live.app { init, update, view, subscriptions } ─────────────────
        //
        // Only the 4-field non-routed cfg scheme is wired here.
        // `Live.appRouted` is gated at lower (SKY-L0118) and must never reach
        // this point; the `LiveAppRouted` arm below is a defensive invariant.
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

        // ── Live.appRouted — gated at lower (SKY-L0118) ────────────────────
        //
        // This arm is a defensive invariant: the lower stage rejects
        // `Live.appRouted` with `Feature::RoutedLiveApp` before IR is
        // produced, so a `LiveAppRouted` call here is a compiler bug, not user
        // error.
        KernelFn::LiveAppRouted => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_live_call::LiveAppRouted",
            detail: "Live.appRouted reached emit stage — should have been \
                     rejected at lower with SKY-L0118 (Feature::RoutedLiveApp)"
                .into(),
        }),

        // ── Live.route pattern ctor ─────────────────────────────────────────
        //
        // `Live.route : String -> page -> LiveRoute`  (#106)
        //
        // The second argument's Sky type is a bare polymorphic `page`, so it is
        // either:
        //   * A nullary ctor expression (`HomePage`) — lowered by the Sky compiler
        //     as a ctor-ref (no params), so the emit wraps it in `|_params| ctor`.
        //   * A partially-applied ctor function or lambda — emit as a generic
        //     `move |params| (builder_s)(params)` call.
        //
        // Ctor-ref detection: `Expr::Ctor` with zero declared payload fields.
        // Any other expression is wrapped generically via a closure.
        KernelFn::LiveRoute => {
            let [pattern_e, builder_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_live_call::LiveRoute",
                    detail: format!("Live.route requires 2 arguments, got {}", args.len()),
                });
            };
            let pattern_s = emit_expr_at(ctx, pattern_e, indent, child, generics)?;

            // Detect whether `builder_e` is a ctor reference.  If so, determine
            // how many payload fields the variant has so we can emit the right
            // `params.get(i)` chain.
            let build_closure = if let Expr::Ctor {
                home,
                ty,
                variant,
                args: ctor_args,
            } = builder_e
            {
                let field_count = ctx.variant_fields(home, *ty, *variant)?.len();
                let ctor_s = emit_expr_at(ctx, builder_e, indent, child, generics)?;
                if field_count == 0 || !ctor_args.is_empty() {
                    // Nullary ctor or fully-applied ctor: wrap as a constant closure.
                    format!("move |_params: ::std::vec::Vec<::std::string::String>| {ctor_s}")
                } else {
                    // Ctor with N payload fields but zero args supplied — it's
                    // a partial-application stub: the user wrote `AppDetailPage`
                    // (not `AppDetailPage "x"`) so the runtime must supply the
                    // captured strings.
                    let param_gets: Vec<String> = (0..field_count)
                        .map(|i| format!("params.get({i}).cloned().unwrap_or_default()"))
                        .collect();
                    // `ctor_s` here is the variant path (e.g. `MainPage::AppDetailPage`),
                    // so we call it with the captured strings.
                    let ctor_name = emit_expr_at(ctx, builder_e, indent, child, generics)?;
                    format!(
                        "move |params: ::std::vec::Vec<::std::string::String>| \
                         {ctor_name}({})",
                        param_gets.join(", ")
                    )
                }
            } else {
                // Generic function / lambda — call it with the params vec.
                let builder_s = emit_expr_at(ctx, builder_e, indent, child, generics)?;
                format!(
                    "move |params: ::std::vec::Vec<::std::string::String>| \
                     ({builder_s})(params)"
                )
            };

            Ok(Some(format!(
                "sky_runtime::live::route::Route::new({pattern_s}, {build_closure})"
            )))
        }

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

    let init_s = emit_live_fn(ctx, init_e, indent, child, generics)?;
    let update_s = emit_live_fn(ctx, update_e, indent, child, generics)?;
    let view_s = emit_live_fn(ctx, view_e, indent, child, generics)?;
    let subs_s = emit_live_fn(ctx, subs_e, indent, child, generics)?;

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
