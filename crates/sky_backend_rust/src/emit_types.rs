//! Type emission (M0 subset): user enums and their `SkyStringify` impls, plus
//! IR-type → Rust-type rendering.
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/TypeEmitter.hs`
//! (`unionToRustTypeDef`) and `Emitter.hs` (`typeDefToString` / the enum
//! `skyStringifyEnumImpl`). The byte target is golden `main.rs` lines 31–43.

use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Symbol;
use sky_ir::{ir_type_is_derivable, EnumDef, IrType, UiCtor, UiPlain};

use crate::naming::mangle_reserved;
use crate::{EmitCtx, RecordStruct};

/// The generic-type-parameter scope in effect while emitting one function's
/// signature and body (M2a).
///
/// Maps a Sky type-variable [`Symbol`] to its deterministic Rust generic name
/// (`T1`, `T2`, …) by the variable's *position* in the function's quantification
/// order — never by the symbol's spelling — so a function quantifying `[a, b]`
/// renders `a` → `T1` and `b` → `T2` regardless of source naming. Empty for
/// monomorphic functions and for program-level emission (enums, record structs),
/// where no generic is in scope.
///
/// Phase-1a: the `enclosing_ui_msg` field and `with_ui_msg`/`enclosing_ui_msg()`
/// methods that used to thread the enclosing function's `Html<M>` return type down
/// to `UiLayout`/`UiLayoutWith` have been removed.  M is now inferred bottom-up
/// from the concrete element/attrs types sourced from `SolvedTypes.regions`.
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
    /// [`sky_ir::Func::type_params`], so an [`IrType::Generic`] outside the
    /// quantification scope is an internal invariant violation, surfaced rather
    /// than emitted as an undefined Rust identifier.
    fn rust_name(&self, sym: Symbol) -> DResult<String> {
        self.params.iter().position(|p| *p == sym).map_or_else(
            || {
                Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::GenericScope::rust_name",
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
        IrType::Order => "sky_runtime::basics::SkyOrder".to_owned(),
        IrType::Int => "i64".to_owned(),
        IrType::Float => "f64".to_owned(),
        IrType::Bool => "bool".to_owned(),
        IrType::Str => "String".to_owned(),
        IrType::Char => "char".to_owned(),
        IrType::Unit => "()".to_owned(),
        IrType::Task(inner) => format!("SkyTask<{}>", render_type(ctx, inner, generics)?),
        IrType::Enum { home, name, args } => {
            // Special-case builtin Http.Stream ADTs that are NOT registered as
            // synthetic `EnumDef`s but appear in user type annotations (#148).
            //
            // `ChunkEvent` is generic over the error type (`E` = always
            // `SkyError` in practice) — we bake the concrete type arg in here
            // rather than propagating it through the IrType layer (the Sky
            // user sees `ChunkEvent` as a non-generic type; the `E` channel is
            // invisible to user code).
            //
            // `StreamId` is handled by the `enum_name` override in `EmitCtx`
            // (returns `"SkyStreamId"`), so it falls through to the normal
            // non-generic path below.
            if home.0.is_empty() && args.is_empty()
                && ctx.resolve_ident(*name) == Ok("ChunkEvent")
            {
                return Ok("ChunkEvent<SkyError>".to_owned());
            }
            let base = ctx.enum_name(home, *name)?.to_owned();
            if args.is_empty() {
                // A non-generic enum renders as the bare Rust type name —
                // byte-identical to the M0 backend.
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
        // `pub use sky_runtime::*`.
        IrType::Maybe(elem) => format!("SkyMaybe<{}>", render_type(ctx, elem, generics)?),
        IrType::Result(err, ok) => format!(
            "SkyResult<{}, {}>",
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
        // Sky: Sky aliases Bytes = String; Rust's String is UTF-8 constrained,
        // so Bytes maps to Vec<u8> for lossless arbitrary binary.
        IrType::Bytes => "Vec<u8>".to_owned(),
        // `Json` is the opaque JSON value type, `serde_json::Value`, exposed
        // from the runtime as `JsonVal` (re-exported via `pub use sky_runtime::*`
        // in the emitted crate).
        IrType::Json => "JsonVal".to_owned(),
        // `Decoder<T>` is the JSON decoder type, aliased in the emitted project's
        // preamble as `pub type Decoder<T> = sky_runtime::json::Decoder<SkyError, T>`.
        IrType::Decoder(inner) => {
            format!("Decoder<{}>", render_type(ctx, inner, generics)?)
        }
        // `Db` is the opaque database connection pool type, re-exported from the
        // runtime as `pub use sky_runtime::Db;` in the emitted crate preamble.
        IrType::Db => "Db".to_owned(),
        // `SkyCmd<M>` / `SkySub<M>` are the opaque TEA command and subscription
        // types, aliased in the emitted project's preamble as
        // `pub type SkyCmd<M> = sky_runtime::tea::SkyCmd<M>` and
        // `pub type SkySub<M> = sky_runtime::tea::SkySub<M>`.
        IrType::Cmd(inner) => format!("SkyCmd<{}>", render_type(ctx, inner, generics)?),
        IrType::Sub(inner) => format!("SkySub<{}>", render_type(ctx, inner, generics)?),
        // M6 opaque server types — render to their sky_runtime names directly.
        IrType::ServerRequest => "ServerRequest".to_owned(),
        IrType::ServerResponse => "ServerResponse".to_owned(),
        IrType::ServerRoute => "ServerRoute".to_owned(),
        IrType::ServerCookie => "ServerCookie".to_owned(),
        // #111: stream writer handle — re-exported from sky_runtime::server_stream.
        IrType::StreamWriter => "StreamWriter".to_owned(),
        // #111: HTTP request handle — re-exported from sky_runtime::http.
        IrType::HttpRequest => "HttpRequest".to_owned(),
        // #127: Sky.Http.Server.WebSocket opaque handles.
        IrType::WebSocketServer => "WsHandle".to_owned(),
        IrType::WebSocketServerCfg => "WsServerCfg<SkyError>".to_owned(),
        // M7 Std.Ui / Std.Html parametric types.  Use fully-qualified Rust paths
        // (T2 soundness: `Attribute` exists in BOTH Std.Ui and Std.Html namespaces;
        // qualified paths keep them unambiguous and prevent glob-import shadowing).
        IrType::Ui { ctor, msg } => {
            let m = render_type(ctx, msg, generics)?;
            match ctor {
                UiCtor::Html => format!("sky_runtime::html::Html<{m}>"),
                UiCtor::Element => format!("sky_runtime::ui::element::Element<{m}>"),
                UiCtor::UiAttribute => format!("sky_runtime::ui::element::Attribute<{m}>"),
                UiCtor::HtmlAttribute => format!("sky_runtime::html::Attribute<{m}>"),
                UiCtor::HtmlEvent => format!("sky_runtime::html::Event<{m}>"),
                UiCtor::Label => format!("sky_runtime::ui::input::Label<{m}>"),
                UiCtor::Placeholder => format!("sky_runtime::ui::input::Placeholder<{m}>"),
                UiCtor::RadioOption => format!("sky_runtime::ui::input::RadioOption<{m}>"),
            }
        }
        IrType::UiPlain(plain) => match plain {
            UiPlain::Length => "sky_runtime::ui::element::Length".to_owned(),
            UiPlain::Color => "sky_runtime::ui::element::Color".to_owned(),
            UiPlain::HAlign => "sky_runtime::ui::element::HAlign".to_owned(),
            UiPlain::VAlign => "sky_runtime::ui::element::VAlign".to_owned(),
            UiPlain::Location => "sky_runtime::ui::element::Location".to_owned(),
            UiPlain::PseudoClass => "sky_runtime::ui::element::PseudoClass".to_owned(),
            UiPlain::Description => "sky_runtime::ui::element::Description".to_owned(),
            UiPlain::LayoutContext => "sky_runtime::ui::element::LayoutContext".to_owned(),
        },
        // M7 Live types — render to qualified runtime paths.
        IrType::LiveReq => "sky_runtime::live::LiveReq".to_owned(),
        // `Route<Page>` has NO default type parameter in the runtime
        // (`live/route.rs`), so the page argument MUST be rendered: a bare
        // `Route` is an E0107 cargo failure in every rendered position — the
        // empty `routes = []` literal's `Vec::<…>::new()` turbofish and any
        // let-bound route table's fn signature (#108 round 4, hole 1).
        IrType::LiveRoute(page) => format!(
            "sky_runtime::live::route::Route<{}>",
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
        // M6 Handler-arrow special case: `Request -> Task Error Response` must
        // render as `ServerHandler<SkyError>` (an Arc<dyn Fn> alias defined in
        // the runtime), not as a generic `Box<dyn Fn + Send + 'static>`.  This
        // arm MUST appear before the generic `Fun` arm so it takes priority.
        IrType::Fun(params, ret)
            if matches!(params.as_slice(), [IrType::ServerRequest])
                && matches!(ret.as_ref(), IrType::Task(inner) if matches!(inner.as_ref(), IrType::ServerResponse)) =>
        {
            "ServerHandler<SkyError>".to_owned()
        }
        // WsServerCfg callback fields store Arc<dyn Fn + Send + Sync>; emit the
        // matching type so the WS adapter functions compile.  The two shapes are:
        //   onConnect / onClose  →  Fn(WsHandle) -> SkyTask<()>
        //   onMessage  / onError →  Fn(WsHandle, String) -> SkyTask<()>
        // This arm MUST appear before the generic `Fun` arm so it takes priority.
        IrType::Fun(params, ret)
            if matches!(
                params.as_slice(),
                [IrType::WebSocketServer] | [IrType::WebSocketServer, IrType::Str]
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
        IrType::Fun(params, ret) => {
            // A first-class function value is a boxed trait object
            // `Box<dyn Fn(T0, ...) -> R + Send + 'static>`. The `Send + 'static`
            // bounds are required so closures can be passed to Task combinators
            // (`task_map`, `task_and_then`, etc.) whose runtime signatures carry
            // those bounds. All closures emitted by this backend are `move`
            // closures that capture only `Send + 'static` values, so this is
            // always sound. A nullary function type renders as
            // `Box<dyn Fn() -> R + Send + 'static>`. The boxed-closure
            // optimisation (a concrete, non-boxed generic closure type) is deferred.
            let mut parts = Vec::with_capacity(params.len());
            for param in params {
                parts.push(render_type(ctx, param, generics)?);
            }
            let ret = render_type(ctx, ret, generics)?;
            format!(
                "Box<dyn Fn({}) -> {ret} + Send + 'static>",
                parts.join(", ")
            )
        }
        // A generic type variable renders as the function's corresponding Rust
        // generic (`T1`, `T2`, …), resolved by position in the quantification
        // scope (M2a). No trait bound is emitted — M2a covers only parametric
        // pass-through; constrained variables are rejected upstream.
        IrType::Generic(sym) => generics.rust_name(*sym)?,
    })
}

/// Emit an enum and its derived `SkyStringify` impl, including the trailing
/// newline.
///
/// A nullary-only, non-generic enum (the M0 case) emits byte-identically to the
/// golden:
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub enum MainMsg {
///     Increment,
///     Decrement,
/// }
/// impl SkyStringify for MainMsg {
///     fn sky_show(&self) -> String {
///         match self {
///             MainMsg::Increment => "Increment".to_string(),
///             MainMsg::Decrement => "Decrement".to_string(),
///         }
///     }
/// }
/// ```
///
/// A payload-carrying and/or generic enum (M3a) gains tuple-variant payloads, a
/// `<T1, …>` clause on the enum and its impl, and `SkyStringify` arms that bind
/// each payload field and render it through the total autoref dispatch — mirroring
/// the Go-reference Rust backend's `skyStringifyEnumImpl`:
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub enum MainMaybe<T1> {
///     Just(T1),
///     Nothing,
/// }
/// impl<T1: SkyStringify + std::fmt::Debug> SkyStringify for MainMaybe<T1> {
///     fn sky_show(&self) -> String {
///         match self {
///             MainMaybe::Just(p0) => format!("Just {}", (&sky_runtime::stringify::Wrap(p0)).dispatch()),
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
    let name = ctx.enum_name(&def.home, def.name)?.to_owned();
    // The enum's own generic scope: each type parameter → `T1`, `T2`, … by
    // position. Empty for a non-generic enum (byte-identical to M0).
    let scope = GenericScope::new(&def.type_params);

    let mut variant_lines = Vec::with_capacity(def.variants.len());
    let mut show_arms = Vec::with_capacity(def.variants.len());
    for variant in &def.variants {
        // The Rust variant ident is keyword-mangled; the `sky_show` string keeps
        // the original Sky name so a variant like `Type` still displays as
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
                    // `SkyStringify` or `Debug` (the autoref fallback).
                    show_args.push(format!(
                        "(&sky_runtime::stringify::Wrap({binder})).dispatch()"
                    ));
                    binders.push(binder);
                } else {
                    // #87 seal: a non-derivable payload (a function / opaque
                    // wrapper) impls neither `SkyStringify` nor `Debug`, so the
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
            // separated fields). Matches the Go-reference `skyStringifyEnumImpl`.
            show_arms.push(format!(
                "            {name}::{vn}({}) => format!(\"{display} {placeholders}\", {}),",
                binders.join(", "),
                show_args.join(", ")
            ));
        }
    }
    let variants = variant_lines.join("\n");
    let arms = show_arms.join("\n");

    // Generic clauses: `<T1, T2>` on the enum, `<T1: SkyStringify + Debug, …>` on
    // the impl, `<T1, T2>` on the impl's `for` type. All empty when the enum is
    // non-generic, so that path stays byte-identical to M0.
    let params: Vec<String> = (1..=def.type_params.len())
        .map(|i| format!("T{i}"))
        .collect();
    let (decl_clause, impl_bounds, use_clause) = if params.is_empty() {
        (String::new(), String::new(), String::new())
    } else {
        let bounds: Vec<String> = params
            .iter()
            .map(|p| format!("{p}: SkyStringify + std::fmt::Debug"))
            .collect();
        (
            format!("<{}>", params.join(", ")),
            format!("<{}>", bounds.join(", ")),
            format!("<{}>", params.join(", ")),
        )
    };

    // #87 seal: only a fully-derivable enum takes the unconditional
    // `#[derive(Clone, Debug, PartialEq)]`. An enum whose payload reaches a
    // first-class function / opaque wrapper (directly or through a carrier /
    // another non-derivable enum) cannot derive those traits — emit no auto
    // derives (the hand-written `SkyStringify` impl below still gives it a total
    // string form).
    let self_derivable = ctx.enum_is_derivable(&def.home, def.name);
    // #93 seal: the serde derive is gated on the SERDE predicate, not the CDPeq
    // one. serde-support ⊊ CDPeq-support: a `Clone + Debug + PartialEq` enum whose
    // payload reaches a `Html` / `Element` / `Color` / `UiPlain` value (serde-less
    // but derivable) must NOT be forced to `serde::Serialize` / `Deserialize` —
    // doing so under `uses_live` is an exit-0-then-cargo-fail (E0277). Such an enum
    // still gets its `#[derive(Clone, Debug, PartialEq)]` (self_derivable), just
    // without serde, so it stays cargo-buildable. `enum_is_serde ⇒ enum_is_derivable`
    // (the serde fixpoint is a demotion of the derivable one), so serde is never
    // added without CDPeq. #91's app-entry Model gate independently rejects a
    // NON-serde type used AS a Live/Tui/Webview Model; this gate covers every OTHER
    // (non-Model) emitted type in a Live program.
    let self_serde = ctx.enum_is_serde(&def.home, def.name);
    // When the program uses Std.Live, model types must implement serde traits
    // so the session store can serialise/deserialise them. The live runtime
    // requires `Model: serde::Serialize + serde::de::DeserializeOwned`.
    let serde_derives = if self_serde && ctx.uses_live {
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
impl{impl_bounds} SkyStringify for {name}{use_clause} {{
    fn sky_show(&self) -> String {{
        match self {{
{arms}
        }}
    }}
}}
"
    ))
}

/// Emit a synthesised record struct and its derived `SkyStringify` impl,
/// including the trailing newline.
///
/// Shape (for `{ x : Int, y : Int }`):
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub struct RecXY {
///     x: i64,
///     y: i64,
/// }
/// impl SkyStringify for RecXY {
///     fn sky_show(&self) -> String {
///         format!("{{{} {}}}", (&sky_runtime::stringify::Wrap(&self.x)).dispatch(), (&sky_runtime::stringify::Wrap(&self.y)).dispatch())
///     }
/// }
/// ```
///
/// The `sky_show` body mirrors the Go reference's `%v` rendering of a struct
/// (`{f0 f1 ...}`, fields space-separated in declared order, no field names) so
/// stringifying a record reads identically across the two backends. Each field
/// renders through the runtime's total autoref `Wrap(..).dispatch()` shim, which
/// never fails to resolve a method regardless of the field type.
///
/// A GENERIC record shape (M2c — a field typed by a type variable) gains a
/// generic clause on both the struct and its impl. Shape (for `{ value : a }`):
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub struct RecValue<T1> {
///     value: T1,
/// }
/// impl<T1: SkyStringify + std::fmt::Debug> SkyStringify for RecValue<T1> {
///     ...
/// }
/// ```
/// The impl bounds each parameter `SkyStringify + std::fmt::Debug` so the inline
/// autoref `Wrap(..).dispatch()` resolves at the generic frame (the
/// `SkyStringify` arm is selected with zero autoref, the `Debug` arm is the
/// always-available fallback). `std::fmt::Debug` is spelled in full — the
/// emitted crate's `pub use sky_runtime::*` shadows the `core` crate with the
/// runtime's `core` module, so `core::fmt` would not resolve. A monomorphic
/// record emits an empty clause, so that path is byte-identical to b3.
pub fn emit_record_struct(ctx: &EmitCtx, rec: &RecordStruct) -> DResult<String> {
    let name = &rec.name;
    // The struct's own generic scope: each parameter symbol → `T1`, `T2`, … by
    // position. Empty for a monomorphic record (byte-identical to b3).
    let scope = GenericScope::new(&rec.type_params);
    let mut field_lines = Vec::with_capacity(rec.fields.len());
    let mut show_args = Vec::with_capacity(rec.fields.len());
    for (field_name, field_ty) in &rec.fields {
        let ident = mangle_reserved(field_name.clone());
        let rust_ty = render_type(ctx, field_ty, scope)?;
        field_lines.push(format!("    {ident}: {rust_ty},"));
        if ir_type_is_derivable(field_ty, &|home, name| ctx.enum_is_derivable(home, name)) {
            show_args.push(format!(
                "(&sky_runtime::stringify::Wrap(&self.{ident})).dispatch()"
            ));
        } else {
            // #87 seal: a non-derivable field (a function / opaque wrapper)
            // impls neither `SkyStringify` nor `Debug`, so `.dispatch()` would
            // not resolve. Render a `<fn>` placeholder for that `{}` slot.
            show_args.push("\"<fn>\"".to_owned());
        }
    }
    let fields_block = field_lines.join("\n");

    // Generic clauses: `<T1, T2>` on the struct, `<T1: SkyStringify + Debug, …>`
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
            .map(|p| format!("{p}: SkyStringify + std::fmt::Debug"))
            .collect();
        (
            format!("<{}>", params.join(", ")),
            format!("<{}>", bounds.join(", ")),
            format!("<{}>", params.join(", ")),
        )
    };

    // Go `%v` of a struct: `{v0 v1 ...}` — N space-separated `{}` placeholders
    // wrapped in literal braces. With zero fields the rendering is just `{}`.
    let body = if rec.fields.is_empty() {
        "\"{}\".to_string()".to_owned()
    } else {
        let placeholders = vec!["{}"; rec.fields.len()].join(" ");
        let fmt = format!("{{{{{placeholders}}}}}");
        format!("format!(\"{fmt}\", {})", show_args.join(", "))
    };

    // #87 seal: only a fully-derivable record takes the unconditional
    // `#[derive(Clone, Debug, PartialEq)]`. A record holding a first-class
    // function / opaque wrapper field (directly or through a carrier / a
    // non-derivable enum) cannot derive those traits — emit no auto derives (the
    // hand-written `SkyStringify` impl below still gives it a total string form).
    //
    // #93 seal: the serde derive is gated on `rec.is_serde` (the per-record serde
    // fixpoint), NOT `rec.is_derivable`. A CDPeq-but-not-serde record — e.g. a
    // view-helper `{ title : String, body : Html Msg }` in a Std.Live program —
    // keeps its `#[derive(Clone, Debug, PartialEq)]` but is NOT forced to
    // `serde::Serialize` / `Deserialize`, which would be an exit-0-then-cargo-fail
    // (E0277: `Html<Msg>: Serialize` unsatisfied). `is_serde ⇒ is_derivable`
    // (serde-OK leaves ⊂ derivable leaves), so serde is never added without CDPeq.
    // #91's app-entry Model gate independently rejects a non-serde type used AS a
    // Live/Tui/Webview Model; this gate covers every OTHER (non-Model) record.
    let serde_derives = if rec.is_serde && ctx.uses_live {
        ", serde::Serialize, serde::Deserialize"
    } else {
        ""
    };
    let derive_prefix = if rec.is_derivable {
        format!("#[derive(Clone, Debug, PartialEq{serde_derives})]\n")
    } else {
        String::new()
    };
    Ok(format!(
        "{derive_prefix}pub struct {name}{decl_clause} {{
{fields_block}
}}
impl{impl_bounds} SkyStringify for {name}{use_clause} {{
    fn sky_show(&self) -> String {{
        {body}
    }}
}}
"
    ))
}
