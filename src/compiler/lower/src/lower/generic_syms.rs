//! Generic-symbol collection over `IrType` and the
//! default-generics-to-unit rewrite used when a type variable is unconstrained.

use std::collections::BTreeSet;

use ipe_intern::Symbol;
use ipe_ir::IrType;

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
#[allow(clippy::too_many_lines)] // one arm per IrType variant, deliberately exhaustive
pub(super) fn collect_ir_generic_syms(ty: &IrType, out: &mut BTreeSet<Symbol>) {
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
        | IrType::WebRoute(inner) => {
            collect_ir_generic_syms(inner, out);
        }
        IrType::Result(a, b)
        | IrType::Dict(a, b)
        | IrType::CustomElement { down: a, up: b } => {
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
        IrType::Fun(params, ret)
        | IrType::SharedFun(params, ret)
        | IrType::FnOnceChain(params, ret) => {
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
        | IrType::BackoffStrategy
        | IrType::Order
        | IrType::HttpMethod
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        // Nominal error-payload leaves (SEAL fix) — monomorphic,
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
        | IrType::Regex
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        | IrType::UiPlain(_)
        | IrType::Decimal
        | IrType::WebReq
        | IrType::SessionHandle
        | IrType::SqlFragment
        | IrType::Secret
        | IrType::Path
        // `Url` is non-parametric — no generic syms.
        | IrType::Url
        // `Dsn` is non-parametric — no generic syms.
        | IrType::Dsn
        | IrType::Connection | IrType::ConnReadOnly | IrType::ConnReadWrite
        | IrType::Setting | IrType::ShapeWeb | IrType::ShapeWebView | IrType::ShapeTerminal
        // Process-run-with cfg + Cache config / stats + Csv document are
        // non-parametric — no generic syms.
        | IrType::ProcessRunWithCfg
        | IrType::ProcessRunInPtyCfg
        | IrType::CacheCfg
        | IrType::WebSocketClientCfg
        | IrType::CacheStats
        | IrType::CsvDoc
        // Ipe.Email records + provider ADT are non-parametric — no generic syms.
        | IrType::EmailMessage
        | IrType::EmailAttachment
        | IrType::EmailSesConfig
        | IrType::EmailSmtpConfig
        | IrType::EmailProvider
        // Typed-key newtypes are non-parametric — no generic syms.
        | IrType::CryptoKey
        | IrType::CryptoMac
        | IrType::EmailAddress
        // `Locale` is non-parametric — no generic syms.
        | IrType::Locale
        // `Principal` is non-parametric — no generic syms.
        | IrType::Principal
        // `AuthConfig` / `TokenSource` are non-parametric — no generic syms.
        | IrType::AuthConfig
        | IrType::TokenSource
        | IrType::WebApp
        | IrType::TuiApp
        | IrType::CliApp
        // A row variable is tracked in `Func::row_params`, NOT in the ordinary
        // `T{n}` generic scope. Collecting it here would double-count it as both
        // a `T`-generic and an `R`-generic, so this arm is a deliberate no-op.
        | IrType::RowGeneric(_) => {}
    }
}

/// Replace every [`IrType::Generic(s)`] whose `s` is in `targets` with
/// [`IrType::Unit`], recursing through every container the type tree can nest a
/// generic under (the same structural shape [`collect_ir_generic_syms`] walks).
///
/// The type-level half of *unconstrained UI-msg defaulting*: a `Html msg` /
/// `Element msg` / `Attribute msg` / `Event msg` message variable the type
/// checker proved is never pinned to a concrete `Msg` -- at the binding or any
/// use (`SolvedTypes::msg_defaulted_vars`) -- has no polymorphic requirement, so
/// *concrete over generic* lowers it to the unit type rather than a Rust generic
/// no caller can instantiate (E0283). Applied to the return type here; the
/// matching body occurrences default to `Unit` on their own because the same
/// variable is withheld from `current_poly_tvars`, so every UI-msg slot lowering
/// routes through [`Lowerer::ir_type_from_ty_ui_msg`]'s free-var to `Unit` arm.
pub(super) fn default_generics_to_unit(ty: IrType, targets: &BTreeSet<Symbol>) -> IrType {
    let recur = |t: IrType| default_generics_to_unit(t, targets);
    let boxed = |t: Box<IrType>| Box::new(default_generics_to_unit(*t, targets));
    match ty {
        IrType::Generic(sym) if targets.contains(&sym) => IrType::Unit,
        IrType::List(elem) => IrType::List(boxed(elem)),
        IrType::Set(elem) => IrType::Set(boxed(elem)),
        IrType::Maybe(inner) => IrType::Maybe(boxed(inner)),
        IrType::Task(inner) => IrType::Task(boxed(inner)),
        IrType::Cmd(inner) => IrType::Cmd(boxed(inner)),
        IrType::Sub(inner) => IrType::Sub(boxed(inner)),
        IrType::Decoder(inner) => IrType::Decoder(boxed(inner)),
        IrType::WebRoute(inner) => IrType::WebRoute(boxed(inner)),
        IrType::Result(e, a) => IrType::Result(boxed(e), boxed(a)),
        IrType::Dict(k, v) => IrType::Dict(boxed(k), boxed(v)),
        IrType::Tuple(elems) => IrType::Tuple(elems.into_iter().map(recur).collect()),
        IrType::Record(fields) => {
            IrType::Record(fields.into_iter().map(|(k, v)| (k, recur(v))).collect())
        }
        IrType::Enum { home, name, args } => IrType::Enum {
            home,
            name,
            args: args.into_iter().map(recur).collect(),
        },
        IrType::Fun(params, ret) => {
            IrType::Fun(params.into_iter().map(recur).collect(), boxed(ret))
        }
        IrType::SharedFun(params, ret) => {
            IrType::SharedFun(params.into_iter().map(recur).collect(), boxed(ret))
        }
        IrType::FnOnceChain(params, ret) => {
            IrType::FnOnceChain(params.into_iter().map(recur).collect(), boxed(ret))
        }
        IrType::Ui { ctor, msg } => IrType::Ui {
            ctor,
            msg: boxed(msg),
        },
        // Every remaining variant is either a non-target `Generic`, a
        // `RowGeneric` (tracked in `Func::row_params`, never a `T{n}`), or a
        // leaf carrying no nested `IrType` -- returned unchanged.
        other => other,
    }
}
