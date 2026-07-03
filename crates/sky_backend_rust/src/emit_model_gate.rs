//! Model-admissibility gate for `Std.Live` / `Std.Tui` / `Std.Webview`
//! app-entry kernels (#91 seal).
//!
//! The app entry threads a **Model** state type through `init` / `update` /
//! `view`. Each runtime entry bounds that Model:
//!
//! * `live_app` — `Model: serde::Serialize + serde::de::DeserializeOwned +
//!   Clone + PartialEq + Send + Sync + 'static` (the Model is persisted to the
//!   session store), so a non-serde Model is inadmissible.
//! * `tui_app` / `webview_app` — `Model: Clone + Send + 'static`, so a
//!   non-`Clone` (i.e. non-derivable) Model is inadmissible.
//!
//! Without this gate a well-typed program storing a `Cmd` / `Sub` / `Task` /
//! `Decoder` / `Db` / function — or, for `Sky.Live`, an `Html` / `Element` /
//! `Color` — in its Model `skyc`-succeeds and then `cargo`-fails on the missing
//! trait bound. This module extracts the Model type from the app cfg's `view`
//! function and, if it fails the required predicate, returns a fail-closed
//! `SKY-L0120` diagnostic naming the offending field — converting the
//! `cargo`-fail into a clean `skyc` error (MAKE INVALID STATES UNREPRESENTABLE).

use sky_diagnostics::{AppShape, Diagnostic, DResult, LowerError, ModelLeaf, Span};
use sky_ir::{ir_type_is_derivable, ir_type_is_serde, Expr, IrType};

use crate::EmitCtx;

/// Extract the Model type from an app cfg's `view` field expression.
///
/// `view : Model -> Html Msg` (Live/Webview) / `Model -> Element Msg` (Tui)
/// lowers to an [`Expr::FuncValue`] whose `ty` is the concrete
/// `IrType::Fun([Model], Ret)`. The Model is the first parameter.
///
/// Returns `None` when the Model type cannot be recovered structurally (e.g. a
/// lambda `view` that did not reify a `Fun` type). Callers treat `None` as
/// "cannot prove inadmissible" and skip the gate — this never *false-blocks* a
/// well-formed program; an inadmissible Model behind an unrecoverable `view`
/// shape simply falls back to the prior behaviour rather than regressing.
#[must_use]
pub fn model_ty_of_view(view_e: &Expr) -> Option<&IrType> {
    match view_e {
        Expr::FuncValue {
            ty: IrType::Fun(params, _),
            ..
        } => params.first(),
        _ => None,
    }
}

/// Gate the Model type of an app entry against the runtime bound `app` requires.
///
/// * [`AppShape::Live`] → the Model must satisfy [`ir_type_is_serde`] (which
///   structurally implies `Clone + PartialEq`, so the full `live_app` bound is
///   covered by this one predicate).
/// * [`AppShape::Tui`] / [`AppShape::Webview`] → the Model must satisfy
///   [`ir_type_is_derivable`] (the backend derives `Clone` iff a type is
///   derivable, and Tui/Webview need only `Clone`).
///
/// On failure returns [`Diagnostic::Lower`] carrying [`LowerError::
/// InadmissibleAppModel`] (`SKY-L0120`) with the offending field and leaf kind.
/// The IR carries no spans at emit, so the span is [`Span::DUMMY`] and the
/// message is self-contained (precedent: the backend's `Span::DUMMY` `Name`
/// diagnostics). Normal plain-data Models pass unchanged.
pub fn check_admissible_model(ctx: &EmitCtx, model_ty: &IrType, app: AppShape) -> DResult<()> {
    let ok = match app {
        AppShape::Live => ir_type_is_serde(model_ty, &|home, name| ctx.enum_is_serde(home, name)),
        AppShape::Tui | AppShape::Webview => {
            ir_type_is_derivable(model_ty, &|home, name| ctx.enum_is_derivable(home, name))
        }
    };
    if ok {
        return Ok(());
    }

    // Inadmissible: locate the offending field + leaf for a precise message.
    let (field, leaf) = blame(ctx, model_ty, app);
    Err(Diagnostic::Lower {
        span: Span::DUMMY,
        msg: LowerError::InadmissibleAppModel {
            app,
            field: field.into_boxed_str(),
            leaf,
        },
    })
}

/// Return whether `ty` satisfies the admissibility predicate for `app`.
fn admissible(ctx: &EmitCtx, ty: &IrType, app: AppShape) -> bool {
    match app {
        AppShape::Live => ir_type_is_serde(ty, &|home, name| ctx.enum_is_serde(home, name)),
        AppShape::Tui | AppShape::Webview => {
            ir_type_is_derivable(ty, &|home, name| ctx.enum_is_derivable(home, name))
        }
    }
}

/// Identify the offending Model field name (empty when the Model is not a
/// record) and the leaf category of the first non-admissible payload.
///
/// Only ever called on a `model_ty` already known to be inadmissible, so a leaf
/// is always found; the `ModelLeaf::Handle` fallback is a defensive default that
/// never fires for a genuinely inadmissible type.
fn blame(ctx: &EmitCtx, model_ty: &IrType, app: AppShape) -> (String, ModelLeaf) {
    if let IrType::Record(fields) = model_ty {
        for (sym, field_ty) in fields {
            if !admissible(ctx, field_ty, app) {
                let name = ctx.resolve_ident(*sym).unwrap_or("").to_owned();
                return (name, leaf_of(ctx, field_ty, app));
            }
        }
    }
    (String::new(), leaf_of(ctx, model_ty, app))
}

/// The leaf category of the first non-admissible leaf reachable from `ty`.
///
/// Recurses through transparent carriers and (via the whole-program variant
/// table) through non-admissible user enums. A depth bound guards against a
/// pathological self-referential enum cycle (the type checker forbids infinite
/// value types, so this is belt-and-braces, never reached in practice).
fn leaf_of(ctx: &EmitCtx, ty: &IrType, app: AppShape) -> ModelLeaf {
    leaf_of_bounded(ctx, ty, app, 64)
}

fn leaf_of_bounded(ctx: &EmitCtx, ty: &IrType, app: AppShape, fuel: u32) -> ModelLeaf {
    if fuel == 0 {
        return ModelLeaf::Handle;
    }
    let next = fuel - 1;
    match ty {
        IrType::Fun(_, _) => ModelLeaf::Function,
        IrType::Cmd(_) => ModelLeaf::Command,
        IrType::Sub(_) => ModelLeaf::Subscription,
        IrType::Task(_) => ModelLeaf::Task,
        IrType::Decoder(_) => ModelLeaf::Decoder,
        IrType::Ui { .. } | IrType::UiPlain(_) => ModelLeaf::ViewValue,
        // Transparent carriers: descend into the first non-admissible child.
        IrType::Maybe(e) | IrType::List(e) | IrType::Set(e) => leaf_of_bounded(ctx, e, app, next),
        IrType::Result(a, b) | IrType::Dict(a, b) => {
            if admissible(ctx, a, app) {
                leaf_of_bounded(ctx, b, app, next)
            } else {
                leaf_of_bounded(ctx, a, app, next)
            }
        }
        IrType::Tuple(es) => es
            .iter()
            .find(|e| !admissible(ctx, e, app))
            .map_or(ModelLeaf::Handle, |e| leaf_of_bounded(ctx, e, app, next)),
        IrType::Record(fields) => fields
            .values()
            .find(|f| !admissible(ctx, f, app))
            .map_or(ModelLeaf::Handle, |f| leaf_of_bounded(ctx, f, app, next)),
        IrType::Enum { home, name, .. } => {
            // A non-admissible user enum: descend into its variant payloads to
            // find the concrete offending leaf.
            for payloads in ctx.enum_variant_payloads(home, *name) {
                for field_ty in payloads {
                    if !admissible(ctx, field_ty, app) {
                        return leaf_of_bounded(ctx, field_ty, app, next);
                    }
                }
            }
            ModelLeaf::Handle
        }
        // The opaque `Db`/server/live handles are `Handle`. The primitive /
        // `Generic` leaves are admissible so `leaf_of` is never called on them
        // as the sole content of an inadmissible type — they share the same
        // defensive `Handle` fallback rather than a panic.
        IrType::Db
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        | IrType::LiveReq
        | IrType::LiveRoute
        | IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        | IrType::Generic(_) => ModelLeaf::Handle,
    }
}
