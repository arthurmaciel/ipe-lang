//! Emission for the `Ipe.Terminal` line-oriented app-entry.
//!
//! * [`KernelFn::TerminalAppLines`] — `Cli.app cfg` →
//!   `ipe_runtime::console_app(init, update, view, subscriptions, on_line)`.
//!   View returns `String` (printed to stdout on each state change).
//!   5-field closed cfg: init / update / view / subscriptions / onLine.
//!
//! # Correctness constraints (MAKE INVALID STATES UNREPRESENTABLE)
//!
//! * All five required cfg fields are looked up with `lookup_field` (fail-closed
//!   on miss — a missing field here is a compiler bug, not user error, because the
//!   constrain scheme already enforces the shape).
//! * `onLine` MUST be present: the runtime calls `on_line(line)` on every stdin
//!   line and returns a `Msg` (not `Option`).  There is no total way to fabricate
//!   a `Msg` without the handler; omitting it would leave `FOnLine` generic
//!   unconstrained (Rust E0282) or produce a runtime-panic/unsound path.
//! * Function fields are emitted via `emit_console_fn` (raw function name for
//!   `FuncValue`, fallback to `emit_expr_at` for lambdas).  A named `fn` item
//!   satisfies `Send + Sync + 'static` via the blanket impl; a `Box<dyn Fn>` does
//!   not without explicit bound annotation.
//! * No store/env plumbing: the Cli runtime reads stdin lines from the OS and has
//!   no session store.

use ipe_diagnostics::{DResult, Diagnostic};
use ipe_ir::{Callee, Expr, KernelFn};

use crate::EmitCtx;
use crate::emit_expr::{callee_name, emit_expr_at};
use crate::emit_types::GenericScope;

/// Dispatch an `Ipe.Terminal` line-oriented kernel call.
///
/// Returns `Some(emitted)` for `TerminalAppLines`; `None` for any other variant
/// (defensive — the caller already guards on `k.is_console()`).
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn emit_console_call(
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
        // ── Cli.app { init, update, view, subscriptions, onLine } ─
        //
        // view : Model -> String
        // Runtime entry: `ipe_runtime::console_app(init, update, view, subs, on_line)`
        KernelFn::TerminalAppLines => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_console_call::TerminalAppLines",
                    detail: format!("Cli.app requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with IPE-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `WebAppRouted` precedent.
            let Expr::Record { fields, .. } = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_console_call::TerminalAppLines",
                    detail: "Cli.app cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with IPE-L0119"
                        .into(),
                });
            };
            emit_console_inner(ctx, fields, indent, child, generics)
        }

        // Any non-Cli kernel variant: let the standard path handle it.
        _ => Ok(None),
    }
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Emit `ipe_runtime::console_app(init, update, view, subs, on_line)`.
///
/// # Function-field emission
///
/// Same discipline as `emit_tui_inner`: named `fn` items are emitted via
/// `emit_console_fn` (raw identifier), which satisfies `Send + Sync + 'static` via
/// the blanket impl.  A `Box<dyn Fn>` (from the fallback `emit_expr_at` path)
/// does NOT carry these bounds without explicit annotation.
fn emit_console_inner(
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
    let on_line_e = lookup_field(ctx, fields, "onLine")?;

    // seal: gate the Model against `console_app`'s `Clone` bound. A
    // non-clonable (non-derivable) Model — a field of type `Cmd`/`Sub`/`Task`/
    // `Decoder`/`Db`/function — would otherwise `ipe`-succeed then
    // `cargo`-fail; the gate makes it a fail-closed `IPE-L0120` error.
    if let Some(model_ty) = crate::emit_model_gate::model_ty_of_view(view_e) {
        crate::emit_model_gate::check_admissible_model(
            ctx,
            model_ty,
            ipe_diagnostics::AppShape::Cli,
        )?;
    }

    let init_s = emit_console_fn(ctx, init_e, indent, child, generics)?;
    let update_s = emit_console_fn(ctx, update_e, indent, child, generics)?;
    let view_s = emit_console_fn(ctx, view_e, indent, child, generics)?;
    let subs_s = emit_console_fn(ctx, subs_e, indent, child, generics)?;
    let on_line_s = emit_console_fn(ctx, on_line_e, indent, child, generics)?;

    Ok(Some(format!(
        "ipe_runtime::tea::CliApp(ipe_runtime::console_app(\
         {init_s}, \
         {update_s}, \
         {view_s}, \
         {subs_s}, \
         {on_line_s}\
         ))"
    )))
}

/// Emit a cfg-field expression for the Cli app-entry kernel.
///
/// Mirrors `emit_tui_fn` exactly: for a named function reference
/// ([`Expr::FuncValue`]), emits the raw callee name (e.g. `Main_on_line`)
/// rather than a boxed closure.  A named function item satisfies
/// `Fn(…) + Send + Sync + 'static` via the compiler's blanket impl; a
/// `Box<dyn Fn(…)>` does NOT carry these bounds without explicit annotation.
///
/// For any other expression (lambda, local variable, etc.) falls back to the
/// general [`emit_expr_at`] emitter.
fn emit_console_fn(
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
        where_: "ipe_backend_rust::emit_console_call",
        detail: format!(
            "required Cli cfg field `{name}` not found; \
             available fields: [{}]",
            fields
                .iter()
                .filter_map(|(s, _)| ctx.resolve_ident(*s).ok())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}
