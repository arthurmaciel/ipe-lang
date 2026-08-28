//! Emission for `Ipe.WebView` / `Ipe.WebView` app-entry kernel.
//!
//! Wires one Webview kernel:
//!
//! * [`KernelFn::WebViewApp`] — `Webview.app cfg` →
//!   `ipe_runtime::webview::webview_app(init, update, view, subs, window_cfg)`.
//!   5-field closed cfg: init / update / view / subscriptions / window,
//!   where `window = { title : String, size : (Int, Int) }`.
//!
//! # Correctness constraints (MAKE INVALID STATES UNREPRESENTABLE)
//!
//! * All five required cfg fields are looked up fail-closed (missing field →
//!   [`Diagnostic::CompilerBug`], not silent drop).
//! * **G4**: `window` MUST be an inline `Expr::Record` AND `size` within it MUST
//!   be an inline 2-element `Expr::Tuple`. Any non-literal shape is rejected at
//!   lower with `IPE-L0119` (`Feature::LetBoundAppCfg`); these emit-site guards
//!   are unreachable-by-construction defensive invariants (defence-in-depth,
//!   mirroring the `WebAppRouted`/`IPE-L0118` precedent).
//! * Function fields (init/update/view/subscriptions) are emitted via
//!   `emit_webview_fn` (raw function name for `FuncValue`, fallback to
//!   `emit_expr_at`). A named `fn` item satisfies `Send + Sync + 'static` via
//!   the blanket impl; `Box<dyn Fn>` does not without explicit bound annotation.
//! * The `fn main` entry uses `ipe_main().run_blocking()` for shape app
//!   entries. For `WebViewApp`, `run_blocking()` internally calls
//!   `block_on_current_thread`, satisfying tao/Cocoa's requirement that the
//!   event loop runs on the process main thread (hard `NSApplication`
//!   requirement on macOS). This switch is performed in `project.rs`
//!   (`emit_program` / `emit_spine`) via an anchor-asserted `replacen-once`
//!   that aborts with `CompilerBug` on zero-match.

use ipe_diagnostics::{DResult, Diagnostic};
use ipe_ir::{Callee, Expr, KernelFn};

use crate::EmitCtx;
use crate::emit_expr::{callee_name, emit_expr_at};
use crate::emit_types::GenericScope;
use crate::emit_web::wrap_view;

/// Dispatch a `Ipe.WebView` kernel call.
///
/// Returns `Some(emitted)` for `WebViewApp`; `None` for any other variant
/// (defensive — the caller already guards on `k.is_webview()`).
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn emit_webview_call(
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
        // ── Webview.app { init, update, view, subscriptions, window } ──────
        //
        // view : Model -> Element Msg — the framework applies `Ui.layout`
        //   internally, unifying the graphical shapes on `Element`. Raw HTML is
        //   reached through the `Ui.html` node inside this `Element` view.
        // window : { title : String, size : (Int, Int) }
        // Runtime entry: `ipe_runtime::webview::webview_app(init, update, view, subs, window)`
        KernelFn::WebViewApp => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_webview_call::WebviewApp",
                    detail: format!("Webview.app requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with IPE-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `WebAppRouted` precedent.
            let Expr::Record { fields, .. } = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_webview_call::WebviewApp",
                    detail: "Webview.app cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with IPE-L0119"
                        .into(),
                });
            };
            emit_webview_app_inner(ctx, fields, indent, child, generics)
        }

        // Any non-WebView kernel variant: let the standard path handle it.
        _ => Ok(None),
    }
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Emit `ipe_runtime::webview::webview_app(init, update, view, subs, window)`.
///
/// **G4 gate — fail-closed on two levels:**
/// 1. `window` MUST be an inline `Expr::Record` literal.
/// 2. `size` within the window record MUST be an inline 2-element `Expr::Tuple`
///    literal `(w, h)`.
///
/// `title` may be any String-typed expression (variable, literal, concatenation).
/// Both checks emit [`Diagnostic::CompilerBug`] on failure — they are unreachable
/// for well-typed source: a non-literal `window`/`size` is rejected at lower with
/// IPE-L0119 (`Feature::LetBoundAppCfg`), so these guards are defensive
/// invariants, mirroring the `WebAppRouted` precedent.
///
/// # Function-field emission
///
/// Same discipline as `emit_web_app_inner` / `emit_tui_inner`: named `fn` items
/// are emitted via `emit_webview_fn` (raw identifier) to satisfy
/// `Send + Sync + 'static` via the blanket impl. A `Box<dyn Fn>` does not carry
/// these bounds without explicit annotation.
fn emit_webview_app_inner(
    ctx: &EmitCtx,
    fields: &[(ipe_intern::Symbol, Expr)],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    // All five fields are required — fail-closed on any miss (compiler bug, not
    // user error: the constrain scheme enforces the 5-field shape upstream).
    let init_e = lookup_field(ctx, fields, "init")?;
    let update_e = lookup_field(ctx, fields, "update")?;
    let view_e = lookup_field(ctx, fields, "view")?;
    let subs_e = lookup_field(ctx, fields, "subscriptions")?;
    let window_e = lookup_field(ctx, fields, "window")?;

    // seal: gate the Model against `webview_app`'s `Clone` bound (same as
    // Tui — memory-resident Model, `Clone` only). A non-clonable (non-derivable)
    // Model becomes a fail-closed `IPE-L0120` error instead of a `cargo` fail.
    if let Some(model_ty) = crate::emit_model_gate::model_ty_of_view(view_e) {
        crate::emit_model_gate::check_admissible_model(
            ctx,
            model_ty,
            ipe_diagnostics::AppShape::WebView,
        )?;
    }

    // seal: gate the Msg type against `webview_app`'s Clone+Send bound.
    // Same derivable predicate as Web/Tui — Msg is never persisted.
    if let Some(msg_ty) = crate::emit_model_gate::msg_ty_of_update(update_e) {
        crate::emit_model_gate::check_admissible_msg(
            ctx,
            msg_ty,
            ipe_diagnostics::AppShape::WebView,
        )?;
    }

    // ── G4 gate 1: `window` must be an inline record literal ─────────────────
    // Unreachable for well-typed source: a let-bound `window` is rejected at lower
    // with IPE-L0119 (Feature::LetBoundAppCfg); this guard is a defensive invariant.
    let Expr::Record {
        fields: win_fields, ..
    } = window_e
    else {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_webview_app_inner::G4_window",
            detail: "Webview.app `window` field must be an inline record literal \
                     `{ title = ..., size = (..., ...) }`; \
                     a let-bound WindowCfg variable is rejected at lower with IPE-L0119"
                .into(),
        });
    };

    let title_e = lookup_field(ctx, win_fields, "title")?;
    let size_e = lookup_field(ctx, win_fields, "size")?;

    // ── G4 gate 2: `size` must be an inline 2-element tuple literal ──────────
    // Unreachable for well-typed source: a let-bound `size` is rejected at lower
    // with IPE-L0119 (Feature::LetBoundAppCfg); this guard is a defensive invariant.
    let Expr::Tuple(size_elems) = size_e else {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_webview_app_inner::G4_size",
            detail: "Webview.app `window.size` must be an inline 2-tuple literal `(w, h)`; \
                     a let-bound size variable is rejected at lower with IPE-L0119"
                .into(),
        });
    };
    let [w_e, h_e] = size_elems.as_slice() else {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_webview_app_inner::G4_size_arity",
            detail: format!(
                "Webview.app `window.size` must be a 2-tuple `(Int, Int)` (width, height), \
                 but got a {}-element tuple",
                size_elems.len()
            ),
        });
    };

    let init_s = emit_webview_fn(ctx, init_e, indent, child, generics)?;
    let update_s = emit_webview_fn(ctx, update_e, indent, child, generics)?;
    let view_raw_s = emit_webview_fn(ctx, view_e, indent, child, generics)?;
    // `WebView.app`'s `view : Model -> Element Msg` is wrapped with `Ui.layout`
    // (framework-applied). Same unification point as `Web.app` (see
    // `emit_web::wrap_view`).
    let view_s = wrap_view(&view_raw_s);
    let subs_s = emit_webview_fn(ctx, subs_e, indent, child, generics)?;
    // `title` may be any String-typed expression.
    let title_s = emit_expr_at(ctx, title_e, indent, child, generics)?;
    // `size` tuple elements: (w, h) — both Int-typed.
    let w_s = emit_expr_at(ctx, w_e, indent, child, generics)?;
    let h_s = emit_expr_at(ctx, h_e, indent, child, generics)?;

    Ok(Some(format!(
        "ipe_runtime::tea::WebViewApp(ipe_runtime::webview::webview_app(\
         {init_s}, \
         {update_s}, \
         {view_s}, \
         {subs_s}, \
         ipe_runtime::webview::WebViewWindowCfg {{ title: {title_s}, size: ({w_s}, {h_s}) }}\
         ))"
    )))
}

/// Emit a cfg-field expression for a Webview app-entry kernel.
///
/// Mirrors `emit_web_fn` (`emit_web.rs`) and `emit_tui_fn` (`emit_tui.rs`)
/// exactly: for a named function reference ([`Expr::FuncValue`]), emits the raw
/// callee name (e.g. `Main_init`) rather than a boxed closure. A named function
/// item satisfies `Fn(…) + Send + Sync + 'static` via the compiler's blanket impl.
///
/// For any other expression (lambda, local variable, etc.) falls back to the
/// general [`emit_expr_at`] emitter.
fn emit_webview_fn(
    ctx: &EmitCtx,
    e: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<String> {
    if let Expr::FuncValue { callee, .. } = e {
        // Raw function-item reference: satisfies Send + Sync + 'static implicitly.
        return callee_name(ctx, callee);
    }
    emit_expr_at(ctx, e, indent, child, generics)
}

/// Find a record field by its Ipê source name in an IR field list.
///
/// Fail-closed: a missing required field surfaces a [`Diagnostic::CompilerBug`]
/// rather than silently emitting wrong code (MAKE INVALID STATES UNREPRESENTABLE).
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
        where_: "ipe_backend_rust::emit_webview_call",
        detail: format!(
            "required Webview cfg field `{name}` not found; \
             available fields: [{}]",
            fields
                .iter()
                .filter_map(|(s, _)| ctx.resolve_ident(*s).ok())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}
