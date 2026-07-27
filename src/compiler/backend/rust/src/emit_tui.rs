//! Emission for `Ipe.Tui` / `Ipe.Tui` app-entry kernels.
//!
//! Wires the two Tui kernels:
//!
//! * [`KernelFn::TuiApp`] — `Tui.app cfg` → `ipe_runtime::tui::tui_app_ui(…)`.
//!   View returns `Element<Msg>` (the Ipe.Ui typed element tree, rendered to ANSI
//!   cells by the runtime).  5-field cfg (init / update / view / subscriptions /
//!   onKey) with an open row tail for optional fields.
//! * [`KernelFn::TuiProgram`] — `Tui.program cfg` → `ipe_runtime::tui::tui_app(…)`.
//!   View returns `String` (the raw ANSI frame, painted verbatim).  Same 5-field
//!   cfg shape.
//!
//! # `onKey` dispatch bridge
//!
//! The Rust runtime signature is `FOnKey: Fn(String, String) -> Msg` — the two
//! `String` arguments are the key's `kind` and `value` as extracted from the
//! [`ipe_runtime::tui::TuiKey`] struct.
//!
//! Ipê user code writes `onKey : KeyEvent -> Msg` where `KeyEvent` is typically a
//! record alias `{ kind : String, value : String }`.  Because `FOnKey` takes two
//! bare `String`s — not a record — the emitter generates a wrapper closure when
//! `on_key_e` is a named function whose first parameter is a CLOSED record of all-
//! `String` fields with `kind` and `value` present:
//!
//! ```text
//! // Ipê source:  onKey : { kind : String, value : String } -> Msg
//! // Emitted:
//! |kind: String, value: String| Main_on_key(RecKindValue { kind, value })
//! ```
//!
//! For records with additional String fields (e.g. `{ ctrl, kind, shift, value }`),
//! the wrapper fills the runtime-supplied `kind` and `value` and initialises every
//! other String field to an empty string.  Non-String fields (Bool, Int, …) are
//! initialised to their zero value.  This matches what the Go runtime does via
//! reflection: fields not present in the runtime key-event struct receive their
//! Go zero value.
//!
//! If the record's argument is not a `FuncValue` (i.e., `onKey` is a lambda or a
//! local variable), the standard [`emit_tui_fn`] fallback applies; `cargo` will
//! then check type compatibility.
//!
//! # Correctness constraints (MAKE INVALID STATES UNREPRESENTABLE)
//!
//! * `onKey` MUST be present: the runtime calls `on_key(kind, value)` on every key
//!   event and returns a `Msg` (not `Option`).  There is no total way to fabricate
//!   a `Msg` without the handler; omitting it would leave `FOnKey` generic
//!   unconstrained (Rust E0282) or produce a runtime-panic/unsound path.
//! * Function fields are emitted via [`emit_tui_fn`] (raw function name for
//!   `FuncValue`, fallback to `emit_expr_at` for lambdas).  A named `fn` item
//!   satisfies `Send + Sync + 'static` via the blanket impl; a `Box<dyn Fn>` does
//!   not without explicit bound annotation.
//! * No store/env plumbing: the Tui runtime reads the terminal size from the OS at
//!   each paint and has no session store.

use std::collections::BTreeMap;

use ipe_diagnostics::{DResult, Diagnostic};
use ipe_ir::{Callee, Expr, IrType, KernelFn};

use crate::EmitCtx;
use crate::emit_expr::{callee_name, emit_expr_at};
use crate::emit_types::GenericScope;

/// Dispatch a `Ipe.Tui` / `Ipe.Tui` kernel call.
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
        // Runtime entry: `ipe_runtime::tui::tui_app_ui(init, update, view, subs, on_key)`
        KernelFn::TuiApp => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_tui_call::TuiApp",
                    detail: format!("Tui.app requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with IPE-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `WebAppRouted` precedent.
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_tui_call::TuiApp",
                    detail: "Tui.app cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with IPE-L0119"
                        .into(),
                });
            };
            emit_tui_inner(ctx, fields, "tui_app_ui", indent, child, generics)
        }

        // ── Tui.program { init, update, view, subscriptions, onKey } ───────
        //
        // view : Model -> String   (raw ANSI frame, painted verbatim)
        // Runtime entry: `ipe_runtime::tui::tui_app(init, update, view, subs, on_key)`
        KernelFn::TuiProgram => {
            let [cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_tui_call::TuiProgram",
                    detail: format!("Tui.program requires 1 argument, got {}", args.len()),
                });
            };
            // Unreachable for well-typed source: a non-literal cfg is rejected
            // at lower with IPE-L0119 (Feature::LetBoundAppCfg); this guard is a
            // defensive invariant, mirroring the `WebAppRouted` precedent.
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_tui_call::TuiProgram",
                    detail: "Tui.program cfg must be an inline record literal; \
                             a non-literal cfg is rejected at lower with IPE-L0119"
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

/// Emit `ipe_runtime::tui::<entry>(init, update, view, subs, on_key)`.
///
/// `entry` is either `"tui_app_ui"` (Element view) or `"tui_app"` (String view).
///
/// # Function-field emission
///
/// Same discipline as `emit_live_app_inner`: named `fn` items are emitted via
/// `emit_tui_fn` (raw identifier), which satisfies `Send + Sync + 'static` via
/// the blanket impl.  A `Box<dyn Fn>` (from the fallback `emit_expr_at` path)
/// does NOT carry these bounds without explicit annotation.
fn emit_tui_inner(
    ctx: &EmitCtx,
    fields: &[(ipe_intern::Symbol, Expr)],
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

    // seal: gate the Model against `tui_app`'s `Clone` bound. A non-clonable
    // (non-derivable) Model — a field of type `Cmd`/`Sub`/`Task`/`Decoder`/`Db`/
    // function — would otherwise `ipe`-succeed then `cargo`-fail; the gate makes
    // it a fail-closed `IPE-L0120` error. (Tui needs only `Clone`, not serde, so
    // an `Html`/`Color` field is admissible here.)
    if let Some(model_ty) = crate::emit_model_gate::model_ty_of_view(view_e) {
        crate::emit_model_gate::check_admissible_model(
            ctx,
            model_ty,
            ipe_diagnostics::AppShape::Tui,
        )?;
    }

    // seal: gate the Msg type against `tui_app`'s Clone+Send bound.
    // Same derivable predicate as Live — Msg is never persisted.
    if let Some(msg_ty) = crate::emit_model_gate::msg_ty_of_update(update_e) {
        crate::emit_model_gate::check_admissible_msg(ctx, msg_ty, ipe_diagnostics::AppShape::Tui)?;
    }

    let init_s = emit_tui_fn(ctx, init_e, indent, child, generics)?;
    let update_s = emit_tui_fn(ctx, update_e, indent, child, generics)?;
    let view_s = emit_tui_fn(ctx, view_e, indent, child, generics)?;
    let subs_s = emit_tui_fn(ctx, subs_e, indent, child, generics)?;
    // `onKey` bridges `FOnKey: Fn(String, String) -> Msg` ↔ user's
    // `onKey : KeyEvent -> Msg`.  A record-taking `FuncValue` is wrapped in a
    // closure that unpacks (kind, value) into the struct literal.
    let on_key_s = emit_tui_on_key(ctx, on_key_e, indent, child, generics)?;

    Ok(Some(format!(
        "ipe_runtime::tui::{entry}(\
         {init_s}, \
         {update_s}, \
         {view_s}, \
         {subs_s}, \
         {on_key_s}\
         )"
    )))
}

/// Emit the `onKey` argument for a Tui app-entry kernel.
///
/// The Rust runtime expects `FOnKey: Fn(String, String) -> Msg` — two bare
/// `String`s for the key's `kind` and `value`.  When the user writes
/// `onKey : KeyEvent -> Msg` (where `KeyEvent` is a record alias), the emitter
/// generates a bridging wrapper:
///
/// ```text
/// |kind: String, value: String| Main_on_key(RecKindValue { kind, value })
/// ```
///
/// The rule for wrapper generation:
///
/// * The expression must be a [`Expr::FuncValue`] (a top-level named function).
///   Lambdas and local variables fall through to the plain [`emit_tui_fn`] path;
///   `cargo` then validates the type compatibility directly.
/// * The first parameter type must be [`IrType::Record`].
/// * The struct is looked up via [`EmitCtx::record_name_for_literal`] (the
///   pre-pass collected every record shape reachable from a signature).
/// * `kind` and `value` fields must be present (both `IrType::Str`); they map
///   directly to the runtime parameters.  Any additional String fields receive
///   `String::new()` as the default; Bool fields receive `false`; Int fields
///   receive `0i64`.  This mirrors the Go runtime's zero-value fill for record
///   fields not supplied by `tuiKeyToIpe`.
///
/// Rationale for the default-fill approach: the Haskell compiler's Go backend
/// handles the `KeyEvent → Msg` bridge via reflection (`IpeCall`), which
/// zero-initialises fields not present in the runtime struct.  The Rust port
/// replicates that contract statically at code-generation time.
fn emit_tui_on_key(
    ctx: &EmitCtx,
    e: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<String> {
    // Named function reference whose first parameter is a record: direct
    // (unboxed) wrapper around the callee name.
    if let Expr::FuncValue { callee, ty } = e
        && let IrType::Fun(params, _ret) = ty
        && let Some(IrType::Record(rec_fields)) = params.first()
    {
        return emit_on_key_record_wrapper(ctx, callee, rec_fields);
    }
    // Lambda whose first parameter is a record: bind the emitted closure and
    // apply it inside the flat wrapper. The `TuiApp` / `TuiProgram` schemes pin
    // `onKey`'s parameter to the closed `{ kind : String, value : String }`
    // record, so a well-typed lambda always lands here — leaving it unwrapped
    // was an exit-0-then-cargo-fail hole (the 1-arg closure broke the
    // runtime's `FOnKey: Fn(String, String) -> Msg` bound with `E0593`).
    if let Expr::Lambda { params, .. } = e
        && let Some((_, IrType::Record(rec_fields))) = params.first()
    {
        let inner = emit_tui_fn(ctx, e, indent, child, generics)?;
        let (struct_name, init_body) = on_key_struct_literal(ctx, rec_fields)?;
        return Ok(format!(
            "{{ let __ipe_on_key = {inner}; \
             move |kind: String, value: String| \
             __ipe_on_key({struct_name} {{ {init_body} }}) }}"
        ));
    }
    // Residual (local var / other fn-typed exprs): plain emission; `cargo`
    // validates compatibility. Reaching here with a record-typed handler value
    // is only possible via a let-bound binding (see the let-bound-cfg gate
    // family).
    emit_tui_fn(ctx, e, indent, child, generics)
}

/// Generate the `|kind: String, value: String| <callee>(<Struct> { … })` wrapper
/// that bridges the runtime's flat `(String, String)` key event to the user's
/// record-typed `onKey` function.
///
/// Field rules (in the struct literal):
///
/// | Field in user's `KeyEvent` | Struct initializer |
/// |---|---|
/// | `"kind"` (must be `String`) | `kind` (closure param) |
/// | `"value"` (must be `String`) | `value` (closure param) |
/// | any other `String` field | `String::new()` |
/// | `Bool` field | `false` |
/// | `Int` field | `0i64` |
/// | unsupported type | returns `Diagnostic::CompilerBug` |
fn emit_on_key_record_wrapper(
    ctx: &EmitCtx,
    callee: &Callee,
    rec_fields: &BTreeMap<ipe_intern::Symbol, IrType>,
) -> DResult<String> {
    let (struct_name, init_body) = on_key_struct_literal(ctx, rec_fields)?;
    let fn_name = callee_name(ctx, callee)?;
    Ok(format!(
        "|kind: String, value: String| {fn_name}({struct_name} {{ {init_body} }})"
    ))
}

/// Resolve the `KeyEvent` record shape to its generated Rust struct name plus
/// the struct-literal body mapping the flat runtime `(kind, value)` params
/// (shared by the named-function and lambda wrapper paths).
fn on_key_struct_literal(
    ctx: &EmitCtx,
    rec_fields: &BTreeMap<ipe_intern::Symbol, IrType>,
) -> DResult<(String, String)> {
    // Resolve all field symbols to (name, IrType) pairs and sort by name so
    // the struct literal matches the pre-pass order used by record_name_for_literal.
    let mut fields: Vec<(String, &IrType)> = rec_fields
        .iter()
        .map(|(sym, ty)| ctx.resolve_ident(*sym).map(|n| (n.to_owned(), ty)))
        .collect::<DResult<_>>()?;
    fields.sort_by(|a, b| a.0.cmp(&b.0));

    // Look up the Rust struct name for this record shape (uses sorted field names).
    let field_names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
    let struct_name = ctx.record_name_for_literal(&field_names)?;

    // Build the struct-literal body, mapping runtime params + zero defaults.
    let mut init_parts: Vec<String> = Vec::with_capacity(fields.len());
    for (name, ty) in &fields {
        let part = match ty {
            IrType::Str if name == "kind" => "kind".to_owned(),
            IrType::Str if name == "value" => "value".to_owned(),
            IrType::Str => format!("{name}: String::new()"),
            IrType::Bool => format!("{name}: false"),
            IrType::Int => format!("{name}: 0i64"),
            other => {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_tui::emit_on_key_record_wrapper",
                    detail: format!(
                        "KeyEvent record field `{name}` has unsupported IR type {other:?}; \
                         the Tui runtime only bridges String, Bool, and Int fields \
                         from the flat (kind, value) key event"
                    ),
                });
            }
        };
        init_parts.push(part);
    }

    Ok((struct_name.to_owned(), init_parts.join(", ")))
}

/// Emit a cfg-field expression for a Tui app-entry kernel.
///
/// Mirrors `emit_live_fn` in `emit_web.rs` exactly: for a named function
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
        where_: "ipe_backend_rust::emit_tui_call",
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
