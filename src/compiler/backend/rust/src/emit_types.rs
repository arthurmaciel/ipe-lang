//! Type emission: user enums and their `IpeStringify` impls, plus
//! IR-type → Rust-type rendering.
//!
//! Ports the relevant arms of `Ipê/Generate/Rust/Builder/TypeEmitter.hs`
//! (`unionToRustTypeDef`) and `Emitter.hs` (`typeDefToString` / the enum
//! `ipeStringifyEnumImpl`). The byte target is golden `main.rs` lines 31–43.

use ipe_diagnostics::{DResult, Diagnostic};
use ipe_intern::Symbol;
use ipe_ir::{EnumDef, IrType, UiCtor, UiPlain, ir_type_is_derivable};

use crate::doc::Doc;
use crate::naming::mangle_reserved;
use crate::render::{RenderConfig, render_seeded};
use crate::{EmitCtx, RecordStruct};

/// Render a `format!(<fmt>, <arg>, …)` `IpeStringify` body natively, laid out
/// exactly as `rustfmt --edition 2024 --style-edition 2024` would — the record
/// `ipe_show` body and the payload-variant enum arm.
///
/// `rustfmt` keeps the whole call on one line when its argument text (the span
/// between `format!(` and the closing `)`) fits `fn_call_width` (60) AND the full
/// line fits `max_width` (100); otherwise it breaks one argument per line
/// (WITHOUT a trailing comma, as it does for every macro call), each argument at
/// one block-indent step past `block_indent`, the `)` dedented back to
/// `block_indent`. This is precisely a [`Doc::CallArgs`] with
/// `trailing_comma == false`, so the shared renderer reproduces the decision by
/// construction.
///
/// `open_col` is the column the `format!` token lands on — used only for the
/// single-line fit test — and `block_indent` is the indent a broken line nests
/// from: for the record body both are 8 (two block-indent levels deep in
/// `ipe_show`); for an enum arm `open_col` is the column after the `… => ` arm
/// head while `block_indent` is the arm's own indent (12). The returned string
/// carries no leading spaces for `open_col` — the caller has already written the
/// prefix on that line — and every broken line carries its own absolute indent.
fn render_stringify_format(
    fmt_literal: &str,
    args: &[String],
    open_col: usize,
    block_indent: usize,
) -> String {
    let elems: Vec<Doc> = std::iter::once(Doc::owned(fmt_literal.to_owned()))
        .chain(args.iter().map(|a| Doc::owned(a.clone())))
        .collect();
    let call = Doc::call_args(
        Doc::text("format!("),
        elems,
        Doc::text(")"),
        // A macro argument list breaks without a trailing comma.
        false,
    );
    render_seeded(&call, RenderConfig::default(), block_indent, open_col)
}

/// `rustfmt`'s `max_width` (default 100): the widest line the formatter leaves
/// unbroken.
const MAX_WIDTH: usize = 100;

/// `rustfmt`'s `fn_call_width` (default 60): the widest a call's argument text may
/// be and still lay out flat between its delimiters.
const FN_CALL_WIDTH: usize = 60;

/// Render a payload-variant enum arm's `Pat => format!(…)` tail exactly as
/// `rustfmt` lays it out, returning the whole arm line(s) including the arm head
/// and the trailing `,`.
///
/// `rustfmt` decides in three tiers, in order:
///
///   * INLINE — the whole `arm_head format!(…),` fits `max_width` and the
///     argument text fits `fn_call_width`: one line.
///   * BLOCK-WRAP — the call's argument text fits `fn_call_width` (so the call
///     itself does not want to break its delimiters) but the inline arm overflows
///     `max_width`: `rustfmt` wraps the body in a synthesized brace block —
///     `Pat => {\n    format!(…)\n}` (no trailing comma) — with the `format!` on
///     its own line at the arm indent + 4, where it fits.
///   * DELIMITER-BREAK — the argument text itself exceeds `fn_call_width`: the
///     `format!` breaks one argument per line in place after the `=> `, the `)`
///     dedented to the arm indent, and the arm keeps its trailing `,`.
///
/// `arm_head` is the `            {Enum}::{Variant}(binders) => ` prefix at block
/// indent 12; `fmt_literal` and `args` are the `format!` operands.
fn render_stringify_enum_arm(arm_head: &str, fmt_literal: &str, args: &[String]) -> String {
    const ARM_INDENT: usize = 12;
    let open_col = arm_head.chars().count();

    // The argument text between `format!(` and `)` — `rustfmt`'s `fn_call_width`
    // span — and the full inline arm line width.
    let inline_args = args.join(", ");
    let args_text = if args.is_empty() {
        fmt_literal.to_owned()
    } else {
        format!("{fmt_literal}, {inline_args}")
    };
    let args_width = args_text.chars().count();
    let inline_call = format!("format!({args_text})");
    let inline_arm_width = open_col + inline_call.chars().count() + 1; // +1 for the `,`

    if args_width <= FN_CALL_WIDTH && inline_arm_width <= MAX_WIDTH {
        // INLINE.
        format!("{arm_head}{inline_call},")
    } else if args_width <= FN_CALL_WIDTH {
        // BLOCK-WRAP: the call fits `fn_call_width` but the inline arm overflows,
        // so `rustfmt` brace-wraps the body. The `format!` lands at arm indent + 4.
        let body_indent = " ".repeat(ARM_INDENT + 4);
        let close_indent = " ".repeat(ARM_INDENT);
        format!("{arm_head}{{\n{body_indent}{inline_call}\n{close_indent}}}")
    } else {
        // DELIMITER-BREAK: the argument text exceeds `fn_call_width`, so the
        // `format!` breaks one argument per line after `=> `; the arm keeps `,`.
        let call = render_stringify_format(fmt_literal, args, open_col, ARM_INDENT);
        format!("{arm_head}{call},")
    }
}

/// The generic-type-parameter scope in effect while emitting one function's
/// signature and body.
///
/// Maps a Ipê type-variable [`Symbol`] to its deterministic Rust generic name
/// (`T1`, `T2`, …) by the variable's *position* in the function's quantification
/// order — never by the symbol's spelling — so a function quantifying `[a, b]`
/// renders `a` → `T1` and `b` → `T2` regardless of source naming. Empty for
/// monomorphic functions and for program-level emission (enums, record structs),
/// where no generic is in scope.
///
/// For `UiLayout`/`UiLayoutWith`, M is inferred bottom-up from the concrete
/// element/attrs types sourced from `SolvedTypes.regions`, not threaded down
/// from the enclosing function's `Html<M>` return type.
///
/// The type is [`Copy`], so it is threaded by value through the emitters.
#[derive(Clone, Copy)]
pub struct GenericScope<'a> {
    params: &'a [Symbol],
}

impl<'a> GenericScope<'a> {
    /// A scope quantifying `params`, in order (`params[i]` → `T{i+1}`).
    #[must_use]
    pub const fn new(params: &'a [Symbol]) -> Self {
        Self { params }
    }

    /// The deterministic Rust generic name for `sym` (`T1`, `T2`, … by position).
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] when `sym` is not in this scope — the
    /// lowerer is contracted to list every structurally-used type variable in
    /// [`ipe_ir::Func::type_params`], so an [`IrType::Generic`] outside the
    /// quantification scope is an internal invariant violation, surfaced rather
    /// than emitted as an undefined Rust identifier.
    fn rust_name(&self, sym: Symbol) -> DResult<String> {
        self.params.iter().position(|p| *p == sym).map_or_else(
            || {
                Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::GenericScope::rust_name",
                    detail: format!(
                        "generic type variable symbol {} is not in the enclosing function's \
                         quantification scope; the lowerer must list every structurally-used \
                         type variable in Func::type_params",
                        sym.as_raw()
                    ),
                })
            },
            |i| Ok(format!("T{}", i.saturating_add(1))),
        )
    }
}

/// Render an IR type to its Rust spelling. `generics` is the enclosing
/// function's generic scope (empty at program level), used to render
/// [`IrType::Generic`] as its deterministic Rust generic name.
#[allow(clippy::too_many_lines)]
pub fn render_type(ctx: &EmitCtx, ty: &IrType, generics: GenericScope) -> DResult<String> {
    Ok(match ty {
        IrType::Order => "ipe_runtime::basics::IpeOrder".to_owned(),
        IrType::HttpMethod => "ipe_runtime::HttpMethod".to_owned(),
        IrType::Decimal => "ipe_runtime::decimal::Decimal".to_owned(),
        IrType::ErrorKind => "ipe_runtime::error::IpeErrorKind".to_owned(),
        IrType::Error => "ipe_runtime::error::IpeError".to_owned(),
        IrType::ErrorDetails => "ipe_runtime::error::IpeErrorDetails".to_owned(),
        // The NOMINAL error-payload types (SEAL fix): rendered as
        // the runtime's concrete structs, so a pattern-bound payload and any
        // position naming these types agree on ONE Rust type — never a
        // project-local synthesized record struct.
        IrType::ErrorInfo => "ipe_runtime::error::IpeErrorInfo".to_owned(),
        IrType::PanicInfo => "ipe_runtime::error::IpePanicInfo".to_owned(),
        IrType::TypeInfo => "ipe_runtime::error::IpeTypeInfo".to_owned(),
        IrType::SqlFragment => "ipe_runtime::db::SqlFragment".to_owned(),
        IrType::Secret => "ipe_runtime::secret::Secret".to_owned(),
        IrType::Path => "ipe_runtime::path::Path".to_owned(),
        IrType::Regex => "ipe_runtime::regex_kernel::Regex".to_owned(),
        IrType::Int => "i64".to_owned(),
        IrType::Float => "f64".to_owned(),
        IrType::Bool => "bool".to_owned(),
        IrType::Str => "String".to_owned(),
        IrType::Char => "char".to_owned(),
        IrType::Unit => "()".to_owned(),
        IrType::Task(inner) => format!("IpeTask<{}>", render_type(ctx, inner, generics)?),
        IrType::Enum { home, name, args } => {
            // A foreign opaque FFI type renders as its REAL Rust path — its
            // placeholder union is never emitted as an enum ([`ipe_lower`]
            // skips `Rust.*`-home unions), so the bare enum name would dangle.
            // Opaque foreign types are non-generic by construction.
            if ctx.is_foreign_interface_home(home) {
                if !args.is_empty() {
                    return Err(Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::render_type",
                        detail: "foreign opaque FFI type rendered with type arguments".to_owned(),
                    });
                }
                return ctx.foreign_type_path(home, *name);
            }
            // Special-case builtin Http.Stream ADTs that are NOT registered as
            // synthetic `EnumDef`s but appear in user type annotations.
            //
            // `ChunkEvent` is generic over the error type (`E` = always
            // `IpeError` in practice) — we bake the concrete type arg in here
            // rather than propagating it through the IrType layer (the Ipê
            // user sees `ChunkEvent` as a non-generic type; the `E` channel is
            // invisible to user code).
            //
            // `StreamId` is handled by the `enum_name` override in `EmitCtx`
            // (returns `"IpeStreamId"`), so it falls through to the normal
            // non-generic path below.
            if home.0.is_empty() && args.is_empty() && ctx.resolve_ident(*name) == Ok("ChunkEvent")
            {
                return Ok("ChunkEvent<IpeError>".to_owned());
            }
            // `Ipe.Cache.Cache k v` is backed by the NON-generic runtime
            // enum `IpeCacheHandle` — drop the phantom `k`/`v` args (they live
            // only on the kernel calls), else the render would emit an invalid
            // `IpeCacheHandle<T1, T2>` (E0107). `enum_name` returns the runtime
            // name; here we skip appending the arg list.
            if ctx.is_cache_handle_type(home, *name) {
                return Ok("IpeCacheHandle".to_owned());
            }
            let base = ctx.enum_name(home, *name)?.to_owned();
            if args.is_empty() {
                // A non-generic enum renders as the bare Rust type name.
                base
            } else {
                let mut parts = Vec::with_capacity(args.len());
                for arg in args {
                    parts.push(render_type(ctx, arg, generics)?);
                }
                format!("{base}<{}>", parts.join(", "))
            }
        }
        // The built-in `Maybe a` / `Result e a` render to the runtime's shared
        // representations, brought into scope by the emitted crate's
        // `pub use ipe_runtime::*`.
        IrType::Maybe(elem) => format!("IpeMaybe<{}>", render_type(ctx, elem, generics)?),
        IrType::Result(err, ok) => format!(
            "IpeResult<{}, {}>",
            render_type(ctx, err, generics)?,
            render_type(ctx, ok, generics)?
        ),
        // The built-in `List a` is the runtime's `Vec<T>`.
        IrType::List(elem) => format!("Vec<{}>", render_type(ctx, elem, generics)?),
        // `Dict k v` is the runtime's `HashMap<K, V>`.
        IrType::Dict(k, v) => format!(
            "HashMap<{}, {}>",
            render_type(ctx, k, generics)?,
            render_type(ctx, v, generics)?
        ),
        // `Set a` is the runtime's `BTreeSet<A>`.
        IrType::Set(a) => format!("BTreeSet<{}>", render_type(ctx, a, generics)?),
        // `Bytes` is an arbitrary byte buffer — `Vec<u8>`. Divergence from
        // Ipê: Ipê aliases Bytes = String; Rust's String is UTF-8 constrained,
        // so Bytes maps to Vec<u8> for lossless arbitrary binary.
        IrType::Bytes => "Vec<u8>".to_owned(),
        // `Json` is the opaque JSON value type, `serde_json::Value`, exposed
        // from the runtime as `JsonVal` (re-exported via `pub use ipe_runtime::*`
        // in the emitted crate).
        IrType::Json => "JsonVal".to_owned(),
        // `Decoder<T>` is the JSON decoder type, aliased in the emitted project's
        // preamble as `pub type Decoder<T> = ipe_runtime::json::Decoder<IpeError, T>`.
        //
        // when the DECODED VALUE is itself a function (`Decoder (a -> b)` —
        // e.g. the accumulator of a `succeed Ctor |> required …` pipeline, or a
        // `succeed (partiallyApplied x)` payload), the runtime represents that
        // payload as an owned/linear curry chain, `Box<dyn FnOnce(a) -> b + Send>`
        // (what `curryN` builds and `decode_succeed`'s `A` is inferred to). A bare
        // `render_type` would render the `IrType::Fun` payload as the SHARED
        // callback form `Box<dyn Fn(a) -> b + Send + Sync>` — the wrong trait
        // (`Fn` vs `FnOnce`) AND an over-constrained `+ Sync` the curry chain does
        // not satisfy → ipe-0-then-cargo-fail (E0308/E0277). A decoder payload
        // never flows into an `Arc<dyn Fn + Send + Sync>` slot, so it is always the
        // Send-only owned shape. Render it as the `FnOnceChain` the runtime uses.
        IrType::Decoder(inner) => {
            let inner_s = match inner.as_ref() {
                IrType::Fun(params, ret) => render_fn_once_chain(ctx, params, ret, generics)?,
                other => render_type(ctx, other, generics)?,
            };
            format!("Decoder<{inner_s}>")
        }
        // `Db` is the opaque database connection pool type, re-exported from the
        // runtime as `pub use ipe_runtime::Db;` in the emitted crate preamble.
        IrType::Db => "Db".to_owned(),
        // `IpeCmd<M>` / `IpeSub<M>` are the opaque TEA command and subscription
        // types, aliased in the emitted project's preamble as
        // `pub type IpeCmd<M> = ipe_runtime::tea::IpeCmd<M>` and
        // `pub type IpeSub<M> = ipe_runtime::tea::IpeSub<M>`.
        IrType::Cmd(inner) => format!("IpeCmd<{}>", render_type(ctx, inner, generics)?),
        IrType::Sub(inner) => format!("IpeSub<{}>", render_type(ctx, inner, generics)?),
        // Opaque server types — render to their ipe_runtime names directly.
        IrType::ServerRequest => "ServerRequest".to_owned(),
        IrType::ServerResponse => "ServerResponse".to_owned(),
        IrType::ServerRoute => "ServerRoute".to_owned(),
        IrType::ServerCookie => "ServerCookie".to_owned(),
        // stream writer handle — re-exported from ipe_runtime::server_stream.
        IrType::StreamWriter => "StreamWriter".to_owned(),
        // HTTP request handle — re-exported from ipe_runtime::http.
        IrType::HttpRequest => "HttpRequest".to_owned(),
        // Ipe.Http.Server.WebSocket opaque handles.
        IrType::WebSocketServer => "WsHandle".to_owned(),
        IrType::WebSocketServerCfg => "WsServerCfg<IpeError>".to_owned(),
        // Ipe.Cache config / stats records — re-exported (ungated) from
        // ipe_runtime::cache, so the bare name resolves via the crate glob use.
        IrType::CacheCfg => "CacheCfg".to_owned(),
        IrType::CacheStats => "CacheStats".to_owned(),
        // Ipe.WebSocket connect-config record — re-exported (feature-gated
        // on `websocket_client`) from ipe_runtime::ws_client, bare via the glob.
        IrType::WebSocketClientCfg => "WsClientCfg".to_owned(),
        // Ipe.Csv document record — re-exported (ungated) from ipe_runtime::csv,
        // so the bare name resolves via the crate glob use.
        IrType::CsvDoc => "CsvDoc".to_owned(),
        // Ipe.Email records + provider ADT — re-exported from ipe_runtime::email
        // (`pub use email::*` appended when the program uses `Email.send`), so
        // the bare names resolve via the crate glob use. `Attachment` maps to the
        // runtime `EmailAttachment` (the Ipê alias name differs from the runtime
        // struct name).
        IrType::EmailMessage => "EmailMessage".to_owned(),
        IrType::EmailAttachment => "EmailAttachment".to_owned(),
        IrType::EmailSesConfig => "SesConfig".to_owned(),
        IrType::EmailSmtpConfig => "SmtpConfig".to_owned(),
        IrType::EmailProvider => "EmailProvider".to_owned(),
        // Typed-key newtypes — fully-qualified to avoid ambiguity with any
        // user-defined `Key`/`Mac`/`EmailAddress` types.
        IrType::CryptoKey => "ipe_runtime::crypto::Key".to_owned(),
        IrType::CryptoMac => "ipe_runtime::crypto::Mac".to_owned(),
        IrType::EmailAddress => "ipe_runtime::email::EmailAddress".to_owned(),
        // `Ipe.Url`'s opaque validated URL — fully-qualified to avoid ambiguity
        // with any user-defined `Url` type.
        IrType::Url => "ipe_runtime::url::Url".to_owned(),
        // Ipe.Ui / Ipe.Html parametric types.  Use fully-qualified Rust paths
        // (T2 soundness: `Attribute` exists in BOTH Ipe.Ui and Ipe.Html namespaces;
        // qualified paths keep them unambiguous and prevent glob-import shadowing).
        IrType::Ui { ctor, msg } => {
            let m = render_type(ctx, msg, generics)?;
            match ctor {
                UiCtor::Html => format!("ipe_runtime::html::Html<{m}>"),
                UiCtor::Element => format!("ipe_runtime::ui::element::Element<{m}>"),
                UiCtor::UiAttribute => format!("ipe_runtime::ui::element::Attribute<{m}>"),
                UiCtor::HtmlAttribute => format!("ipe_runtime::html::Attribute<{m}>"),
                UiCtor::HtmlEvent => format!("ipe_runtime::html::Event<{m}>"),
                UiCtor::Label => format!("ipe_runtime::ui::input::Label<{m}>"),
                UiCtor::Placeholder => format!("ipe_runtime::ui::input::Placeholder<{m}>"),
                UiCtor::RadioOption => format!("ipe_runtime::ui::input::RadioOption<{m}>"),
            }
        }
        IrType::UiPlain(plain) => match plain {
            UiPlain::Length => "ipe_runtime::ui::element::Length".to_owned(),
            UiPlain::Color => "ipe_runtime::ui::element::Color".to_owned(),
            UiPlain::HAlign => "ipe_runtime::ui::element::HAlign".to_owned(),
            UiPlain::VAlign => "ipe_runtime::ui::element::VAlign".to_owned(),
            UiPlain::Location => "ipe_runtime::ui::element::Location".to_owned(),
            UiPlain::PseudoClass => "ipe_runtime::ui::element::PseudoClass".to_owned(),
            UiPlain::Description => "ipe_runtime::ui::element::Description".to_owned(),
            UiPlain::LayoutContext => "ipe_runtime::ui::element::LayoutContext".to_owned(),
        },
        // Web types — render to qualified runtime paths.
        IrType::WebReq => "ipe_runtime::web::WebReq".to_owned(),
        // `Route<Page>` has NO default type parameter in the runtime
        // (`web/route.rs`), so the page argument MUST be rendered: a bare
        // `Route` is an E0107 cargo failure in every rendered position — the
        // empty `routes = []` literal's `Vec::<…>::new()` turbofish and any
        // let-bound route table's fn signature.
        IrType::WebRoute(page) => format!(
            "ipe_runtime::web::route::Route<{}>",
            render_type(ctx, page, generics)?
        ),
        IrType::Tuple(elems) => {
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(render_type(ctx, elem, generics)?);
            }
            format!("({})", parts.join(", "))
        }
        IrType::Record(fields) => ctx.render_record_use(fields, generics)?,
        // Handler-arrow special case: `Request -> Task Error Response` must
        // render as `ServerHandler<IpeError>` (an Arc<dyn Fn> alias defined in
        // the runtime), not as a generic `Box<dyn Fn + Send + 'static>`.  This
        // arm MUST appear before the generic `Fun` arm so it takes priority.
        IrType::Fun(params, ret)
            if matches!(params.as_slice(), [IrType::ServerRequest])
                && matches!(ret.as_ref(), IrType::Task(inner) if matches!(inner.as_ref(), IrType::ServerResponse)) =>
        {
            "ServerHandler<IpeError>".to_owned()
        }
        // WsServerCfg callback fields store Arc<dyn Fn + Send + Sync>; emit the
        // matching type so the WS adapter functions compile.  The three shapes are:
        //   onConnect / onClose  →  Fn(WsHandle) -> IpeTask<()>
        //   onMessage            →  Fn(WsHandle, String) -> IpeTask<()>
        //   onError              →  Fn(WsHandle, Error)  -> IpeTask<()>
        // `onError`'s second param is the error type, NOT String — its runtime
        // setter `ws_server_with_on_error` takes `Arc<dyn Fn(WsHandle, E) -> …>`,
        // so it MUST render as `Arc<…>` here (and box with `Arc::new` in
        // `wants_arc_ctor`, whose pattern is kept in lock-step). Omitting the
        // `[WebSocketServer, Error]` shape rendered it as the generic `Box<dyn Fn>`
        // below and passed a `Box` into that `Arc` param → ipe-0-then-cargo-fail
        // E0308 for any `onError` callback.
        // This arm MUST appear before the generic `Fun` arm so it takes priority.
        IrType::Fun(params, ret)
            if matches!(
                params.as_slice(),
                [IrType::WebSocketServer] | [IrType::WebSocketServer, IrType::Str | IrType::Error]
            ) && matches!(ret.as_ref(), IrType::Task(inner) if matches!(inner.as_ref(), IrType::Unit)) =>
        {
            let mut parts = Vec::with_capacity(params.len());
            for param in params {
                parts.push(render_type(ctx, param, generics)?);
            }
            let ret_ty = render_type(ctx, ret, generics)?;
            format!(
                "Arc<dyn Fn({}) -> {ret_ty} + Send + Sync + 'static>",
                parts.join(", ")
            )
        }
        // The promoted reference-counted fn carrier: `Arc<dyn Fn(..) -> R + Send
        // + Sync + 'static>`. `Arc` is `Clone` (a refcount bump), so a composite
        // holding this slot is itself `Clone` and can be duplicated for reuse.
        // Same trait-object bound as the `Box` carrier below — the ONLY
        // difference is the smart pointer — so the swap is capture-transparent
        // (every capture the `Box` form admits, the `Arc` form admits too).
        IrType::SharedFun(params, ret) => {
            let mut parts = Vec::with_capacity(params.len());
            for param in params {
                parts.push(render_type(ctx, param, generics)?);
            }
            let ret = render_type(ctx, ret, generics)?;
            format!(
                "::std::sync::Arc<dyn Fn({}) -> {ret} + Send + Sync + 'static>",
                parts.join(", ")
            )
        }
        IrType::Fun(params, ret) => {
            // A first-class function value is a boxed trait object
            // `Box<dyn Fn(T0, ...) -> R + Send + Sync + 'static>`. The
            // `Send + Sync + 'static` bounds are required so closures can be
            // passed to Task combinators (`task_map`, `task_and_then`, etc.)
            // AND — crucially — so a callback-typed PARAMETER can be forwarded
            // into the runtime's UI/Live event-callback slots, whose fields are
            // `Arc<dyn Fn(_) -> _ + Send + Sync + 'static>` (see
            // `ipe_runtime::ui::element::Event`). Without `Sync` on this boxed
            // param, an emitted user fn generic over its Msg type
            // (`fn f<T1: Clone>(onEdit: Box<dyn Fn(String) -> T1 + Send>) …`)
            // that arc-wraps `onEdit` (via `arc_callback_wrap`) and passes it to
            // `input_multiline_`/`input_text_`/… fails E0277 (`… cannot be
            // shared between threads safely`) — the seal break that
            // `26-ui-showcase`'s `regression_gates_input_multiline_fill`
            // surfaced. `Send + Sync` is strictly stronger than the previous
            // `Send`, so every `Send + 'static` consumer (the Task combinators)
            // still accepts it; all closures emitted by this backend are `move`
            // closures capturing only `Send + Sync + 'static` values, so this is
            // sound. A nullary function type renders as
            // `Box<dyn Fn() -> R + Send + Sync + 'static>`. The boxed-closure
            // optimisation (a concrete, non-boxed generic closure type) is deferred.
            let mut parts = Vec::with_capacity(params.len());
            for param in params {
                parts.push(render_type(ctx, param, generics)?);
            }
            let ret = render_type(ctx, ret, generics)?;
            format!(
                "Box<dyn Fn({}) -> {ret} + Send + Sync + 'static>",
                parts.join(", ")
            )
        }
        // `f7_succeed_curried`: a curried chain of ONE-SHOT closures,
        // one `Box<dyn FnOnce>` level per parameter — distinct from `Fun`'s
        // flattened, re-callable `Box<dyn Fn(T0, T1, ...) -> R>`. Rendered
        // from the INSIDE out: the last parameter's box wraps the bare
        // return type; every earlier parameter's box wraps the box built
        // for the levels after it. Matches exactly the nesting the
        // `curryN` runtime helpers construct (`curry2` → `Box<dyn
        // FnOnce(A1) -> Box<dyn FnOnce(A2) -> R + Send> + Send>`), which is
        // what the `next_decoder` parameter of `decode_pipeline_required` /
        // `decode_pipeline_optional` / `decode_pipeline_required_at` /
        // `db_decode_required` / `db_decode_optional` actually requires.
        IrType::FnOnceChain(params, ret) => render_fn_once_chain(ctx, params, ret, generics)?,
        // A generic type variable renders as the function's corresponding Rust
        // generic (`T1`, `T2`, …), resolved by position in the quantification
        // scope. No trait bound is emitted — only parametric pass-through is
        // supported here; constrained variables are rejected upstream.
        IrType::Generic(sym) => generics.rust_name(*sym)?,
    })
}

/// Render a curried chain of ONE-SHOT closures — one `Box<dyn FnOnce(..) -> _ +
/// Send + 'static>` level per parameter, nested from the INSIDE out. This is the
/// exact shape the `curryN` runtime helpers construct (`curry2` → `Box<dyn
/// FnOnce(A1) -> Box<dyn FnOnce(A2) -> R + Send> + Send>`) and that the
/// `next_decoder` / factory parameters of the decode/db-decode combinators
/// require. It is Send-ONLY (never `+ Sync`): a `FnOnce` curry chain is an
/// owned/linear value that flows into the runtime's `Box<dyn Fn(..) + Send>`
/// decoder slots, never into a shared `Arc<dyn Fn + Send + Sync>` callback slot —
/// so forcing `+ Sync` on it is over-constrained and unsatisfiable.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] on an empty parameter list — a function
/// type with no parameters is not a `FnOnce` chain (see the [`IrType::FnOnceChain`]
/// variant's doc comment: its sole producer never constructs a zero-param chain).
fn render_fn_once_chain(
    ctx: &EmitCtx,
    params: &[IrType],
    ret: &IrType,
    generics: GenericScope,
) -> DResult<String> {
    let Some((last, init)) = params.split_last() else {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_types::render_fn_once_chain",
            detail: "function-value curry chain with an empty parameter list".to_owned(),
        });
    };
    let mut acc = render_type(ctx, ret, generics)?;
    acc = format!(
        "Box<dyn FnOnce({}) -> {acc} + Send + 'static>",
        render_type(ctx, last, generics)?
    );
    for param in init.iter().rev() {
        acc = format!(
            "Box<dyn FnOnce({}) -> {acc} + Send + 'static>",
            render_type(ctx, param, generics)?
        );
    }
    Ok(acc)
}

/// Emit an enum and its derived `IpeStringify` impl, including the trailing
/// newline.
///
/// A nullary-only, non-generic enum emits byte-identically to the
/// golden:
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub enum MainMsg {
///     Increment,
///     Decrement,
/// }
/// impl IpeStringify for MainMsg {
///     fn ipe_show(&self) -> String {
///         match self {
///             MainMsg::Increment => "Increment".to_string(),
///             MainMsg::Decrement => "Decrement".to_string(),
///         }
///     }
/// }
/// ```
///
/// A payload-carrying and/or generic enum gains tuple-variant payloads, a
/// `<T1, …>` clause on the enum and its impl, and `IpeStringify` arms that bind
/// each payload field and render it through the total autoref dispatch — mirroring
/// the Go-reference Rust backend's `ipeStringifyEnumImpl`:
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub enum MainMaybe<T1> {
///     Just(T1),
///     Nothing,
/// }
/// impl<T1: IpeStringify + std::fmt::Debug> IpeStringify for MainMaybe<T1> {
///     fn ipe_show(&self) -> String {
///         match self {
///             MainMaybe::Just(p0) => format!("Just {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch()),
///             MainMaybe::Nothing => "Nothing".to_string(),
///         }
///     }
/// }
/// ```
///
/// A payload field that sits on a type-size cycle back to its own enum —
/// directly (`Node Tree Int Tree`) or indirectly (mutual recursion, or a
/// self-edge routed through a tuple / record / another generic's type argument)
/// — is wrapped in `Box<…>` so the Rust enum stays finite-sized (E0072); the
/// construction and pattern emitters balance that boxing. See
/// [`crate::EmitCtx::is_cyclic_self_field`].
pub fn emit_enum(ctx: &EmitCtx, def: &EnumDef) -> DResult<String> {
    // A `Ipe.WebSocket` ADT bridged to a runtime enum
    // (`WebSocketMessage` → `WsClientMessage`, `CloseCode` → `WsCloseCode`) has
    // NO user-emitted decl — its definition lives in `ipe_runtime::ws_client`,
    // and emitting a second enum with the same name would trip E0428. The
    // `enum_name` override in `EmitCtx::build` already routes every reference to
    // the runtime type. Suppress the body here.
    if ctx.is_websocket_bridged_enum(&def.home, def.name)? {
        return Ok(String::new());
    }
    let name = ctx.enum_name(&def.home, def.name)?.to_owned();
    // The enum's own generic scope: each type parameter → `T1`, `T2`, … by
    // position. Empty for a non-generic enum.
    let scope = GenericScope::new(&def.type_params);

    let mut variant_lines = Vec::with_capacity(def.variants.len());
    let mut show_arms = Vec::with_capacity(def.variants.len());
    for variant in &def.variants {
        // The Rust variant ident is keyword-mangled; the `ipe_show` string keeps
        // the original Ipê name so a variant like `Type` still displays as
        // "Type", not "Type_". For non-keyword variants the two coincide, so the
        // golden stays byte-identical.
        let vn = ctx.emit_ident(variant.name)?;
        let display = ctx.resolve_ident(variant.name)?.to_owned();
        if variant.fields.is_empty() {
            variant_lines.push(format!("    {vn},"));
            show_arms.push(format!(
                "            {name}::{vn} => \"{display}\".to_string(),"
            ));
        } else {
            // Payload variant: render each field type (boxing a direct self-edge),
            // and bind a `pN` per field in the stringify arm.
            let mut field_types = Vec::with_capacity(variant.fields.len());
            let mut binders = Vec::with_capacity(variant.fields.len());
            let mut show_args = Vec::with_capacity(variant.fields.len());
            for (i, field_ty) in variant.fields.iter().enumerate() {
                let rendered = render_type(ctx, field_ty, scope)?;
                let rendered = if ctx.is_cyclic_self_field(field_ty, &def.home, def.name) {
                    format!("Box<{rendered}>")
                } else {
                    rendered
                };
                field_types.push(rendered);
                if ir_type_is_derivable(field_ty, &|home, name| ctx.enum_is_derivable(home, name)) {
                    let binder = format!("p{i}");
                    // `binder` is a `match self` binder → already a `&FieldType`,
                    // so `Wrap(binder)` carries the reference the dispatch
                    // expects. Sound because a derivable field type impls
                    // `IpeStringify` or `Debug` (the autoref fallback).
                    show_args.push(format!(
                        "(&ipe_runtime::stringify::Wrap({binder})).dispatch()"
                    ));
                    binders.push(binder);
                } else {
                    // seal: a non-derivable payload (a function / opaque
                    // wrapper) impls neither `IpeStringify` nor `Debug`, so the
                    // autoref `.dispatch()` would not resolve (E0599). Bind it
                    // with `_` and render a `<fn>` placeholder — these carry no
                    // user-visible data, matching the reference backend.
                    binders.push("_".to_owned());
                    show_args.push("\"<fn>\"".to_owned());
                }
            }
            variant_lines.push(format!("    {vn}({}),", field_types.join(", ")));
            let placeholders = vec!["{}"; variant.fields.len()].join(" ");
            // Go `%v`-style: `Vname <f0> <f1> …` (variant name, then space-
            // separated fields). Matches the Go-reference `ipeStringifyEnumImpl`.
            // The arm head `            {name}::{vn}(binders) => ` sits at block
            // indent 12; `render_stringify_enum_arm` lays the `format!` tail out in
            // `rustfmt`'s inline / block-wrap / delimiter-break tiers.
            let arm_head = format!("            {name}::{vn}({}) => ", binders.join(", "));
            let fmt_literal = format!("\"{display} {placeholders}\"");
            show_arms.push(render_stringify_enum_arm(
                &arm_head,
                &fmt_literal,
                &show_args,
            ));
        }
    }
    let variants = variant_lines.join("\n");
    let arms = show_arms.join("\n");

    // Generic clauses: `<T1, T2>` on the enum, `<T1: IpeStringify + Debug, …>` on
    // the impl, `<T1, T2>` on the impl's `for` type. All empty when the enum is
    // non-generic, so that path emits no generic clause.
    let params: Vec<String> = (1..=def.type_params.len())
        .map(|i| format!("T{i}"))
        .collect();
    let (decl_clause, impl_bounds, use_clause) = if params.is_empty() {
        (String::new(), String::new(), String::new())
    } else {
        let bounds: Vec<String> = params
            .iter()
            .map(|p| format!("{p}: IpeStringify + std::fmt::Debug"))
            .collect();
        (
            format!("<{}>", params.join(", ")),
            format!("<{}>", bounds.join(", ")),
            format!("<{}>", params.join(", ")),
        )
    };

    // seal: only a fully-derivable enum takes the unconditional
    // `#[derive(Clone, Debug, PartialEq)]`. An enum whose payload reaches a
    // first-class function / opaque wrapper (directly or through a carrier /
    // another non-derivable enum) cannot derive those traits — emit no auto
    // derives (the hand-written `IpeStringify` impl below still gives it a total
    // string form).
    let self_derivable = ctx.enum_is_derivable(&def.home, def.name);
    // seal: the serde derive is gated on the SERDE predicate, not the CDPeq
    // one. serde-support ⊊ CDPeq-support: a `Clone + Debug + PartialEq` enum whose
    // payload reaches a `Html` / `Element` / `Color` / `UiPlain` value (serde-less
    // but derivable) must NOT be forced to `serde::Serialize` / `Deserialize` —
    // doing so under `uses_web` is an exit-0-then-cargo-fail (E0277). Such an enum
    // still gets its `#[derive(Clone, Debug, PartialEq)]` (self_derivable), just
    // without serde, so it stays cargo-buildable. `enum_is_serde ⇒ enum_is_derivable`
    // (the serde fixpoint is a demotion of the derivable one), so serde is never
    // added without CDPeq. The app-entry Model gate independently rejects a
    // NON-serde type used AS a Web/Tui/WebView Model; this gate covers every OTHER
    // (non-Model) emitted type in a Web program.
    let self_serde = ctx.enum_is_serde(&def.home, def.name);
    // When the program uses Ipe.Web, model types must implement serde traits
    // so the session store can serialise/deserialise them. The live runtime
    // requires `Model: serde::Serialize + serde::de::DeserializeOwned`.
    let serde_derives = if self_serde && ctx.uses_web {
        ", serde::Serialize, serde::Deserialize"
    } else {
        ""
    };
    let derive_prefix = if self_derivable {
        format!("#[derive(Clone, Debug, PartialEq{serde_derives})]\n")
    } else {
        String::new()
    };
    Ok(format!(
        "{derive_prefix}pub enum {name}{decl_clause} {{
{variants}
}}
impl{impl_bounds} IpeStringify for {name}{use_clause} {{
    fn ipe_show(&self) -> String {{
        match self {{
{arms}
        }}
    }}
}}
"
    ))
}

/// Emit a synthesised record struct and its derived `IpeStringify` impl,
/// including the trailing newline.
///
/// Shape (for `{ x : Int, y : Int }`):
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub struct RecXY {
///     x: i64,
///     y: i64,
/// }
/// impl IpeStringify for RecXY {
///     fn ipe_show(&self) -> String {
///         format!("{{{} {}}}", (&ipe_runtime::stringify::Wrap(&self.x)).dispatch(), (&ipe_runtime::stringify::Wrap(&self.y)).dispatch())
///     }
/// }
/// ```
///
/// The `ipe_show` body mirrors the Go reference's `%v` rendering of a struct
/// (`{f0 f1 ...}`, fields space-separated in declared order, no field names) so
/// stringifying a record reads identically across the two backends. Each field
/// renders through the runtime's total autoref `Wrap(..).dispatch()` shim, which
/// never fails to resolve a method regardless of the field type.
///
/// A GENERIC record shape (a field typed by a type variable) gains a
/// generic clause on both the struct and its impl. Shape (for `{ value : a }`):
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub struct RecValue<T1> {
///     value: T1,
/// }
/// impl<T1: IpeStringify + std::fmt::Debug> IpeStringify for RecValue<T1> {
///     ...
/// }
/// ```
/// The impl bounds each parameter `IpeStringify + std::fmt::Debug` so the inline
/// autoref `Wrap(..).dispatch()` resolves at the generic frame (the
/// `IpeStringify` arm is selected with zero autoref, the `Debug` arm is the
/// always-available fallback). `std::fmt::Debug` is spelled in full — the
/// emitted crate's `pub use ipe_runtime::*` shadows the `core` crate with the
/// runtime's `core` module, so `core::fmt` would not resolve. A monomorphic
/// record emits an empty clause.
pub fn emit_record_struct(ctx: &EmitCtx, rec: &RecordStruct) -> DResult<String> {
    let name = &rec.name;
    // The struct's own generic scope: each parameter symbol → `T1`, `T2`, … by
    // position. Empty for a monomorphic record.
    let scope = GenericScope::new(&rec.type_params);
    let mut field_lines = Vec::with_capacity(rec.fields.len());
    let mut show_args = Vec::with_capacity(rec.fields.len());
    for (field_name, field_ty) in &rec.fields {
        let ident = mangle_reserved(field_name.clone());
        let rust_ty = render_type(ctx, field_ty, scope)?;
        field_lines.push(format!("    {ident}: {rust_ty},"));
        if ir_type_is_derivable(field_ty, &|home, name| ctx.enum_is_derivable(home, name)) {
            show_args.push(format!(
                "(&ipe_runtime::stringify::Wrap(&self.{ident})).dispatch()"
            ));
        } else {
            // seal: a non-derivable field (a function / opaque wrapper)
            // impls neither `IpeStringify` nor `Debug`, so `.dispatch()` would
            // not resolve. Render a `<fn>` placeholder for that `{}` slot.
            show_args.push("\"<fn>\"".to_owned());
        }
    }
    let fields_block = field_lines.join("\n");

    // Generic clauses: `<T1, T2>` on the struct, `<T1: IpeStringify + Debug, …>`
    // on the impl, `<T1, T2>` on the impl's `for` type. All empty when the record
    // is monomorphic.
    let params: Vec<String> = (1..=rec.type_params.len())
        .map(|i| format!("T{i}"))
        .collect();
    let (decl_clause, impl_bounds, use_clause) = if params.is_empty() {
        (String::new(), String::new(), String::new())
    } else {
        let bounds: Vec<String> = params
            .iter()
            .map(|p| format!("{p}: IpeStringify + std::fmt::Debug"))
            .collect();
        (
            format!("<{}>", params.join(", ")),
            format!("<{}>", bounds.join(", ")),
            format!("<{}>", params.join(", ")),
        )
    };

    // Go `%v` of a struct: `{v0 v1 ...}` — N space-separated `{}` placeholders
    // wrapped in literal braces. With zero fields the rendering is just `{}`.
    //
    // The `format!` body is laid out natively (not hand-inlined then handed to
    // `rustfmt`): it lands at column 8 — `fn ipe_show`'s body, two block-indent
    // levels deep — and breaks one argument per line there when its argument text
    // exceeds `fn_call_width`, matching `rustfmt` by construction.
    let body = if rec.fields.is_empty() {
        "\"{}\".to_string()".to_owned()
    } else {
        let placeholders = vec!["{}"; rec.fields.len()].join(" ");
        let fmt_literal = format!("\"{{{{{placeholders}}}}}\"");
        render_stringify_format(&fmt_literal, &show_args, 8, 8)
    };

    // seal: only a fully-derivable record takes the unconditional
    // `#[derive(Clone, Debug, PartialEq)]`. A record holding a first-class
    // function / opaque wrapper field (directly or through a carrier / a
    // non-derivable enum) cannot derive those traits — emit no auto derives (the
    // hand-written `IpeStringify` impl below still gives it a total string form).
    //
    // seal: the serde derive is gated on `rec.is_serde` (the per-record serde
    // fixpoint), NOT `rec.is_derivable`. A CDPeq-but-not-serde record — e.g. a
    // view-helper `{ title : String, body : Html Msg }` in a Ipe.Web program —
    // keeps its `#[derive(Clone, Debug, PartialEq)]` but is NOT forced to
    // `serde::Serialize` / `Deserialize`, which would be an exit-0-then-cargo-fail
    // (E0277: `Html<Msg>: Serialize` unsatisfied). `is_serde ⇒ is_derivable`
    // (serde-OK leaves ⊂ derivable leaves), so serde is never added without CDPeq.
    // The app-entry Model gate independently rejects a non-serde type used AS a
    // Web/Tui/WebView Model; this gate covers every OTHER (non-Model) record.
    let serde_derives = if rec.is_serde && ctx.uses_web {
        ", serde::Serialize, serde::Deserialize"
    } else {
        ""
    };
    let derive_prefix = if rec.is_derivable {
        format!("#[derive(Clone, Debug, PartialEq{serde_derives})]\n")
    } else {
        String::new()
    };
    // seal: a record that is `Clone` but NOT fully `CDPeq`-derivable — the
    // fn-value-reuse promotion's record-of-functions, whose `Arc<dyn Fn>`
    // (`SharedFun`) fields are `Clone` yet neither `Debug` nor `PartialEq` — gets
    // a HAND-WRITTEN `impl Clone` (cloning every field, an `Arc::clone` refcount
    // bump on each fn slot). Without it, a reused promoted record's `.clone()`
    // would be an `ipe`-0-then-cargo-fail `E0599`. A fully-derivable record takes
    // the derive above instead, so the two paths never both emit a `Clone` impl.
    let clone_impl = if rec.is_clone && !rec.is_derivable {
        let field_clones: Vec<String> = rec
            .fields
            .iter()
            .map(|(field_name, _)| {
                let ident = mangle_reserved(field_name.clone());
                format!("            {ident}: self.{ident}.clone(),")
            })
            .collect();
        format!(
            "impl{impl_clone_bounds} Clone for {name}{use_clause} {{
    fn clone(&self) -> Self {{
        Self {{
{}
        }}
    }}
}}
",
            field_clones.join("\n"),
            impl_clone_bounds = if params.is_empty() {
                String::new()
            } else {
                format!("<{}>", params.join(", "))
            },
        )
    } else {
        String::new()
    };
    Ok(format!(
        "{derive_prefix}pub struct {name}{decl_clause} {{
{fields_block}
}}
{clone_impl}impl{impl_bounds} IpeStringify for {name}{use_clause} {{
    fn ipe_show(&self) -> String {{
        {body}
    }}
}}
"
    ))
}
