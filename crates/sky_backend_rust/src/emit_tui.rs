//! Emission for `Std.Tui` / `Sky.Tui` app-entry kernels (Phase-1c).
//!
//! Wires the two Tui kernels:
//!
//! * [`KernelFn::TuiApp`] — `Tui.app cfg` → `sky_runtime::tui::tui_app_ui(…)`.
//!   View returns `Element<Msg>` (the Std.Ui typed element tree, rendered to ANSI
//!   cells by the runtime).  5-field closed cfg: init / update / view /
//!   subscriptions / onKey.
//! * [`KernelFn::TuiProgram`] — `Tui.program cfg` → `sky_runtime::tui::tui_app(…)`.
//!   View returns `String` (the raw ANSI frame, painted verbatim).  Same 5-field
//!   cfg shape.
//!
//! # Correctness constraints (MAKE INVALID STATES UNREPRESENTABLE)
//!
//! * All five required cfg fields are looked up with `lookup_field` (fail-closed
//!   on miss — a missing field here is a compiler bug, not user error, because the
//!   constrain scheme already enforces the shape).
//! * `onKey` MUST be present: the runtime calls `on_key(kind, value)` on every key
//!   event and returns a `Msg` (not `Option`).  There is no total way to fabricate
//!   a `Msg` without the handler; omitting it would leave `FOnKey` generic
//!   unconstrained (Rust E0282) or produce a runtime-panic/unsound path.
//! * Function fields are emitted via `emit_live_fn` (raw function name for
//!   `FuncValue`, fallback to `emit_expr_at` for lambdas).  A named `fn` item
//!   satisfies `Send + Sync + 'static` via the blanket impl; a `Box<dyn Fn>` does
//!   not without explicit bound annotation.
//! * No store/env plumbing: the Tui runtime reads the terminal size from the OS at
//!   each paint and has no session store.

use sky_diagnostics::{DResult, Diagnostic};
use sky_ir::{Callee, Expr, KernelFn};

use crate::EmitCtx;
use crate::emit_expr::{callee_name, emit_expr_at};
use crate::emit_types::GenericScope;

/// Dispatch a `Std.Tui` / `Sky.Tui` kernel call.
///
/// Returns `Some(emitted)` for `TuiApp` and `TuiProgram`; `None` for any other
/// variant (defensive — the caller already guards on `k.is_tui()`).
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn emit_tui_call(
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
        // ── Tui.app { init, update, view, subscriptions, onKey } ───────────
        //
        // view : Model -> Element Msg
        // Runtime entry: `sky_runtime::tui::tui_app_ui(init, update, view, subs, on_key)`
        KernelFn::TuiApp => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_tui_call::TuiApp",
                    detail: format!("Tui.app requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with SKY-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `LiveAppRouted` precedent.
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_tui_call::TuiApp",
                    detail: "Tui.app cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with SKY-L0119"
                        .into(),
                });
            };
            emit_tui_inner(ctx, fields, "tui_app_ui", indent, child, generics)
        }

        // ── Tui.program { init, update, view, subscriptions, onKey } ───────
        //
        // view : Model -> String   (raw ANSI frame, painted verbatim)
        // Runtime entry: `sky_runtime::tui::tui_app(init, update, view, subs, on_key)`
        KernelFn::TuiProgram => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_tui_call::TuiProgram",
                    detail: format!("Tui.program requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with SKY-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `LiveAppRouted` precedent.
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_tui_call::TuiProgram",
                    detail: "Tui.program cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with SKY-L0119"
                        .into(),
                });
            };
            emit_tui_inner(ctx, fields, "tui_app", indent, child, generics)
        }

        // Any non-Tui kernel variant: let the standard path handle it.
        _ => Ok(None),
    }
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Emit `sky_runtime::tui::<entry>(init, update, view, subs, on_key)`.
///
/// `entry` is either `"tui_app_ui"` (Element view) or `"tui_app"` (String view).
///
/// # Function-field emission
///
/// Same discipline as `emit_live_app_inner`: named `fn` items are emitted via
/// `emit_live_fn` (raw identifier), which satisfies `Send + Sync + 'static` via
/// the blanket impl.  A `Box<dyn Fn>` (from the fallback `emit_expr_at` path)
/// does NOT carry these bounds without explicit annotation.
fn emit_tui_inner(
    ctx: &EmitCtx,
    fields: &[(sky_intern::Symbol, Expr)],
    entry: &str,
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
    let on_key_e = lookup_field(ctx, fields, "onKey")?;

    let init_s = emit_tui_fn(ctx, init_e, indent, child, generics)?;
    let update_s = emit_tui_fn(ctx, update_e, indent, child, generics)?;
    let view_s = emit_tui_fn(ctx, view_e, indent, child, generics)?;
    let subs_s = emit_tui_fn(ctx, subs_e, indent, child, generics)?;
    let on_key_s = emit_tui_fn(ctx, on_key_e, indent, child, generics)?;

    Ok(Some(format!(
        "sky_runtime::tui::{entry}(\
         {init_s}, \
         {update_s}, \
         {view_s}, \
         {subs_s}, \
         {on_key_s}\
         )"
    )))
}

/// Emit a cfg-field expression for a Tui app-entry kernel.
///
/// Mirrors `emit_live_fn` in `emit_live.rs` exactly: for a named function
/// reference ([`Expr::FuncValue`]), emits the raw callee name (e.g.
/// `Main_on_key`) rather than a boxed closure.  A named function item satisfies
/// `Fn(…) + Send + Sync + 'static` via the compiler's blanket impl; a
/// `Box<dyn Fn(…)>` does NOT carry these bounds without explicit annotation.
///
/// For any other expression (lambda, local variable, etc.) falls back to the
/// general [`emit_expr_at`] emitter.
fn emit_tui_fn(
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

/// Find a record field by its Sky source name in an IR field list.
///
/// Fail-closed: a missing required field surfaces a [`Diagnostic::CompilerBug`]
/// rather than silently emitting wrong code (MAKE INVALID STATES UNREPRESENTABLE).
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
        where_: "sky_backend_rust::emit_tui_call",
        detail: format!(
            "required Tui cfg field `{name}` not found; \
             available fields: [{}]",
            fields
                .iter()
                .filter_map(|(s, _)| ctx.resolve_ident(*s).ok())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}
