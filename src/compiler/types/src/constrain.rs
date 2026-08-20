//! Constraint generation, ported from the relevant arms of
//! `Ipe.Type.Constrain.Expression` (derivative of elm/compiler's
//! `Type.Constrain.Expression`, BSD-3-Clause).
//!
//! Walks the canonical module, minting a union-find variable for each
//! sub-expression region and emitting equality [`Constraint`]s that the solver
//! discharges. The arms modelled are exactly those the golden program
//! exercises: integer literals, `VarLocal` / `VarTopLevel` / `VarKernel` /
//! `VarCtor` references, function application (`Call`), `case`, and the binary
//! operators `+` / `-`.
//!
//! This module also owns the two bridges between the resolved [`Ty`] level and
//! the solver level: [`Builder::instantiate`] (a [`Ty`] → fresh union-find
//! structure) and [`Builder::zonk`] (a settled union-find variable → [`Ty`]).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use ipe_canon::ast as canon;
use ipe_diagnostics::{DResult, Diagnostic, Feature, LowerError, Span, TypeError};
use ipe_intern::{Interner, Symbol};
use ipe_kernels::{BuiltinTag, FieldTag, RowTailShape, SchemeKey, StdlibKernel, TyShape};

use crate::doc::{VarNamer, canon_type_to_doc, ty_to_doc};
use crate::solve::{Budget, Constraint};
use crate::ty::{
    Content, FlatType, RowTail, Ty, TyBounds, from_canon, is_solver_var, tag_solver_var,
};
use crate::unify::unify;
use crate::unionfind::{UnionFind, VarId};

/// `where_` tag for any `CompilerBug` raised during constraint generation.
const STAGE: &str = "ipe_types::constrain";

/// Recursively replace every `Ty::Var(v)` where `v` resolves to the `"any"`
/// wildcard AND `v` is NOT one of the union's declared type parameters with
/// `Dict String String` — the concrete pub/sub wire carrier.
///
/// Mirrors the reference's `any`-wildcard semantics for union-ctor field types:
/// the Haskell/Go backend carries `any` payloads as dynamic `interface{}`; the
/// Rust backend pins them to `Dict String String`, the sole concrete carrier that
/// satisfies `Clone + Debug + PartialEq + Serialize + DeserializeOwned`.
fn pin_any_in_ty(
    ty: Ty,
    union_vars: &[Symbol],
    interner: &Interner,
    dict: Symbol,
    string: Symbol,
) -> Ty {
    match ty {
        Ty::Var(v) => {
            let is_any = interner
                .resolve(Symbol::from_raw(v))
                .is_some_and(|n| n == "any");
            let is_declared = union_vars.iter().any(|uv| uv.as_raw() == v);
            if is_any && !is_declared {
                let mk_str = || Ty::Con {
                    module: Vec::new(),
                    name: string,
                    args: Vec::new(),
                };
                Ty::Con {
                    module: Vec::new(),
                    name: dict,
                    args: vec![mk_str(), mk_str()],
                }
            } else {
                Ty::Var(v)
            }
        }
        Ty::Fun(a, b) => Ty::Fun(
            Box::new(pin_any_in_ty(*a, union_vars, interner, dict, string)),
            Box::new(pin_any_in_ty(*b, union_vars, interner, dict, string)),
        ),
        Ty::Con { module, name, args } => Ty::Con {
            module,
            name,
            args: args
                .into_iter()
                .map(|a| pin_any_in_ty(a, union_vars, interner, dict, string))
                .collect(),
        },
        Ty::Unit => Ty::Unit,
        Ty::Tuple(elems) => Ty::Tuple(
            elems
                .into_iter()
                .map(|e| pin_any_in_ty(e, union_vars, interner, dict, string))
                .collect(),
        ),
        Ty::Record(fields, tail) => Ty::Record(
            fields
                .into_iter()
                .map(|(k, v)| (k, pin_any_in_ty(v, union_vars, interner, dict, string)))
                .collect(),
            tail,
        ),
    }
}

/// Per-binding polymorphic-variable entry: maps `(home_module, def_name)` to
/// the annotation-variable → rigid-VarId map for that definition.
///
/// Used in [`Builder::typed_rigids`] and re-exported via [`Generated::typed_rigids`]
/// so `SolvedTypes::poly_var_map` can build the lowerer's generic-variable lookup.
type PolyVarEntry = ((Vec<Symbol>, Symbol), BTreeMap<Symbol, VarId>);

/// Maximum number of nodes [`zonk`] reads back from a single type before
/// declaring it pathologically deep. The occurs check in unification rules out
/// true cycles, so this bound is only ever hit on adversarial input.
///
/// Kept deliberately **well under** the native-stack ceiling (a few thousand,
/// not the previous 100 000): the [`Ty`] this produces is then walked
/// recursively by the renderer ([`crate::doc::ty_to_doc`]), so capping the node
/// count here keeps that downstream recursion provably stack-safe. The
/// read-back itself is iterative (an explicit work stack), so it never grows the
/// native stack regardless of the bound.
const ZONK_NODE_LIMIT: u32 = 4_096;

/// Interned symbols for the built-in type constructors the inferencer needs to
/// name. `Int` / `String` usually already exist (from the source), but `Task`
/// never appears in source, so the builder interns them up front to
/// guarantee a stable, resolvable [`Symbol`] for each.
struct Builtins {
    int: Symbol,
    float: Symbol,
    bool: Symbol,
    string: Symbol,
    char: Symbol,
    task: Symbol,
    maybe: Symbol,
    result: Symbol,
    list: Symbol,
    /// Interned `Just` / `Nothing` / `Ok` / `Err` / `True` / `False` — the
    /// Prelude-exposed built-in constructor names.
    just: Symbol,
    nothing: Symbol,
    ok: Symbol,
    err: Symbol,
    true_: Symbol,
    false_: Symbol,
    /// `Ipe.Dict` type constructor symbol.
    dict: Symbol,
    /// `Ipe.Set` type constructor symbol.
    set: Symbol,
    /// `Ipe.Bytes` type constructor symbol.
    /// Divergence from Ipê: Bytes is a distinct primitive in Ipê-Rust (Vec<u8>),
    /// not a String alias as in the Go reference.
    bytes: Symbol,
    /// The interned `Error` symbol, used to validate the error channel in
    /// `Task Error a` annotations (normalised to unary `Task a`) and to pin the
    /// handler parameter type in `mapError` / `onError` so a bare lambda `\e ->
    /// ...` infers `e : Error` without leaving a free variable.
    error: Symbol,
    /// `ErrorKind` — the 11-variant classification carried by `Error`'s first
    /// field. Registered as a Prelude built-in exactly
    /// like `Order` — see `ipe_lower`'s `enum_variants`/`ctor_arity`
    /// (E-12), which already validate `Error kind info ->` patterns.
    errorkind: Symbol,
    /// The 11 `ErrorKind` nullary constructor symbols, in canon's registered
    /// index order (`crates/ipe_canon/src/env.rs`) — do not reorder.
    ek_io: Symbol,
    ek_network: Symbol,
    ek_ffi: Symbol,
    ek_decode: Symbol,
    ek_timeout: Symbol,
    ek_not_found: Symbol,
    ek_permission_denied: Symbol,
    ek_invalid_input: Symbol,
    ek_conflict: Symbol,
    ek_unavailable: Symbol,
    ek_unexpected: Symbol,
    /// `ErrorDetails` — the 5-variant enrichment union carried on
    /// `ErrorInfo.details`. Registered as a Prelude
    /// built-in exactly like `ErrorKind` — see `ipe_lower`'s
    /// `enum_variants`/`ctor_arity` seeding.
    errordetails: Symbol,
    /// The 5 `ErrorDetails` constructor symbols, in canon's registered index
    /// order (`crates/ipe_canon/src/env.rs`) — do not reorder.
    ed_ffi_panic: Symbol,
    ed_type_mismatch: Symbol,
    ed_http_status: Symbol,
    ed_json_decode: Symbol,
    ed_custom: Symbol,
    /// `PanicInfo` / `TypeInfo` / `ErrorInfo` — NOMINAL type-constructor
    /// symbols (SEAL fix, see
    /// `docs/adr/0017-error-payload-nominal-identity.md`). The three payload
    /// record types are opaque nominal Cons (like the server `Request`), NOT
    /// structural records: a bare record literal must not unify with them —
    /// the runtime backs them with concrete structs (`IpePanicInfo` /
    /// `IpeTypeInfo` / `IpeErrorInfo`), so a structural lowering
    /// (project-local synthesized struct) would fail `cargo build` after a
    /// clean `ipe` exit. Field ACCESS on them stays available through
    /// `resolve_deferred`'s builtin-record field tables.
    panicinfo: Symbol,
    typeinfo: Symbol,
    errorinfo: Symbol,
    /// Two distinct scheme type-variable symbols (`a`, `e`) used to build the
    /// built-in constructor schemes. Their identity links a constructor's
    /// payload to its result type, exactly like a user union's declared vars;
    /// each use site instantiates them fresh through one shared map.
    tv_a: Symbol,
    tv_e: Symbol,
    // ── Http field-name symbols ──────────────────────────────────────────────
    // Pre-interned because `kernel_ty` takes `&self` (the interner is immutable
    // at that point); these symbols give `Ty::Record` the correct BTreeMap keys
    // for `HttpResponse` and `HttpRequest` so the emit prepass registers both
    // record shapes.
    /// `"body"` — shared by `HttpResponse` and `HttpRequest`.
    http_f_body: Symbol,
    /// `"headers"` — shared by `HttpResponse` (`Dict String String`) and
    /// `HttpRequest` (`List (String, String)`).
    http_f_headers: Symbol,
    /// `"status"` — `HttpResponse` only.
    http_f_status: Symbol,
    /// `"method"` — `HttpRequest` only.
    http_f_method: Symbol,
    /// `"HttpMethod"` — the `Ipe.Http.HttpMethod` ADT type constructor.
    http_method: Symbol,
    /// `"url"` — `HttpRequest` only.
    http_f_url: Symbol,
    /// `"timeout"` — `HttpRequest` only.
    http_f_timeout: Symbol,
    /// `"followRedirects"` — `HttpRequest` only (camelCase Ipê field name).
    http_f_follow_redirects: Symbol,
    /// `"maxRedirects"` — `HttpRequest` only (camelCase Ipê field name).
    http_f_max_redirects: Symbol,
    /// `"contentType"` — `Ipe.Http.Server.Response` record field (camelCase).
    server_f_content_type: Symbol,
    /// `"name"` — `Ipe.Db.Migration` record field.
    migration_f_name: Symbol,
    /// `"sql"` — `Ipe.Db.Migration` record field.
    migration_f_sql: Symbol,
    // ── Db type symbols ──────────────────────────────────────────────────────
    /// `"Db"` — the opaque database connection pool type constructor.
    db: Symbol,
    /// `"SqlValue"` — the sum type for typed SQL parameter values.
    sqlvalue: Symbol,
    /// `"SqlField"` — the sum type for PATCH-style field-set / field-omit SQL params.
    sqlfield: Symbol,
    /// `"SqlFragment"` — `Ipe.Db.Sql`'s opaque, parameterized WHERE-fragment
    /// type.
    sqlfragment: Symbol,
    /// `"Secret"` — `Ipe.Secret`'s opaque, sealed secret-string wrapper
    /// type.
    secret: Symbol,
    /// `"Path"` — `Ipe.Path`'s opaque, validated filesystem-path type.
    path: Symbol,
    /// `"Regex"` — `Ipe.Regex`'s opaque compiled-pattern handle. Built ONLY by
    /// `Regex.compile : String -> Result Error Regex`. Zero type arguments.
    /// Lowered to `IrType::Regex`.
    regex: Symbol,
    // ── SqlValue constructor name symbols ─────────────────────────────────────
    sql_string: Symbol,
    sql_int: Symbol,
    sql_float: Symbol,
    sql_bool: Symbol,
    sql_bytes: Symbol,
    sql_time: Symbol,
    /// `"SqlDecimal"` — wraps a `String` decimal representation (lossless TEXT).
    sql_decimal: Symbol,
    /// `"SqlMoney"` — wraps a `String` in `"ISO_CODE AMOUNT"` format (TEXT).
    sql_money: Symbol,
    sql_null: Symbol,
    // ── SqlField constructor name symbols ─────────────────────────────────────
    set_field: Symbol,
    omit_field: Symbol,
    // ── Shared row-decoder type (JSON) ────────────────────────────────────────
    /// `"Decoder"` — the opaque decoder type constructor shared by `Ipe.Json.Decode`
    /// and `Ipe.Db.Decode`. Represented in the IR as `IrType::Decoder(Box<IrType>)`.
    decoder: Symbol,
    // ── TEA Cmd / Sub type constructor symbols ────────────────────────────────
    /// `"Cmd"` — the opaque command type constructor `Cmd msg`.
    /// Represented in the IR as `IrType::Cmd(Box<IrType>)`.
    cmd: Symbol,
    /// `"Sub"` — the opaque subscription type constructor `Sub msg`.
    /// Represented in the IR as `IrType::Sub(Box<IrType>)`.
    sub: Symbol,
    // ── Ipe.Http.Server opaque type constructor symbols ───────────────────────
    /// `"Request"` — the opaque server request type.
    server_request: Symbol,
    /// `"Response"` — the opaque server response type.
    server_response: Symbol,
    /// `"Route"` — the opaque server route type.
    server_route: Symbol,
    /// `"Cookie"` — the opaque server cookie type.
    server_cookie: Symbol,
    /// `"Handler"` — the `Request -> Task Error Response` alias from
    /// `Ipe.Http.Server`. Pre-interned so `constrain_def` can detect a
    /// `handler : Handler` annotation and expand it to the full arrow type
    /// before the parameter-loop runs (fixes IPE-T0004 for handler bindings).
    handler: Symbol,
    // ── Ipe.Http.Server.Stream opaque type constructor symbol ───────────
    /// `"StreamWriter"` — the opaque stream writer handle passed to the
    /// `Stream.stream` callback and consumed by `Stream.emit` /
    /// `Stream.finish` / `Stream.withContentType`.
    stream_writer: Symbol,
    // ── Ipe.Http.Server.WebSocket opaque type constructor symbols ────────
    /// `"WebSocketServer"` — the opaque per-peer WebSocket handle (`WsHandle`).
    ws_server: Symbol,
    /// `"WebSocketServerCfg"` — the opaque WebSocket server configuration
    /// (`WsServerCfg<IpeError>`).
    ws_server_cfg: Symbol,
    // ── Ipe.Ui / Ipe.Html parametric type constructor symbols ─────────────────
    /// `"Attribute"` — Ipe.Ui attribute type constructor `Attribute msg`.
    ///
    /// Used to build Ui kernel type schemes so the HM solver constrains
    /// `List (Attribute msg)` arguments (e.g. `layout [] child`) to a concrete
    /// element type rather than leaving them as free variables.  Without these
    /// entries the empty-attrs list `[]` keeps `List (Ty::Var)` as its region
    /// type, `list_elem_ir` returns `IrType::Json`, and `emit_list` emits the
    /// bare `Vec::new()` that Rust rejects with E0283 when M cannot be inferred
    /// from elsewhere in the expression.
    attribute: Symbol,
    /// `"Element"` — Ipe.Ui element type constructor `Element msg`.
    element: Symbol,
    /// `"Html"` — Html type constructor `Html msg` (shared by Ipe.Html and
    /// Ipe.Ui render entry points).
    html_con: Symbol,
    /// `"Length"` — Ipe.Ui nullary length type produced by `Ui.px` / `Ui.fill`
    /// / `Ui.minimum` / …. Lowered to `IrType::UiPlain(UiPlain::Length)` via the
    /// `"Length"` arm in `ipe_lower::ir_type_from_ty`.
    length: Symbol,
    /// `"Color"` — Ipe.Ui nullary colour type produced by `Ui.rgb` / `Ui.rgba`
    /// / `Ui.white` / …. Lowered to `IrType::UiPlain(UiPlain::Color)`.
    color: Symbol,
    /// `"Description"` — Ipe.Ui semantic description type produced by `Ui.descMain`
    /// / `Ui.descNavigation` / …. Lowered to `IrType::UiPlain(UiPlain::Description)`
    /// via the `"Description"` arm in `ipe_lower::ir_type_from_ty`.
    description: Symbol,
    /// `"PseudoClass"` — Ipe.Ui nullary pseudo-class-selector type produced by
    /// `Ui.hover` / `Ui.focus` / `Ui.focusVisible` / `Ui.active` / `Ui.disabled`
    /// and consumed by `Ui.onPseudo`. Lowered to
    /// `IrType::UiPlain(UiPlain::PseudoClass)` via the
    /// `"PseudoClass"` arm in `ipe_lower::ir_type_from_ty`.
    pseudo_class: Symbol,
    /// `"Value"` — the opaque JSON value type (`Value = any` in Ipê) produced /
    /// consumed by the `JsonEnc.*` encoders. Lowered to `IrType::Json`
    /// (`serde_json::Value`, re-exported as `JsonVal`) via the `"Value"` arm in
    /// `ipe_lower::ir_type_from_ty`. A distinct interned symbol so the `JsonEnc`
    /// scheme can produce a *concrete* `Value` region type (closing the former
    /// `Ty::Var(u32::MAX)` exit-0 hole) rather than leaning on the lowerer's
    /// free-`Ty::Var` → `Json` fallback.
    json_value: Symbol,
    /// `"wrapperAttrs"` — field name in the `Ui.layoutWith` config record.
    /// Pre-interned because `kernel_ty` builds a `Ty::Record` for the first
    /// argument of `Ui.layoutWith : { wrapperAttrs, rootAttrs } -> ...` and
    /// needs the key as a `Symbol`.
    lw_wrapper_attrs: Symbol,
    /// `"rootAttrs"` — the second field in the `Ui.layoutWith` config record.
    lw_root_attrs: Symbol,
    // ── Ipe.Web / Ipe.Web opaque type constructor symbols ───────────────────
    /// `"WebReq"` — opaque request threaded through `Web.app`'s `init`.
    web_req: Symbol,
    /// `"WebRoute"` — opaque route descriptor returned by `Web.route`.
    live_route_con: Symbol,
    // ── Web cfg record field name symbols ───────────────────────────────────────
    /// `"init"` — the init field of the `Web.app` config record.
    live_f_init: Symbol,
    /// `"update"` — the update field of the `Web.app` config record.
    live_f_update: Symbol,
    /// `"view"` — the view field of the `Web.app` config record.
    live_f_view: Symbol,
    /// `"subscriptions"` — the subscriptions field of the `Web.app` config record.
    live_f_subscriptions: Symbol,
    /// `"routes"` — the routes field of the `Web.appRouted` config record.
    /// Reserved for a future split scheme between `app` and `appRouted`.
    #[allow(dead_code)]
    live_f_routes: Symbol,
    /// `"notFound"` — the notFound field of the `Web.appRouted` config record.
    /// Reserved for a future split scheme between `app` and `appRouted`.
    #[allow(dead_code)]
    live_f_not_found: Symbol,
    // ── Tui cfg record field name symbols ─────────────────────────────────────
    /// `"onKey"` — the onKey field of the `Terminal.appScreen` config record.
    /// Typed `{ kind : String, value : String } -> msg`; the backend bridges the
    /// record handler onto the runtime bound `FOnKey: Fn(String, String) -> Msg`.
    tui_f_on_key: Symbol,
    /// `"kind"` — field of the pinned `KeyEvent` record in the `onKey` scheme.
    tui_f_key_kind: Symbol,
    /// `"value"` — field of the pinned `KeyEvent` record in the `onKey` scheme.
    tui_f_key_value: Symbol,
    // ── Webview cfg record field name symbols ─────────────────────────────────
    /// `"window"` — the window field of the `Webview.app` config record.
    /// Typed as a closed record `{ title : String, size : (Int, Int) }`.
    webview_f_window: Symbol,
    /// `"title"` — the title field inside the Webview window config record.
    webview_f_title: Symbol,
    /// `"size"` — the size field inside the Webview window config record.
    /// Typed as `(Int, Int)` — width × height in logical pixels.
    webview_f_size: Symbol,
    // ── Cli cfg record field name symbols ──────────────────────────────
    /// `"onLine"` — the onLine field of the `Terminal.appLines` config record.
    /// Typed as `String -> Msg` — called once per stdin line.
    cli_f_on_line: Symbol,
    // ── Ui.button cfg record field name symbols ───────────────────────────────
    /// `"onPress"` — the onPress field of the `Ui.button` config record.
    /// Typed as `Maybe msg`.
    btn_f_on_press: Symbol,
    /// `"label"` — the label field of the `Ui.button` config record.
    /// Typed as `Element msg`.
    btn_f_label: Symbol,
    // ── Ipe.Ui.Input type constructor + cfg field symbols ─────────────
    /// `"Label"` — the `Label msg` type constructor from `Ipe.Ui.Input`.
    /// Lowered to `IrType::Ui { ctor: UiCtor::Label, msg }`.
    input_label_con: Symbol,
    /// `"Placeholder"` — the `Placeholder msg` type constructor from `Ipe.Ui.Input`.
    /// Lowered to `IrType::Ui { ctor: UiCtor::Placeholder, msg }`.
    input_placeholder_con: Symbol,
    /// `"RadioOption"` — the `RadioOption msg` type constructor from `Ipe.Ui.Input`.
    /// Lowered to `IrType::Ui { ctor: UiCtor::RadioOption, msg }`.
    input_radio_option_con: Symbol,
    /// `"onChange"` — the onChange field of Input text/multiline/password cfg records.
    input_f_on_change: Symbol,
    /// `"text"` — the text field of text/multiline/email/username/search/password cfg records.
    input_f_text: Symbol,
    /// `"placeholder"` — the placeholder field of text-variant cfg records.
    input_f_placeholder: Symbol,
    /// `"checked"` — the checked field of the checkbox cfg record.
    input_f_checked: Symbol,
    /// `"icon"` — the icon field of the checkbox cfg record.
    input_f_icon: Symbol,
    /// `"spellcheck"` — the spellcheck field of the multiline cfg record.
    input_f_spellcheck: Symbol,
    /// `"value"` — the value field of the slider cfg record (current value as String).
    input_f_value: Symbol,
    /// `"min"` — the min field of the slider cfg record.
    input_f_min: Symbol,
    /// `"max"` — the max field of the slider cfg record.
    input_f_max: Symbol,
    /// `"step"` — the step field of the slider cfg record.
    input_f_step: Symbol,
    /// `"options"` — the options field of the radio/radioRow cfg record.
    input_f_options: Symbol,
    /// `"selected"` — the selected field of the radio/radioRow cfg record.
    input_f_selected: Symbol,
    // ── Ipe.Http.Stream opaque StreamId type constructor ─────────────────
    /// `"StreamId"` — the opaque stream identifier type constructor from
    /// `Ipe.Http.Stream`. Backed by `ipe_runtime::http_stream::IpeStreamId`.
    /// No synthetic `EnumDef` is injected; the backend handles it via a special
    /// case in `enum_name` that maps the symbol to the runtime struct.
    stream_id: Symbol,
    // ── Order ADT ─────────────────────────────────────────────────────
    /// `"Order"` — the type constructor for three-way comparison results.
    order: Symbol,
    /// `"LT"` — the LT constructor of the Order ADT (less-than).
    lt: Symbol,
    /// `"EQ"` — the EQ constructor of the Order ADT (equal).
    eq: Symbol,
    /// `"GT"` — the GT constructor of the Order ADT (greater-than).
    gt: Symbol,
    // ── Task.RetryPolicy field name symbols (retry surface) ───────────────────
    /// `"maxAttempts"` — maximum number of attempts in `RetryPolicy e`.
    retry_f_max_attempts: Symbol,
    /// `"baseMs"` — base delay in milliseconds in `RetryPolicy e`.
    retry_f_base_ms: Symbol,
    /// `"jitter"` — enable jitter flag in `RetryPolicy e`.
    retry_f_jitter: Symbol,
    /// `"kind"` — delay kind (0=linear, 1=exponential) in `RetryPolicy e`.
    retry_f_kind: Symbol,
    /// `"shouldRetry"` — predicate field `e -> Bool` in `RetryPolicy e`.
    retry_f_should_retry: Symbol,
    // ── Border/padding edge field name symbols (Border.widthEach) ────────────
    /// `"top"` — top edge field of `Border.widthEach { top, right, bottom, left }`.
    edge_f_top: Symbol,
    /// `"right"` — right edge field.
    edge_f_right: Symbol,
    /// `"bottom"` — bottom edge field.
    edge_f_bottom: Symbol,
    /// `"left"` — left edge field.
    edge_f_left: Symbol,
    // ── Border.shadow record field name symbols ──────────────────────────────
    /// `"offsetX"` — horizontal offset field of `Border.shadow { offsetX, … }`.
    shadow_f_offset_x: Symbol,
    /// `"offsetY"` — vertical offset field.
    shadow_f_offset_y: Symbol,
    /// `"blur"` — blur radius field.
    shadow_f_blur: Symbol,
    /// `"spread"` — spread field.
    shadow_f_spread: Symbol,
    /// `"color"` — shadow colour field.
    shadow_f_color: Symbol,
    // ── Ui.image record field name symbols ──────────────────────────────
    /// `"src"` — image source URL field of `Ui.image _ { src, description }`.
    img_f_src: Symbol,
    /// `"description"` — alt-text field of `Ui.image _ { src, description }`.
    img_f_description: Symbol,
    // ── JWT builder opaque type constructor symbols (D-00) ────────────────────
    /// `"Claims"` — opaque JWT claims builder object.  Backed at runtime by
    /// `serde_json::Value` (a JSON object accumulator).  Used as the input /
    /// output of the `Jwt.subject`, `Jwt.issuer`, … builder chain functions
    /// and the final `Jwt.encode` call.
    jwt_claims: Symbol,
    /// `"Algorithm"` — JWT signing algorithm descriptor.  Backed at runtime by
    /// a sealed `Ipe.Secret` wrapping the string `"HS256:<secret>"` or
    /// `"RS256:<pem>"`.  Built by `Jwt.hs256` / `Jwt.rs256` and consumed by
    /// `Jwt.encode` / `Jwt.decode`.
    jwt_algorithm: Symbol,
    // ── Ipe.Decimal opaque type constructor symbol ────────────────────────────
    /// `"Decimal"` — the opaque arbitrary-precision decimal type constructor
    /// from `Ipe.Decimal`.  Backed by `ipe_runtime::decimal::Decimal` (wrapping
    /// `rust_decimal::Decimal`).  Zero type arguments.  Lowered to
    /// `IrType::Decimal` by `ir_type_from_ty` / `ir_type_from_canon`.
    decimal: Symbol,
    // ── Ipe.Csv record field symbols ─────────────────────────────────────
    /// `"header"` — `Ipe.Csv.Csv.header : List String`.
    csv_f_header: Symbol,
    /// `"rows"` — `Ipe.Csv.Csv.rows : List (List String)`.
    csv_f_rows: Symbol,
    // ── Ipe.Cache record field symbols ───────────────────────────────────
    /// `"maxEntries"` — `Ipe.Cache.CacheCfg.maxEntries : Int`.
    cache_f_max_entries: Symbol,
    /// `"ttlMs"` — `Ipe.Cache.CacheCfg.ttlMs : Int`.
    cache_f_ttl_ms: Symbol,
    /// `"maxBytes"` — `Ipe.Cache.CacheCfg.maxBytes : Int`.
    cache_f_max_bytes: Symbol,
    /// `"hits"` — `Ipe.Cache.stats` return field `hits : Int`.
    cache_f_hits: Symbol,
    /// `"misses"` — `Ipe.Cache.stats` return field `misses : Int`.
    cache_f_misses: Symbol,
    /// `"evictions"` — `Ipe.Cache.stats` return field `evictions : Int`.
    cache_f_evictions: Symbol,
    // ── Ipe.WebSocket.WebSocketCfg record field symbols ─────────────
    /// `"url"` — `Ipe.WebSocket.WebSocketCfg.url : String`.
    ws_f_url: Symbol,
    /// `"headers"` — `WebSocketCfg.headers : List (String, String)`.
    ws_f_headers: Symbol,
    /// `"timeout"` — `WebSocketCfg.timeout : Int`.
    ws_f_timeout: Symbol,
    /// `"pingInterval"` — `WebSocketCfg.pingInterval : Int`.
    ws_f_ping_interval: Symbol,
    // ── Ipe.Email type + record field symbols ────────────────────────────
    /// `"EmailProvider"` — the opaque `Ipe.Email.EmailProvider` ADT constructor
    /// (`Resend`/`Ses`/`SendGrid`/`Smtp`).  Backed by
    /// `ipe_runtime::email::EmailProvider`; lowered to `IrType::EmailProvider`.
    email_provider: Symbol,
    /// `EmailMessage` record field names (`ipe_runtime::email::EmailMessage`).
    email_f_from: Symbol,
    email_f_to: Symbol,
    email_f_cc: Symbol,
    email_f_bcc: Symbol,
    email_f_subject: Symbol,
    email_f_text_body: Symbol,
    email_f_html_body: Symbol,
    email_f_attachments: Symbol,
    email_f_reply_to: Symbol,
    /// `Attachment` record field names (`ipe_runtime::email::EmailAttachment`)
    /// — the `attachments` element shape carried inside `EmailMessage`.
    email_f_filename: Symbol,
    email_f_mime_type: Symbol,
    email_f_content: Symbol,
    // `SesConfig` / `SmtpConfig` record shapes are folded by the lowerer via
    // field-name string constants (`ipe_lower`), not through a kernel scheme, so
    // no interned field symbols for them are needed here.
    // ── Ipe.Crypto typed-key newtypes ──────────────────────────────────────
    /// `"Key"` — opaque role-typed crypto key (`ipe_runtime::crypto::Key`).
    /// The ONLY constructor is `Key.fromString`/`Key.fromBytes`; no implicit
    /// `String` coercion. Lowered to `IrType::CryptoKey`.
    crypto_key: Symbol,
    /// `"Mac"` — opaque role-typed MAC output (`ipe_runtime::crypto::Mac`).
    /// Produced exclusively by `hmacSha256WithKey`/`hmacSha512WithKey`; extracted
    /// via `Mac.toHex`.  Lowered to `IrType::CryptoMac`.
    crypto_mac: Symbol,
    // ── Ipe.Email.EmailAddress ──────────────────────────────────────────────
    /// `"EmailAddress"` — opaque validated email address
    /// (`ipe_runtime::email::EmailAddress`).  The ONLY constructor is
    /// `EmailAddress.parse : String -> Maybe EmailAddress`; extracted via
    /// `EmailAddress.toString`.  Lowered to `IrType::EmailAddress`.
    email_address: Symbol,
    // ── Ipe.Url ─────────────────────────────────────────────────────────────
    /// `"Url"` — `Ipe.Url`'s opaque validated URL type (`ipe_runtime::url::Url`).
    /// The ONLY constructor is `Url.fromString : String -> Result Error Url`;
    /// extracted via `Url.toString`. Zero type arguments. Lowered to
    /// `IrType::Url`.
    url: Symbol,
    // ── Ipe.Db.Dsn ──────────────────────────────────────────────────────────
    /// `"Dsn"` — `Ipe.Db.Dsn`'s opaque validated connection descriptor
    /// (`ipe_runtime::dsn::Dsn`). Constructed only by `Db.Dsn.parse` /
    /// `Db.Dsn.build`; zero type arguments. Lowered to `IrType::Dsn`.
    dsn: Symbol,
    // ── Ipe.Db external Connection ──────────────────────────────────────────
    /// `"Connection"` — the external-DB connection handle constructor
    /// `Connection mode` (`ipe_runtime::external_conn::ExternalConnection`).
    /// Minted only by `Db.Dsn.open`. The phantom
    /// `mode` distinguishes `ReadOnly` from `ReadWrite` at inference and is
    /// erased at emit. Lowered to `IrType::Connection`.
    connection: Symbol,
    /// `"ReadOnly"` — the phantom read-only access-mode marker. Appears only as
    /// `Connection`'s argument; never a standalone value. Lowered to
    /// `IrType::ConnReadOnly`.
    conn_read_only: Symbol,
    /// `"ReadWrite"` — the phantom mutable access-mode marker. Appears only as
    /// `Connection`'s argument; never a standalone value. Lowered to
    /// `IrType::ConnReadWrite`.
    conn_read_write: Symbol,
    // ── Ipe.Locale ─────────────────────────────────────────────────────────
    /// `"Locale"` — opaque BCP-47 locale handle (`ipe_runtime::locale::Locale`).
    /// The ONLY constructor is `Locale.fromTag : String -> Maybe Locale`;
    /// extracted via `Locale.toTag : Locale -> String`.  Lowered to
    /// `IrType::Locale`.
    locale: Symbol,
    // ── Ipe.PubSub.Topic ───────────────────────────────────────────────────
    /// `"Topic"` — the phantom topic-handle type constructor `Topic a`.
    /// Erases to `String` at runtime (`ir_type_from_ty` maps `Topic a → Str`).
    /// Used only in kernel type schemes (`CmdPublish`/`SubSubscribeTopic`/
    /// `PubSubPublish`/`PubSubPublishNoEcho`/`PubSubTopic`) to share the
    /// payload type variable `a` between publisher and subscriber.
    topic_con: Symbol,
}

impl Builtins {
    #[allow(clippy::too_many_lines)] // declarative intern table — each field listed explicitly for exhaustiveness
    fn new(interner: &mut Interner) -> DResult<Self> {
        Ok(Self {
            int: interner.intern("Int")?,
            float: interner.intern("Float")?,
            bool: interner.intern("Bool")?,
            string: interner.intern("String")?,
            char: interner.intern("Char")?,
            task: interner.intern("Task")?,
            maybe: interner.intern("Maybe")?,
            result: interner.intern("Result")?,
            list: interner.intern("List")?,
            dict: interner.intern("Dict")?,
            set: interner.intern("Set")?,
            bytes: interner.intern("Bytes")?,
            just: interner.intern("Just")?,
            nothing: interner.intern("Nothing")?,
            ok: interner.intern("Ok")?,
            err: interner.intern("Err")?,
            true_: interner.intern("True")?,
            false_: interner.intern("False")?,
            error: interner.intern("Error")?,
            errorkind: interner.intern("ErrorKind")?,
            ek_io: interner.intern("Io")?,
            ek_network: interner.intern("Network")?,
            ek_ffi: interner.intern("Ffi")?,
            ek_decode: interner.intern("Decode")?,
            ek_timeout: interner.intern("Timeout")?,
            ek_not_found: interner.intern("NotFound")?,
            ek_permission_denied: interner.intern("PermissionDenied")?,
            ek_invalid_input: interner.intern("InvalidInput")?,
            ek_conflict: interner.intern("Conflict")?,
            ek_unavailable: interner.intern("Unavailable")?,
            ek_unexpected: interner.intern("Unexpected")?,
            errordetails: interner.intern("ErrorDetails")?,
            ed_ffi_panic: interner.intern("FfiPanic")?,
            ed_type_mismatch: interner.intern("TypeMismatch")?,
            ed_http_status: interner.intern("HttpStatus")?,
            ed_json_decode: interner.intern("JsonDecode")?,
            ed_custom: interner.intern("Custom")?,
            panicinfo: interner.intern("PanicInfo")?,
            typeinfo: interner.intern("TypeInfo")?,
            errorinfo: interner.intern("ErrorInfo")?,
            tv_a: interner.intern("a")?,
            tv_e: interner.intern("e")?,
            // Http field names (camelCase, as they appear in Ipê source).
            http_f_body: interner.intern("body")?,
            http_f_headers: interner.intern("headers")?,
            http_f_status: interner.intern("status")?,
            server_f_content_type: interner.intern("contentType")?,
            migration_f_name: interner.intern("name")?,
            migration_f_sql: interner.intern("sql")?,
            http_f_method: interner.intern("method")?,
            http_method: interner.intern("HttpMethod")?,
            http_f_url: interner.intern("url")?,
            http_f_timeout: interner.intern("timeout")?,
            http_f_follow_redirects: interner.intern("followRedirects")?,
            http_f_max_redirects: interner.intern("maxRedirects")?,
            // Db symbols.
            db: interner.intern("Db")?,
            sqlvalue: interner.intern("SqlValue")?,
            sqlfield: interner.intern("SqlField")?,
            sqlfragment: interner.intern("SqlFragment")?,
            secret: interner.intern("Secret")?,
            path: interner.intern("Path")?,
            regex: interner.intern("Regex")?,
            sql_string: interner.intern("SqlString")?,
            sql_int: interner.intern("SqlInt")?,
            sql_float: interner.intern("SqlFloat")?,
            sql_bool: interner.intern("SqlBool")?,
            sql_bytes: interner.intern("SqlBytes")?,
            sql_time: interner.intern("SqlTime")?,
            sql_decimal: interner.intern("SqlDecimal")?,
            sql_money: interner.intern("SqlMoney")?,
            sql_null: interner.intern("SqlNull")?,
            set_field: interner.intern("SetField")?,
            omit_field: interner.intern("OmitField")?,
            decoder: interner.intern("Decoder")?,
            // TEA Cmd / Sub type constructors.
            cmd: interner.intern("Cmd")?,
            sub: interner.intern("Sub")?,
            // Ipe.Http.Server opaque types.
            server_request: interner.intern("Request")?,
            server_response: interner.intern("Response")?,
            server_route: interner.intern("Route")?,
            server_cookie: interner.intern("Cookie")?,
            handler: interner.intern("Handler")?,
            // Ipe.Http.Server.Stream opaque handle.
            stream_writer: interner.intern("StreamWriter")?,
            // Ipe.Http.Server.WebSocket opaque handles.
            ws_server: interner.intern("WebSocketServer")?,
            ws_server_cfg: interner.intern("WebSocketServerCfg")?,
            // Ipe.Ui / Ipe.Html parametric type constructor symbols.
            attribute: interner.intern("Attribute")?,
            element: interner.intern("Element")?,
            html_con: interner.intern("Html")?,
            length: interner.intern("Length")?,
            color: interner.intern("Color")?,
            description: interner.intern("Description")?,
            pseudo_class: interner.intern("PseudoClass")?,
            json_value: interner.intern("Value")?,
            lw_wrapper_attrs: interner.intern("wrapperAttrs")?,
            lw_root_attrs: interner.intern("rootAttrs")?,
            // Ipe.Web / Ipe.Web opaque types + cfg field names.
            web_req: interner.intern("WebReq")?,
            live_route_con: interner.intern("WebRoute")?,
            live_f_init: interner.intern("init")?,
            live_f_update: interner.intern("update")?,
            live_f_view: interner.intern("view")?,
            live_f_subscriptions: interner.intern("subscriptions")?,
            live_f_routes: interner.intern("routes")?,
            live_f_not_found: interner.intern("notFound")?,
            // Tui cfg field names.
            tui_f_on_key: interner.intern("onKey")?,
            tui_f_key_kind: interner.intern("kind")?,
            tui_f_key_value: interner.intern("value")?,
            // Webview cfg field names.
            webview_f_window: interner.intern("window")?,
            webview_f_title: interner.intern("title")?,
            webview_f_size: interner.intern("size")?,
            // Cli cfg field names.
            cli_f_on_line: interner.intern("onLine")?,
            // Ui.button cfg field names.
            btn_f_on_press: interner.intern("onPress")?,
            btn_f_label: interner.intern("label")?,
            // Ipe.Ui.Input type constructors + cfg field names.
            input_label_con: interner.intern("Label")?,
            input_placeholder_con: interner.intern("Placeholder")?,
            input_radio_option_con: interner.intern("RadioOption")?,
            input_f_on_change: interner.intern("onChange")?,
            input_f_text: interner.intern("text")?,
            input_f_placeholder: interner.intern("placeholder")?,
            input_f_checked: interner.intern("checked")?,
            input_f_icon: interner.intern("icon")?,
            input_f_spellcheck: interner.intern("spellcheck")?,
            // Ipe.Ui.Input.slider cfg fields.
            input_f_value: interner.intern("value")?,
            input_f_min: interner.intern("min")?,
            input_f_max: interner.intern("max")?,
            input_f_step: interner.intern("step")?,
            // Ipe.Ui.Input.radio / radioRow cfg fields.
            input_f_options: interner.intern("options")?,
            input_f_selected: interner.intern("selected")?,
            // Ipe.Http.Stream: StreamId opaque handle type.
            stream_id: interner.intern("StreamId")?,
            csv_f_header: interner.intern("header")?,
            csv_f_rows: interner.intern("rows")?,
            // ── Ipe.Cache record field symbols ──────────────────────────
            cache_f_max_entries: interner.intern("maxEntries")?,
            cache_f_ttl_ms: interner.intern("ttlMs")?,
            cache_f_max_bytes: interner.intern("maxBytes")?,
            cache_f_hits: interner.intern("hits")?,
            cache_f_misses: interner.intern("misses")?,
            cache_f_evictions: interner.intern("evictions")?,
            // ── Ipe.WebSocket.WebSocketCfg record field symbols ────
            ws_f_url: interner.intern("url")?,
            ws_f_headers: interner.intern("headers")?,
            ws_f_timeout: interner.intern("timeout")?,
            ws_f_ping_interval: interner.intern("pingInterval")?,
            // ── Ipe.Email type + record field symbols ───────────────────
            email_provider: interner.intern("EmailProvider")?,
            email_f_from: interner.intern("from")?,
            email_f_to: interner.intern("to")?,
            email_f_cc: interner.intern("cc")?,
            email_f_bcc: interner.intern("bcc")?,
            email_f_subject: interner.intern("subject")?,
            email_f_text_body: interner.intern("textBody")?,
            email_f_html_body: interner.intern("htmlBody")?,
            email_f_attachments: interner.intern("attachments")?,
            email_f_reply_to: interner.intern("replyTo")?,
            email_f_filename: interner.intern("filename")?,
            email_f_mime_type: interner.intern("mimeType")?,
            email_f_content: interner.intern("content")?,
            // ── Order ADT ─────────────────────────────────────────────
            order: interner.intern("Order")?,
            lt: interner.intern("LT")?,
            eq: interner.intern("EQ")?,
            gt: interner.intern("GT")?,
            // ── Task.RetryPolicy field names (retry surface) ─────────────────────
            retry_f_max_attempts: interner.intern("maxAttempts")?,
            retry_f_base_ms: interner.intern("baseMs")?,
            retry_f_jitter: interner.intern("jitter")?,
            retry_f_kind: interner.intern("kind")?,
            retry_f_should_retry: interner.intern("shouldRetry")?,
            // ── Border/padding edge field names (Border.widthEach) ───────────────
            edge_f_top: interner.intern("top")?,
            edge_f_right: interner.intern("right")?,
            edge_f_bottom: interner.intern("bottom")?,
            edge_f_left: interner.intern("left")?,
            // ── Border.shadow record field names ─────────────────────────────────
            shadow_f_offset_x: interner.intern("offsetX")?,
            shadow_f_offset_y: interner.intern("offsetY")?,
            shadow_f_blur: interner.intern("blur")?,
            shadow_f_spread: interner.intern("spread")?,
            shadow_f_color: interner.intern("color")?,
            // ── Ui.image record field names ────────────────────────────────
            img_f_src: interner.intern("src")?,
            img_f_description: interner.intern("description")?,
            // ── JWT builder opaque type constructor symbols (D-00) ──────────────
            jwt_claims: interner.intern("Claims")?,
            jwt_algorithm: interner.intern("Algorithm")?,
            // ── Ipe.Decimal opaque type constructor ──────────────────────────────
            decimal: interner.intern("Decimal")?,
            // ── Ipe.Crypto typed-key newtypes ────────────────────────────────────
            crypto_key: interner.intern("Key")?,
            crypto_mac: interner.intern("Mac")?,
            // ── Ipe.Email.EmailAddress ────────────────────────────────────────────
            email_address: interner.intern("EmailAddress")?,
            // ── Ipe.Url ───────────────────────────────────────────────────────────
            url: interner.intern("Url")?,
            // ── Ipe.Db.Dsn ────────────────────────────────────────────────────────
            dsn: interner.intern("Dsn")?,
            connection: interner.intern("Connection")?,
            conn_read_only: interner.intern("ReadOnly")?,
            conn_read_write: interner.intern("ReadWrite")?,
            // ── Ipe.Locale ───────────────────────────────────────────────────────
            locale: interner.intern("Locale")?,
            // ── Ipe.PubSub.Topic ────────────────────────────────────────────────
            topic_con: interner.intern("Topic")?,
        })
    }

    /// The Prelude-built-in constructor schemes, keyed by constructor name.
    ///
    /// `Bool` (`True` / `False` : `Bool`), `Maybe a` (`Just : a -> Maybe a`,
    /// `Nothing : Maybe a`), and `Result e a` (`Ok : a -> Result e a`,
    /// `Err : e -> Result e a`). These types have no user `type` declaration, so
    /// their schemes are synthesised here; each is instantiated fresh per use
    /// site exactly like a user constructor's scheme. The built-in `Con`s carry
    /// an empty module path, matching how `from_canon` renders the builtin type
    /// names (`Int` / `Bool` / …) and how the lowerer recognises them by name.
    #[allow(clippy::too_many_lines)]
    fn ctor_schemes(&self) -> Vec<(Symbol, CtorScheme)> {
        let bool_ty = Ty::Con {
            module: Vec::new(),
            name: self.bool,
            args: Vec::new(),
        };
        let maybe_ty = Ty::Con {
            module: Vec::new(),
            name: self.maybe,
            args: vec![Ty::Var(self.tv_a.as_raw())],
        };
        let result_ty = Ty::Con {
            module: Vec::new(),
            name: self.result,
            args: vec![Ty::Var(self.tv_e.as_raw()), Ty::Var(self.tv_a.as_raw())],
        };
        // Monomorphic SqlValue / SqlField types (no type parameters).
        let sqlvalue_ty = Ty::Con {
            module: Vec::new(),
            name: self.sqlvalue,
            args: Vec::new(),
        };
        let sqlfield_ty = Ty::Con {
            module: Vec::new(),
            name: self.sqlfield,
            args: Vec::new(),
        };
        let int_ty = Ty::Con {
            module: Vec::new(),
            name: self.int,
            args: Vec::new(),
        };
        let float_ty = Ty::Con {
            module: Vec::new(),
            name: self.float,
            args: Vec::new(),
        };
        let string_ty = Ty::Con {
            module: Vec::new(),
            name: self.string,
            args: Vec::new(),
        };
        let bool_ty_plain = Ty::Con {
            module: Vec::new(),
            name: self.bool,
            args: Vec::new(),
        };
        let bytes_ty = Ty::Con {
            module: Vec::new(),
            name: self.bytes,
            args: Vec::new(),
        };
        // Monomorphic `Error` / `ErrorKind` — no type params.
        let error_ty = Ty::Con {
            module: Vec::new(),
            name: self.error,
            args: Vec::new(),
        };
        let errorkind_ty = Ty::Con {
            module: Vec::new(),
            name: self.errorkind,
            args: Vec::new(),
        };
        // Monomorphic `ErrorDetails` — no type params.
        let errordetails_ty = Ty::Con {
            module: Vec::new(),
            name: self.errordetails,
            args: Vec::new(),
        };
        // `PanicInfo` / `TypeInfo` / `ErrorInfo` — NOMINAL opaque Cons, not
        // structural records (SEAL fix; see the `TypeNames` field
        // doc). A bare record literal (`FfiPanic { message = …, stack = … }`)
        // now fails to unify with a clean ipe-time type mismatch instead of
        // lowering to a synthesized struct that fails `cargo build` against
        // the runtime's `IpePanicInfo`/`IpeTypeInfo`/`IpeErrorInfo`. Field
        // access on values of these types resolves through
        // `resolve_deferred`'s builtin-record field tables (the `Request`
        // recipe); construction from Ipê source goes through the smart
        // constructors (`Error.io`/… + `Error.withDetails`) only.
        let panic_info_ty = Ty::Con {
            module: Vec::new(),
            name: self.panicinfo,
            args: Vec::new(),
        };
        let type_info_ty = Ty::Con {
            module: Vec::new(),
            name: self.typeinfo,
            args: Vec::new(),
        };
        let error_info_ty = Ty::Con {
            module: Vec::new(),
            name: self.errorinfo,
            args: Vec::new(),
        };
        vec![
            (
                self.true_,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: bool_ty.clone(),
                },
            ),
            (
                self.false_,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: bool_ty,
                },
            ),
            (
                self.just,
                CtorScheme {
                    arg_tys: vec![Ty::Var(self.tv_a.as_raw())],
                    result: maybe_ty.clone(),
                },
            ),
            (
                self.nothing,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: maybe_ty,
                },
            ),
            (
                self.ok,
                CtorScheme {
                    arg_tys: vec![Ty::Var(self.tv_a.as_raw())],
                    result: result_ty.clone(),
                },
            ),
            (
                self.err,
                CtorScheme {
                    arg_tys: vec![Ty::Var(self.tv_e.as_raw())],
                    result: result_ty,
                },
            ),
            // ── Error / ErrorKind constructors ──────────────
            // `Error : ErrorKind -> ErrorInfo -> Error` — without it the
            // no-scheme ctor-pattern fallback would bind `info` to an untied
            // fresh var. `ErrorKind`'s 11 variants are all nullary.
            (
                self.error,
                CtorScheme {
                    arg_tys: vec![errorkind_ty.clone(), error_info_ty],
                    result: error_ty,
                },
            ),
            (
                self.ek_io,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_network,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_ffi,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_decode,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_timeout,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_not_found,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_permission_denied,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_invalid_input,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_conflict,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_unavailable,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty.clone(),
                },
            ),
            (
                self.ek_unexpected,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: errorkind_ty,
                },
            ),
            // ── ErrorDetails constructors ───────────────
            // `FfiPanic : PanicInfo -> ErrorDetails`
            // `TypeMismatch : TypeInfo -> ErrorDetails`
            // `HttpStatus : Int -> ErrorDetails`
            // `JsonDecode : String -> ErrorDetails`
            // `Custom : String -> ErrorDetails`
            (
                self.ed_ffi_panic,
                CtorScheme {
                    arg_tys: vec![panic_info_ty],
                    result: errordetails_ty.clone(),
                },
            ),
            (
                self.ed_type_mismatch,
                CtorScheme {
                    arg_tys: vec![type_info_ty],
                    result: errordetails_ty.clone(),
                },
            ),
            (
                self.ed_http_status,
                CtorScheme {
                    arg_tys: vec![int_ty.clone()],
                    result: errordetails_ty.clone(),
                },
            ),
            (
                self.ed_json_decode,
                CtorScheme {
                    arg_tys: vec![string_ty.clone()],
                    result: errordetails_ty.clone(),
                },
            ),
            (
                self.ed_custom,
                CtorScheme {
                    arg_tys: vec![string_ty.clone()],
                    result: errordetails_ty,
                },
            ),
            // ── SqlValue constructors ──────────────────────────────────────────
            // Each maps its payload type → SqlValue.
            (
                self.sql_string,
                CtorScheme {
                    arg_tys: vec![string_ty.clone()],
                    result: sqlvalue_ty.clone(),
                },
            ),
            (
                self.sql_int,
                CtorScheme {
                    arg_tys: vec![int_ty.clone()],
                    result: sqlvalue_ty.clone(),
                },
            ),
            (
                self.sql_float,
                CtorScheme {
                    arg_tys: vec![float_ty],
                    result: sqlvalue_ty.clone(),
                },
            ),
            (
                self.sql_bool,
                CtorScheme {
                    arg_tys: vec![bool_ty_plain],
                    result: sqlvalue_ty.clone(),
                },
            ),
            (
                self.sql_bytes,
                CtorScheme {
                    arg_tys: vec![bytes_ty],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // SqlTime wraps a Unix-millisecond Int timestamp.
            (
                self.sql_time,
                CtorScheme {
                    arg_tys: vec![int_ty],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // SqlDecimal wraps a String decimal representation (lossless TEXT
            // serialisation matching Go's shopspring.Decimal.String()).
            // Minimal wiring: Ipê users write `SqlDecimal "1234.56"` rather than
            // a native Decimal value (native Decimal is not yet an IrType).
            (
                self.sql_decimal,
                CtorScheme {
                    arg_tys: vec![string_ty.clone()],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // SqlMoney wraps a String in "ISO_CODE AMOUNT" format (TEXT).
            // Minimal wiring matching Go's sqlMoneyToString / db_decode_money.
            // Ipê users write `SqlMoney "USD 1234.56"`.
            (
                self.sql_money,
                CtorScheme {
                    arg_tys: vec![string_ty],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // SqlNull wraps another SqlValue as a type-level witness; the inner
            // value is discarded (a NULL carries no value) but its VARIANT TAG
            // is threaded through `into_sql_param()` → `SqlParam::Null(Box<SqlParam>)`
            // so the bind site can select a correctly-typed `Option::<T>::None`
            // (load-bearing on Postgres, whose extended query protocol validates
            // a per-param type-OID hint against the target column — Class 7 §4a).
            (
                self.sql_null,
                CtorScheme {
                    arg_tys: vec![sqlvalue_ty.clone()],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // ── SqlField constructors ──────────────────────────────────────────
            // SetField : SqlValue -> SqlField — wraps a typed parameter value.
            (
                self.set_field,
                CtorScheme {
                    arg_tys: vec![sqlvalue_ty],
                    result: sqlfield_ty.clone(),
                },
            ),
            // OmitField : SqlField — nullary; column is omitted from generated SQL.
            (
                self.omit_field,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: sqlfield_ty,
                },
            ),
            // ── Order constructors ──────────────────────────────────
            // LT, EQ, GT are all nullary: no payload, result is Order.
            (
                self.lt,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: Ty::Con {
                        module: Vec::new(),
                        name: self.order,
                        args: Vec::new(),
                    },
                },
            ),
            (
                self.eq,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: Ty::Con {
                        module: Vec::new(),
                        name: self.order,
                        args: Vec::new(),
                    },
                },
            ),
            (
                self.gt,
                CtorScheme {
                    arg_tys: Vec::new(),
                    result: Ty::Con {
                        module: Vec::new(),
                        name: self.order,
                        args: Vec::new(),
                    },
                },
            ),
        ]
    }
}

/// The type discipline a binary operator imposes. Classified once from the
/// resolved kernel name so the constraint walk doesn't re-borrow the interner.
#[derive(Clone, Copy)]
enum BinopClass {
    /// `//`: integer division `Int -> Int -> Int`.
    IntDiv,
    /// `/`: `Float -> Float -> Float` (matches the Go backend's float division).
    FloatDiv,
    /// `+ - *`: `Number a => a -> a -> a`. The operands and the result share one
    /// numeric variable carrying the named obligation, so the operation stays
    /// generic over `Int` / `Float` until a concrete operand pins it.
    Num(TyBounds),
    /// `< > <= >=`: `Comparable a => a -> a -> Bool` — operands share one
    /// ordered type; the result is `Bool`.
    Order,
    /// `== /=`: `Equatable a => a -> a -> Bool` — operands share one equatable
    /// type (structural equality is total over every non-function type); the
    /// result is `Bool`. The shared variable carries the equality obligation, so
    /// a generalised use emits a Rust `PartialEq` bound.
    Equality,
    /// `&& ||`: `Bool -> Bool -> Bool`.
    Boolean,
    /// `++`: `String -> String -> String`. The general `Appendable` super-type
    /// (which would also cover `List a -> List a -> List a`) is a later batch;
    /// for now both operands and the result are pinned to `String`, so applying
    /// `++` to any other type (a would-be `List`) is a fail-closed type error
    /// rather than a mis-typed pass-through.
    Append,
    /// Any other operator (`::`, …): `a -> a -> a`. The numeric/ordering
    /// super-types do not cover list cons, so it stays a plain pass-through here
    /// and is gated at lowering rather than mis-typed.
    Poly,
}

/// Classify a resolved operator kernel name (`add`, `eq`, `and`, …).
const fn classify_binop(func: &str) -> BinopClass {
    match func.as_bytes() {
        b"add" => BinopClass::Num(TyBounds::add()),
        b"sub" => BinopClass::Num(TyBounds::sub()),
        b"mul" => BinopClass::Num(TyBounds::mul()),
        b"idiv" => BinopClass::IntDiv,
        b"fdiv" => BinopClass::FloatDiv,
        b"lt" | b"gt" | b"le" | b"ge" => BinopClass::Order,
        b"eq" | b"neq" => BinopClass::Equality,
        b"and" | b"or" => BinopClass::Boolean,
        b"append" => BinopClass::Append,
        _ => BinopClass::Poly,
    }
}

/// The constraint-generation state threaded through the walk.
pub struct Builder<'a> {
    uf: &'a mut UnionFind<Content>,
    interner: &'a Interner,
    builtins: Builtins,
    /// Resolved type per source region, keyed by `(home_module_path, Span)`.
    ///
    /// The home path discriminant prevents span collisions after `link::link`
    /// merges N source modules into a single flat def list: two different files
    /// may independently contain expressions at the same byte-offset span.  The
    /// bare-`Span` key (pre-fix) silently overwrote earlier entries, causing the
    /// lowerer to read the wrong type and produce IPE-I0001.
    regions: BTreeMap<(Vec<Symbol>, Span), VarId>,
    /// The type EXPECTED at each source region by its surrounding context,
    /// keyed by `(home_module_path, Span)` — the type-directed-completion
    /// sidecar (ADR 0034 / LSP plan §6). Where [`Self::regions`] records the
    /// type an expression WAS inferred to have, this records the type its
    /// enclosing context PUSHES DOWN onto it: a `Call` argument's declared
    /// parameter slot, a typed def body's annotation return, an `if` branch's
    /// shared result, a `let` binding's pattern, a list/cons element. Recording
    /// an already-created solver variable is a pure map insert — it adds NO
    /// constraint and NO variable, so `SolvedTypes`'s existing fields are
    /// byte-identical whether or not this map is populated (additivity proven
    /// by `expected_types_additive` in `lib.rs`). Only positions with a genuine
    /// contextual expectation appear; an unconstrained position (a bare
    /// top-level body, a lambda not in an annotated context) is absent, and the
    /// completion provider degrades to scope-only ranking there.
    expected: BTreeMap<(Vec<Symbol>, Span), VarId>,
    /// Home module path of the def currently being constrained.  Set at the
    /// start of each `constrain_def` call; read by every `regions.insert`.
    current_home: Vec<Symbol>,
    /// Equality constraints to be discharged by the solver.
    constraints: Vec<Constraint>,
    /// Annotation-derived types of every top-level binding, for cross-binding
    /// references (`main` mentions `update`).
    ///
    /// Keyed by `(home_module_path, bare_name)` — not bare `Symbol` alone — so
    /// same-named defs from different modules (e.g. `Lib.helper` and
    /// `Main.helper`) never overwrite each other after `link::link` merges them
    /// into one flat def list.  Every `VarTopLevel { module, name }` reference
    /// looks up its home module's entry, not an entry that may belong to a
    /// different module that happens to share the bare name.
    /// Values are `Rc` so a typed top-level reference clones a refcount, not
    /// the whole annotation `Ty` tree (efficiency-audit §2/§7 medium).
    /// `instantiate_tracked` only reads the scheme; resolved types are
    /// byte-identical. Single-threaded solver → `Rc` suffices.
    top_level: BTreeMap<(Vec<Symbol>, Symbol), Rc<Ty>>,
    /// Body region-var of each untyped top-level binding, read back for `env`.
    ///
    /// Keyed by `(home_module_path, bare_name)` for the same reason as
    /// [`Self::top_level`].
    untyped: BTreeMap<(Vec<Symbol>, Symbol), VarId>,
    /// Deferred record field-access obligations, resolved after the main solve.
    field_accesses: Vec<FieldAccess>,
    /// Deferred record-update obligations, resolved after the main solve.
    record_updates: Vec<RecordUpdate>,
    /// Deferred routed-Web.app type checks, resolved after the main solve.
    routed_web_checks: Vec<RoutedWebCheck>,
    /// Deferred per-route page-witness checks (one per `Web.route` reference),
    /// resolved after the main solve, BEFORE the routed-Web.app checks.
    route_witness_checks: Vec<RouteWitnessCheck>,
    /// Body result var of every typed top-level binding whose RETURN annotation
    /// is the bare wildcard `any`. Keyed by `(home_module_path, bare_name)`.
    /// A wildcard `any` return severs the body's settled type from every use
    /// site (each occurrence instantiates its own fresh flex); this map is the
    /// handle [`Self::tie_wildcard_any_uses_to_bodies`] uses to re-connect them.
    wildcard_any_return_bodies: BTreeMap<(Vec<Symbol>, Symbol), VarId>,
    /// Names of typed bindings whose RETURN annotation is the bare wildcard
    /// `any`, recorded in the registration pass. Each reference to one of these
    /// (in [`Self::constrain_var_top_level`]) records a use tie so its body can
    /// flow to the use site.
    wildcard_any_return_bindings: BTreeSet<(Vec<Symbol>, Symbol)>,
    /// One entry per reference to a wildcard-`any`-return binding: the use's
    /// instantiated arrow var + the binding it references. Tied to the binding's
    /// body by [`Self::tie_wildcard_any_uses_to_bodies`] once every def is
    /// constrained.
    wildcard_any_use_results: Vec<(VarId, (Vec<Symbol>, Symbol))>,
    /// The type scheme of every data constructor declared in this module, keyed
    /// by constructor name. A constructor is a (possibly generic) function
    /// `field0 -> … -> fieldN -> T vars`; each use site instantiates the scheme
    /// fresh, exactly as a polymorphic top-level binding does.
    /// Each value is an `Rc` so per-use-site instantiation clones a refcount,
    /// not the whole scheme (efficiency-audit §2 medium: the constructor-ref /
    /// ctor-pattern checks deep-cloned the full `CtorScheme` per use to
    /// release the `&self` borrow before the `&mut self` instantiate call).
    /// The `Rc` holds byte-identical data — same fresh vars, same constraints,
    /// same errors. Fully internal to `Builder`.
    ctors: BTreeMap<Symbol, Rc<CtorScheme>>,
    /// One entry per typed binding: its `(home, name)` and the rigid (skolem)
    /// variable each of its annotation type variables instantiated to while its
    /// body was checked. Read post-solve to recover each variable's super-type
    /// obligations (the bounds the body imposed) for generalisation, and to build
    /// `SolvedTypes::poly_var_map` (the per-binding generic-variable map the
    /// lowerer uses to distinguish enclosing-generic `Ty::Var`s from
    /// message-free `Ty::Var`s inside UI attribute lists).
    typed_rigids: Vec<PolyVarEntry>,
    /// One entry per *reference* to a typed top-level binding (each `VarTopLevel`
    /// use site), recording how that use instantiated the binding's scheme. Used
    /// post-solve to check a super-typed binding's obligations against the
    /// concrete type each use pins it to.
    scheme_apps: Vec<SchemeApp>,
    /// Every super-typed flex variable minted by a numeric / ordering / equality
    /// operator, paired with the obligations it was minted with and the operand
    /// span to blame. Read post-solve for two jobs: numeric defaulting (an
    /// unpinned `Number` variable resolves to `Int`, matching the reference
    /// compiler's defaulting of an otherwise-unconstrained `number`) and the
    /// concrete-pin soundness gate (a variable that pinned to a concrete type
    /// during solving must be one the operation truly supports — an equality
    /// obligation rejects a type containing a function, which Rust cannot
    /// compare, with IPE-T0014 rather than emitting code `cargo` rejects).
    super_vars: Vec<(VarId, TyBounds, Span)>,
    /// One entry per *cross-module* reference to an untyped top-level binding
    /// (`Builder::current_home != source.0`). A same-module reference keeps
    /// sharing `untyped[key]` directly (unchanged monomorphic-within-module
    /// behaviour); a cross-module reference instead gets its own isolated
    /// placeholder here, discharged post-solve by `promote_untyped_boundaries`
    /// against the source binding's *generalized* scheme — see the "Boundary
    /// Scheme Promotion" design at
    /// `docs/adr/0008-untyped-binding-module-boundary-generalization.md`.
    pending_instantiations: Vec<PendingInstantiation>,
}

/// A cross-module reference to an untyped top-level binding, recorded during
/// constraint generation. `placeholder` is a fresh, isolated `Flex` var minted
/// at the reference site (instead of sharing the binding's program-wide var);
/// the post-solve `promote_untyped_boundaries` pass unifies it with a fresh
/// instantiation of the source binding's generalized scheme, once that scheme
/// exists (source module precedes `use_home` in topo order).
pub struct PendingInstantiation {
    /// The referenced binding's `(home, name)`.
    pub source: (Vec<Symbol>, Symbol),
    /// The fresh, isolated `Flex` var minted at the reference site.
    pub placeholder: VarId,
    /// The module that owns the reference (for blame attribution).
    pub use_home: Vec<Symbol>,
    /// The reference's source span (for blame attribution).
    pub span: Span,
}

/// A single use site of a typed top-level binding.
///
/// At each reference the binding's scheme is instantiated into fresh variables
/// (the [`Builder::instantiate`] / `CForeign` path). `vars` records, for each of
/// the scheme's type variables (keyed by the annotation variable's raw symbol
/// id), the fresh union-find variable it instantiated to — so once the solver
/// settles, the concrete type this use pinned each variable to can be read back
/// and checked against the binding's super-type obligations.
pub struct SchemeApp {
    /// The referenced binding's HOME module path (AUD-05 seal fix) — paired
    /// with `name` so the use-site soundness check
    /// ([`super::check_scheme_applications`]) looks up the bound set of the
    /// binding actually referenced, not a same-named binding from a different
    /// module (matches the `(home, name)` key shape `SolvedTypes::env` /
    /// `SolvedTypes::regions` already use for the identical reason).
    pub home: Vec<Symbol>,
    /// The referenced binding's name.
    pub name: Symbol,
    /// Scheme type-variable raw id → the fresh variable it instantiated to here.
    pub vars: BTreeMap<u32, VarId>,
    /// The reference's source span, for blame on an unsatisfied bound.
    pub span: Span,
}

/// A data constructor's quantified type scheme.
///
/// `arg_tys` are the declared payload field types (a nullary constructor has an
/// empty list); `result` is the enum type the constructor builds, applied to the
/// union's type variables (`Maybe a` for `Just`). Both sides share the union's
/// type variables as [`Ty::Var`]s, so instantiating them through one shared map
/// alpha-renames a generic constructor consistently per use site.
#[derive(Clone)]
struct CtorScheme {
    arg_tys: Vec<Ty>,
    result: Ty,
}

/// A deferred record field-access obligation `record.field`.
///
/// Closed records carry no row variable, so a field access cannot be discharged
/// by ordinary unification while the constraints are still being built (the
/// record's type may not be settled yet). Each access is recorded here and
/// resolved once after the main solve, when [`crate::resolve_field_accesses`]
/// can read the now-settled record type and link `result` to the field's type.
pub struct FieldAccess {
    /// The variable of the record sub-expression (`record` in `record.field`).
    pub record: VarId,
    /// The accessed field name.
    pub field: Symbol,
    /// The variable the access's result type was bound to (the access's region).
    pub result: VarId,
    /// The access expression's source span, for blame.
    pub span: Span,
    /// The home module path of the def this access lives in. After `link::link`
    /// merges modules, a bare byte-offset span cannot identify the source file;
    /// the home lets a post-solve error (IPE-T0012) attribute to the correct
    /// module instead of the byte-offset heuristic's best guess (which can pick
    /// a numerically-closer def in a *different* file — the span-collision
    /// class, here surfacing as an `info.message` error blamed on an unrelated
    /// `class` call in another module).
    pub home: Vec<Symbol>,
}

/// A deferred record-update obligation `{ base | field = value, ... }`.
///
/// Like [`FieldAccess`], a closed record carries no row variable, so the
/// updated fields cannot be checked against the base's type while the
/// constraints are still being built. Each update is recorded here and resolved
/// once after the main solve, when [`crate::resolve_record_updates`] reads the
/// settled base type and unifies each updated value against the corresponding
/// field's type (a field absent from the base is a [`crate::TypeError::NoSuchField`]).
pub struct RecordUpdate {
    /// The variable of the base record being copied (`base` in `{ base | … }`).
    pub record: VarId,
    /// The updated `(field name, value variable)` pairs.
    pub fields: Vec<(Symbol, VarId)>,
    /// The update expression's source span, for blame.
    pub span: Span,
    /// The home module path of the def this update lives in — see
    /// [`FieldAccess::home`].
    pub home: Vec<Symbol>,
}

/// A deferred post-solve check for routed `Web.app` configurations.
///
/// `Web.app`'s cfg row accepts both routed apps (Model has a `page : Page`
/// field) and non-routed apps (Model has no `page` field) through the same
/// open-record scheme.  The distinction cannot be expressed as a plain HM
/// constraint at build time (a conditional `{ page : var(2) | ρ }` projection
/// would break every non-routed app whose Model has no `page` field).
///
/// Instead, the constrain pass pushes one `RoutedWebCheck` per `Web.app`
/// call site and defers the gate to [`crate::resolve_routed_web_checks`],
/// which runs after the main solve when the Model type has settled:
///
/// * If Model's settled type has a `page` field → this is a routed app →
///   `notFound`'s type must match `Model.page`'s type (same `var(2)` share).
///   A mismatch produces IPE-T0001 here instead of a cargo E0308 / E0631
///   from the emitted `set_page` closure.
/// * If Model has no `page` field → non-routed → no validation; passes.
pub struct RoutedWebCheck {
    /// `var(0)` from the `K::WebApp` scheme instantiation — the Model type.
    pub model_var: VarId,
    /// `var(2)` from the `K::WebApp` scheme instantiation — the `notFound` type.
    pub not_found_var: VarId,
    /// The `Web.app { … }` call span; used to blame a type mismatch.
    pub span: Span,
}

/// A deferred per-route page-witness check for `Web.route`.
///
/// `Web.route : String -> builder -> WebRoute page` types its second
/// argument with a variable (`builder`, var(1)) DISTINCT from the result's
/// page variable (`page`, var(0)), because the argument is legitimately either
/// shape:
///
/// * a nullary page VALUE — `Web.route "/" HomePage` (builder : `Page`), or
/// * a params-consuming page CONSTRUCTOR —
///   `Web.route "/apps/:slug" AppDetailPage` (builder : `String -> Page`;
///   multi-`:param` routes curry further: `String -> String -> Page`, …).
///
/// A single shared variable (the pre-round-4 scheme) forced
/// `Page ≟ String -> Page` on every param route — a false IPE-T0001 on the
/// canonical corpus shape.  A plain HM constraint cannot express the
/// disjunction, so the constrain pass pushes one `RouteWitnessCheck` per
/// `Web.route` reference and defers it to
/// [`crate::resolve_route_witness_checks`], which runs after the main solve:
///
/// * Follow `builder_var`'s settled structure, peeling leading `_ -> rest`
///   arrows (each arrow is one `:param` payload slot; the emit tier separately
///   gates the payload types to `String`/`Int`/`Float`/`Bool`).
/// * Unify what remains — the built PAGE type — with `page_var`.
///
/// A nullary route therefore witnesses `page` directly, a param constructor
/// witnesses it with its result type, and a wrong-ADT constructor
/// (`Web.route "/" Increment` in a `Page`-routed app) still fails unification
/// with IPE-T0001 at this route's span.  Runs BEFORE
/// [`crate::resolve_routed_web_checks`] so route constructors pin the page
/// variable before the `notFound ≟ Model.page` gate reads it.
pub struct RouteWitnessCheck {
    /// `var(1)` from the `K::WebRoute` scheme instantiation — the route's
    /// page-builder argument type.
    pub builder_var: VarId,
    /// `var(0)` from the `K::WebRoute` scheme instantiation — the page type
    /// carried by the resulting `WebRoute page`.
    pub page_var: VarId,
    /// The `Web.route` reference span; used to blame a type mismatch.
    pub span: Span,
}

/// The output of constraint generation, consumed by the solver + read-back.
pub struct Generated {
    /// Resolved type per source region, keyed by `(home_module_path, Span)`.
    /// See [`Builder::regions`] for the rationale.
    pub regions: BTreeMap<(Vec<Symbol>, Span), VarId>,
    /// Contextually-EXPECTED type per source region — the type-directed
    /// completion sidecar. See [`Builder::expected`]. Read back into
    /// `SolvedTypes::expected` and never consulted by the solver, so it is
    /// purely additive over the existing inference result.
    pub expected: BTreeMap<(Vec<Symbol>, Span), VarId>,
    pub constraints: Vec<Constraint>,
    /// Values stay behind the builder's `Rc`; the read-back (`lib.rs`) unwraps
    /// them into the public `SolvedTypes::env` shape (refcount is 1 by then —
    /// every per-reference clone is transient inside constraint generation).
    pub top_level: BTreeMap<(Vec<Symbol>, Symbol), Rc<Ty>>,
    pub untyped: BTreeMap<(Vec<Symbol>, Symbol), VarId>,
    pub field_accesses: Vec<FieldAccess>,
    pub record_updates: Vec<RecordUpdate>,
    /// Deferred routed-Web.app checks, resolved after the main solve.
    pub routed_web_checks: Vec<RoutedWebCheck>,
    /// Deferred per-route page-witness checks, resolved after the main solve
    /// (before `routed_web_checks`).
    pub route_witness_checks: Vec<RouteWitnessCheck>,
    pub typed_rigids: Vec<PolyVarEntry>,
    pub scheme_apps: Vec<SchemeApp>,
    pub super_vars: Vec<(VarId, TyBounds, Span)>,
    /// Every cross-module untyped-binding reference recorded during
    /// constraint generation. See [`PendingInstantiation`].
    pub pending_instantiations: Vec<PendingInstantiation>,
    /// Every distinct module home reachable in the linked program, in
    /// first-encounter order over `module.defs` — which is itself
    /// dependency-first topo order, since `link::link` concatenates each
    /// source module's whole def list in the caller-supplied topo order (see
    /// `ipe_canon::link` and `ipe::project::topological_order`). Consumed by
    /// `promote_untyped_boundaries` to discharge/generalize each module's
    /// untyped defs only after every module it depends on has already been
    /// generalized.
    pub module_order: Vec<Vec<Symbol>>,
}

impl<'a> Builder<'a> {
    /// Build a constraint set for the whole module.
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] on an internal invariant violation (e.g. an
    /// arity mismatch between a binding's pattern count and its annotation, or
    /// an unbound local — both ruled out by canonicalisation).
    pub fn run(
        uf: &'a mut UnionFind<Content>,
        interner: &'a mut Interner,
        module: &canon::Module,
    ) -> DResult<Generated> {
        Self::run_seeded(uf, interner, module, &[], BTreeMap::new())
    }

    /// [`Self::run`] over ONE module of a multi-module program, seeded with
    /// its dependencies' typed interfaces: `dep_unions` registers the deps'
    /// constructor schemes (so a cross-module constructor reference or
    /// pattern instantiates exactly as it does over the linked merge), and
    /// `seed_top_level` pre-populates the `(home, name)` scheme table with
    /// the deps' exported binding schemes (so a cross-module `VarTopLevel`
    /// reference takes the ordinary instantiate-fresh-per-use-site path).
    /// With empty seeds this IS [`Self::run`] — one code path, no drift.
    ///
    /// # Errors
    /// Same conditions as [`Self::run`].
    #[allow(clippy::too_many_lines)] // declarative registration loops — every case listed explicitly for safety
    pub fn run_seeded(
        uf: &'a mut UnionFind<Content>,
        interner: &'a mut Interner,
        module: &canon::Module,
        dep_unions: &[&canon::Union],
        seed_top_level: BTreeMap<(Vec<Symbol>, Symbol), Rc<Ty>>,
    ) -> DResult<Generated> {
        let builtins = Builtins::new(interner)?;
        let mut builder = Self {
            uf,
            interner,
            builtins,
            regions: BTreeMap::new(),
            expected: BTreeMap::new(),
            current_home: Vec::new(),
            constraints: Vec::new(),
            top_level: seed_top_level, // (home, name) → Ty
            untyped: BTreeMap::new(),  // (home, name) → VarId
            field_accesses: Vec::new(),
            record_updates: Vec::new(),
            routed_web_checks: Vec::new(),
            route_witness_checks: Vec::new(),
            wildcard_any_return_bodies: BTreeMap::new(),
            wildcard_any_return_bindings: BTreeSet::new(),
            wildcard_any_use_results: Vec::new(),
            ctors: BTreeMap::new(),
            typed_rigids: Vec::new(),
            scheme_apps: Vec::new(),
            super_vars: Vec::new(),
            pending_instantiations: Vec::new(),
        };

        // Register the Prelude-built-in constructor schemes (`True` / `False` /
        // `Just` / `Nothing` / `Ok` / `Err`) first, so a reference or pattern
        // instantiates `Maybe a` / `Result e a` / `Bool` fresh per use site. A
        // user `type` cannot shadow these names (the canon §3.2 gate rejects it),
        // so the module-union loop below never collides with them.
        for (name, scheme) in builder.builtins.ctor_schemes() {
            builder.ctors.insert(name, Rc::new(scheme));
        }

        // Register every data constructor's scheme up front, so a `VarCtor`
        // reference or a constructor pattern can instantiate it fresh. A
        // constructor `C : field0 -> … -> T vars`; the result type applies the
        // union to its declared type variables (as `Ty::Var`s), and the field
        // types carry those same variables, so one shared instantiation map
        // alpha-renames a generic constructor per use site. Seeded dep unions
        // register after the module's own, mirroring how the linked merge
        // carries every module's unions in one list.
        for union in module.unions.iter().chain(dep_unions.iter().copied()) {
            // Use the union's own `home` (its original defining module path)
            // rather than `module.name`. After `link::link` merges N canonical
            // modules into one, every union retains its source-module path in
            // `home`; `module.name` would always be the entry module's name
            // (e.g. `["Main"]`), causing cross-module constructor result types
            // (`Main.Color`) to diverge from cross-module type annotations
            // (`Helper.Color`) and fail unification (IPE-T0001).
            let result = Ty::Con {
                module: union.home.clone(),
                name: union.name,
                args: union.vars.iter().map(|v| Ty::Var(v.as_raw())).collect(),
            };
            // Pre-compute once per union (Copy types, no borrow conflict with
            // builder.ctors below).
            let dict_sym = builder.builtins.dict;
            let string_sym = builder.builtins.string;
            for ctor in &union.ctors {
                let mut arg_tys = Vec::with_capacity(ctor.args.len());
                for ct in &ctor.args {
                    // Pin `any` wildcard fields to Dict String String so every
                    // instantiation site (pattern binder, ctor-as-value,
                    // Sub.subscribeTopic) sees the concrete carrier, never a
                    // free Ty::Var that the lowerer would reject (IPE-L0102).
                    arg_tys.push(pin_any_in_ty(
                        from_canon(ct),
                        &union.vars,
                        builder.interner,
                        dict_sym,
                        string_sym,
                    ));
                }
                builder.ctors.insert(
                    ctor.name,
                    Rc::new(CtorScheme {
                        arg_tys,
                        result: result.clone(),
                    }),
                );
            }
        }

        // First pass: register every binding so any binding can reference any
        // other (forward references resolve).
        //
        // * Typed bindings record their annotation type — the binding's *scheme*,
        //   instantiated fresh (flex) at each reference (`VarTopLevel`).
        // * Untyped bindings mint one shared monomorphic variable up front. Every
        //   reference resolves to that *same* variable, so a reference is checked
        //   against the binding's inferred type instead of being left
        //   unconstrained. The variable's settled type is read back into `env`.
        //   (Generalising an *un*annotated binding so it can be used at several
        //   concrete types in one module needs rank-based let-generalisation,
        //   which the solver does not yet model — so an untyped polymorphic
        //   binding is monomorphic at its use sites. Sound, not yet complete;
        //   write an annotation to get full polymorphism.)
        for def in &module.defs {
            // Key by (home_module_path, bare_name) so same-named defs from
            // different source modules never overwrite each other after
            // `link::link` merges them into a single flat def list.
            let home_key = def.home().to_vec();
            match def {
                canon::Def::Typed {
                    name, ty, patterns, ..
                } => {
                    let raw = from_canon(ty);
                    // ex15: a binding annotated `Handler` is really
                    // `Request -> Task Response` at call sites.  The internal
                    // constrain_def pass already expands Handler for the body so
                    // `req` gets type `Request`; here we must also expand for the
                    // top_level table so callers (e.g. Server.get) unify correctly.
                    let expanded = if let Ty::Con {
                        name: tname, args, ..
                    } = &raw
                    {
                        if *tname == builder.builtins.handler
                            && args.is_empty()
                            && !patterns.is_empty()
                        {
                            Ty::Fun(
                                Box::new(Ty::Con {
                                    module: Vec::new(),
                                    name: builder.builtins.server_request,
                                    args: Vec::new(),
                                }),
                                Box::new(Ty::Con {
                                    module: Vec::new(),
                                    name: builder.builtins.task,
                                    args: vec![Ty::Con {
                                        module: Vec::new(),
                                        name: builder.builtins.server_response,
                                        args: Vec::new(),
                                    }],
                                }),
                            )
                        } else {
                            raw
                        }
                    } else {
                        raw
                    };
                    let normalized = builder.normalize_annotation_ty(expanded, name.span)?;
                    // A bare wildcard `any` in the annotation's RETURN position
                    // severs the body from every use (see
                    // [`Builder::tie_wildcard_any_uses_to_bodies`]); record the
                    // binding so each reference is tied back to its body.
                    if builder.annotation_returns_wildcard_any(&normalized) {
                        builder
                            .wildcard_any_return_bindings
                            .insert((home_key.clone(), name.value));
                    }
                    builder
                        .top_level
                        .insert((home_key, name.value), Rc::new(normalized));
                }
                canon::Def::Untyped { name, .. } => {
                    let v = builder.flex()?;
                    builder.untyped.insert((home_key, name.value), v);
                }
            }
        }

        // Second pass: constrain each binding's body.
        for def in &module.defs {
            builder.constrain_def(def)?;
        }

        // With every binding constrained, `wildcard_any_return_bodies` is
        // complete: tie every wildcard-`any`-return reference to its body so the
        // body's real type flows to each use before the solver runs, regardless
        // of the source order in which a use and its binding appeared.
        builder.tie_wildcard_any_uses_to_bodies()?;

        // `module.defs` is already dependency-first topo order (link::link
        // concatenates each source module's whole def list in the
        // caller-supplied topo order) — a single first-encounter dedup pass
        // recovers the distinct module homes in that same order.
        let mut module_order: Vec<Vec<Symbol>> = Vec::new();
        for def in &module.defs {
            let home = def.home();
            if module_order.iter().all(|h| h.as_slice() != home) {
                module_order.push(home.to_vec());
            }
        }

        Ok(Generated {
            regions: builder.regions,
            expected: builder.expected,
            constraints: builder.constraints,
            top_level: builder.top_level,
            untyped: builder.untyped,
            field_accesses: builder.field_accesses,
            record_updates: builder.record_updates,
            routed_web_checks: builder.routed_web_checks,
            route_witness_checks: builder.route_witness_checks,
            typed_rigids: builder.typed_rigids,
            scheme_apps: builder.scheme_apps,
            super_vars: builder.super_vars,
            pending_instantiations: builder.pending_instantiations,
            module_order,
        })
    }

    // ── solver-var construction helpers ────────────────────────────────────

    fn flex(&mut self) -> DResult<VarId> {
        self.uf.fresh(Content::Flex)
    }

    fn rigid(&mut self) -> DResult<VarId> {
        self.uf.fresh(Content::Rigid)
    }

    fn structure(&mut self, f: FlatType) -> DResult<VarId> {
        self.uf.fresh(Content::Structure(f))
    }

    /// Mint a fresh [`FlatType::EmptyRecord`] variable — the closed-tail
    /// sentinel for closed records. Every `FlatType::Record(fields, ext)`
    /// whose `ext` points here is a closed record (field set exact).
    ///
    /// Each closed record gets its own `EmptyRecord` node rather than sharing
    /// one, so the occurs-check can distinguish different records' tails;
    /// this matches the Haskell reference's `UF.fresh EmptyRecord1` per
    /// record literal.
    fn empty_record_tail(&mut self) -> DResult<VarId> {
        self.structure(FlatType::EmptyRecord)
    }

    fn int_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.int;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn bool_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.bool;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn float_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.float;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn string_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.string;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn char_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.char;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    fn path_var(&mut self) -> DResult<VarId> {
        let name = self.builtins.path;
        self.structure(FlatType::Con {
            module: Vec::new(),
            name,
            args: Vec::new(),
        })
    }

    /// Mint a fresh super-typed flexible variable carrying `bounds` — a value
    /// the body has constrained to a Ipê super-type (numeric / ordered /
    /// equatable) but not yet to a concrete type. It pins to any matching type,
    /// or — when it meets an annotation skolem — lifts that skolem's obligations
    /// so the generic parameter is emitted with the matching trait bound.
    /// `span` is the operand span blamed if the variable later pins to a
    /// concrete type that does not actually support the operation.
    fn super_var(&mut self, bounds: TyBounds, span: Span) -> DResult<VarId> {
        let v = self.uf.fresh(Content::Super {
            rigid: false,
            bounds,
        })?;
        self.super_vars.push((v, bounds, span));
        Ok(v)
    }

    /// Constrain a binary operation by the type discipline of its operator. The
    /// returned [`VarId`] is the result type's variable. Mirrors the core
    /// subset of `Ipe.Type.Constrain.Expression.binopTypes`.
    fn constrain_binop(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        func: Symbol,
        lhs: &canon::Expr,
        rhs: &canon::Expr,
    ) -> DResult<VarId> {
        let class = classify_binop(self.interner.resolve(func).unwrap_or(""));
        let lv = self.constrain_expr(local, lhs)?;
        let rv = self.constrain_expr(local, rhs)?;
        match class {
            BinopClass::Num(bounds) => {
                // `+ - *` are Number-polymorphic: operands and result share one
                // numeric variable. A concrete operand (`x + 1`) pins it to that
                // type; an all-variable use (`x + x`) leaves it generic, carrying
                // the operator's obligation so generalisation emits the bound.
                let s = self.super_var(bounds, lhs.span)?;
                self.eq(lhs.span, lv, s);
                self.eq(rhs.span, rv, s);
                Ok(s)
            }
            BinopClass::IntDiv => {
                let li = self.int_var()?;
                self.eq(lhs.span, lv, li);
                let ri = self.int_var()?;
                self.eq(rhs.span, rv, ri);
                self.int_var()
            }
            BinopClass::FloatDiv => {
                let lf = self.float_var()?;
                self.eq(lhs.span, lv, lf);
                let rf = self.float_var()?;
                self.eq(rhs.span, rv, rf);
                self.float_var()
            }
            BinopClass::Order => {
                // `< > <= >=` are Comparable-polymorphic: operands share one
                // ordered type (carrying the ordering obligation), result Bool.
                let s = self.super_var(TyBounds::ord(), lhs.span)?;
                self.eq(lhs.span, lv, s);
                self.eq(rhs.span, rv, s);
                self.bool_var()
            }
            BinopClass::Equality => {
                // `== /=` are Equatable-polymorphic: operands share one equatable
                // type (carrying the equality obligation), result Bool. A
                // concrete operand pins it (`n == 1` → `Int`); an all-variable
                // use (`p == q`) leaves it generic, so generalisation emits a
                // `PartialEq` bound rather than an unbounded `T{n}` the backend
                // could not compare. A function operand fails the pin and a
                // function instantiation fails the post-solve gate (IPE-T0014).
                let s = self.super_var(TyBounds::eq(), lhs.span)?;
                self.eq(lhs.span, lv, s);
                self.eq(rhs.span, rv, s);
                self.bool_var()
            }
            BinopClass::Boolean => {
                let lb = self.bool_var()?;
                self.eq(lhs.span, lv, lb);
                let rb = self.bool_var()?;
                self.eq(rhs.span, rv, rb);
                self.bool_var()
            }
            BinopClass::Append => {
                // `++` is `Appendable a => a -> a -> a`: both operands and the
                // result share one super-typed variable carrying the appendable
                // obligation. The unifier pins it to `String` or `List _` at
                // the head; a non-appendable operand (Int, Bool, record, …)
                // fails at the pin and surfaces as IPE-T0014 before reaching
                // the backend.
                let s = self.super_var(TyBounds::appendable(), lhs.span)?;
                self.eq(lhs.span, lv, s);
                self.eq(rhs.span, rv, s);
                Ok(s)
            }
            BinopClass::Poly => {
                // `a -> a -> a`: operands and result share one type.
                self.eq(rhs.span, lv, rv);
                Ok(lv)
            }
        }
    }

    fn con_var(&mut self, module: Vec<Symbol>, name: Symbol, args: Vec<VarId>) -> DResult<VarId> {
        self.structure(FlatType::Con { module, name, args })
    }

    /// A `List elem` type variable over the element variable `elem`. The built-in
    /// `List` carries an empty module path, matching the other builtins.
    fn list_var(&mut self, elem: VarId) -> DResult<VarId> {
        let name = self.builtins.list;
        self.con_var(Vec::new(), name, vec![elem])
    }

    /// Constrain a list literal `[]` / `[a, b, c]`: every element shares one
    /// element variable, and the whole expression is the `List` over it. An empty
    /// list leaves the element variable flexible (inferred from context, else
    /// numeric-defaulted like any unpinned variable). Returns the result variable.
    fn constrain_list(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        elems: &[canon::Expr],
    ) -> DResult<VarId> {
        let elem = self.flex()?;
        for e in elems {
            let ev = self.constrain_expr(local, e)?;
            // Every list element expects the shared element type — an empty
            // slot in `[ ⟨|⟩ ]` where sibling elements pin `elem` completes to
            // that element type.
            self.record_expected(e.span, elem);
            self.eq(e.span, ev, elem);
        }
        self.list_var(elem)
    }

    /// Constrain a cons `head :: tail`: `head : elem`, `tail : List elem`, result
    /// `List elem`. Imposing the `a -> List a -> List a` discipline directly makes
    /// a non-list tail or a mismatched element a type error, not a backend crash.
    fn constrain_cons(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        head: &canon::Expr,
        tail: &canon::Expr,
    ) -> DResult<VarId> {
        let elem = self.constrain_expr(local, head)?;
        let list = self.list_var(elem)?;
        let tail_var = self.constrain_expr(local, tail)?;
        // The tail of `head :: tail` expects `List elem`.
        self.record_expected(tail.span, list);
        self.eq(tail.span, tail_var, list);
        Ok(list)
    }

    fn eq(&mut self, span: Span, lhs: VarId, rhs: VarId) {
        self.constraints.push(Constraint {
            span,
            lhs,
            rhs,
            home: self.current_home.clone(),
        });
    }

    /// Record the solver variable the enclosing context EXPECTS at `span` —
    /// the type-directed-completion sidecar (see [`Self::expected`]).
    ///
    /// Pure bookkeeping: it inserts into a map the solver never reads and mints
    /// no variable, so it cannot perturb inference. First writer wins — the
    /// tightest (innermost-recorded) expectation for a span is kept; an outer
    /// context that revisits the same span (rare, only under span-sharing
    /// desugarings) does not overwrite it.
    fn record_expected(&mut self, span: Span, var: VarId) {
        self.expected
            .entry((self.current_home.clone(), span))
            .or_insert(var);
    }

    // ── Ty ⇄ solver bridges ────────────────────────────────────────────────

    /// Instantiate a resolved [`Ty`] into fresh union-find structure, with every
    /// type variable replaced by a fresh **flexible** variable.
    ///
    /// This is the per-call-site instantiation (the Haskell `CForeign` path):
    /// each reference to a polymorphic top-level binding alpha-renames the
    /// binding's scheme into fresh flex variables, so the call unifies against the
    /// concrete argument types at *this* site without pinning the binding's other
    /// uses. Type variables alpha-rename consistently *within this call* via a
    /// fresh `vars` map (`a -> a` becomes `f -> f`, one shared flex), so calling
    /// `identity` at `Int` and at `Bool` in the same module yields two
    /// independent, separately-satisfiable instantiations.
    fn instantiate(&mut self, ty: &Ty) -> DResult<VarId> {
        let (var, _vars) = self.instantiate_tracked(ty)?;
        Ok(var)
    }

    /// [`Self::instantiate`], additionally returning the alpha-renaming map
    /// (scheme type-variable raw id → fresh variable). The map lets a use site be
    /// checked post-solve against the binding's super-type obligations: each
    /// obligated scheme variable's fresh variable reveals the concrete type this
    /// use pinned it to.
    fn instantiate_tracked(&mut self, ty: &Ty) -> DResult<(VarId, BTreeMap<u32, VarId>)> {
        let mut vars = BTreeMap::new();
        let var = self.instantiate_in(ty, &mut vars, /* rigid */ false)?;
        Ok((var, vars))
    }

    /// Instantiate a constructor scheme through one shared variable map, returning
    /// the fresh variables of its payload fields and of its result enum type.
    /// Sharing the map keeps a generic constructor's field and result variables
    /// linked at this use site (`Just : a -> Maybe a` instantiated at `a = Int`
    /// ties the payload to the result), exactly like [`Self::instantiate`] over the
    /// equivalent arrow — but decomposed, so a pattern can bind each field and a
    /// value reference can rebuild the arrow.
    fn instantiate_ctor(&mut self, scheme: &CtorScheme) -> DResult<(Vec<VarId>, VarId)> {
        let mut vars = BTreeMap::new();
        let mut arg_vars = Vec::with_capacity(scheme.arg_tys.len());
        for t in &scheme.arg_tys {
            arg_vars.push(self.instantiate_in(t, &mut vars, /* rigid */ false)?);
        }
        let result_var = self.instantiate_in(&scheme.result, &mut vars, /* rigid */ false)?;
        Ok((arg_vars, result_var))
    }

    /// Instantiate a resolved [`Ty`] with every type variable replaced by a fresh
    /// **rigid** (skolem) variable, sharing `vars` across the call so repeated
    /// occurrences of one annotation variable map to one rigid node.
    ///
    /// Used to seed a typed binding's parameters + return when checking its body:
    /// the whole signature is instantiated through *one* `vars` map so `a` is the
    /// same rigid everywhere it appears, and distinct annotation variables become
    /// distinct rigids that the body cannot conflate ([`Content::Rigid`]).
    fn instantiate_rigid(&mut self, ty: &Ty, vars: &mut BTreeMap<u32, VarId>) -> DResult<VarId> {
        self.instantiate_in(ty, vars, /* rigid */ true)
    }

    fn instantiate_in(
        &mut self,
        ty: &Ty,
        vars: &mut BTreeMap<u32, VarId>,
        rigid: bool,
    ) -> DResult<VarId> {
        match ty {
            Ty::Unit => self.structure(FlatType::Unit),
            Ty::Tuple(elems) => {
                let mut elem_vars = Vec::with_capacity(elems.len());
                for e in elems {
                    elem_vars.push(self.instantiate_in(e, vars, rigid)?);
                }
                self.structure(FlatType::Tuple(elem_vars))
            }
            Ty::Record(fields, tail) => {
                let mut field_vars = BTreeMap::new();
                for (name, field_ty) in fields {
                    let v = self.instantiate_in(field_ty, vars, rigid)?;
                    field_vars.insert(*name, v);
                }
                // Open records: instantiate the row tail variable via the same
                // `vars` map so the same source-level row var (`appExt`) maps
                // to a single UF node across all uses in the same binding.
                // Closed records: mint a fresh EmptyRecord sentinel.
                let ext = match tail {
                    RowTail::Closed => self.empty_record_tail()?,
                    RowTail::Open(raw_id) => {
                        if let Some(v) = vars.get(raw_id).copied() {
                            v
                        } else {
                            let v = if rigid { self.rigid()? } else { self.flex()? };
                            vars.insert(*raw_id, v);
                            v
                        }
                    }
                };
                self.structure(FlatType::Record(field_vars, ext))
            }
            Ty::Var(id) => {
                // `any` is Ipê's wildcard type-variable name. In annotations it
                // means "I don't care about this type" — each occurrence is an
                // INDEPENDENT fresh flex UV, NOT a shared rigid skolem. Sharing
                // would force all occurrences to the same type; rigid would
                // prevent the body from assigning a concrete type.  Mirrors the
                // Haskell compiler's `Instantiate.fromAnnotation` filtering
                // `"any"` out of the skolem set and `buildEnv` giving each
                // occurrence its own fresh UF var.
                // AUD-13: a solver-representative id (tagged by `zonk`) is
                // structurally never an annotation symbol — skip the
                // interner resolution entirely rather than risk a spurious
                // numeric collision with the interned "any" string.
                let is_any = !is_solver_var(*id)
                    && self
                        .interner
                        .resolve(ipe_intern::Symbol::from_raw(*id))
                        .is_some_and(|name| name == "any");
                if is_any {
                    // Fresh flex UV per occurrence — intentionally NOT inserted
                    // into `vars` so the next occurrence also gets its own UV.
                    return self.flex();
                }
                if let Some(v) = vars.get(id).copied() {
                    return Ok(v);
                }
                let v = if rigid { self.rigid()? } else { self.flex()? };
                vars.insert(*id, v);
                Ok(v)
            }
            Ty::Fun(a, b) => {
                let av = self.instantiate_in(a, vars, rigid)?;
                let bv = self.instantiate_in(b, vars, rigid)?;
                self.structure(FlatType::Fun(av, bv))
            }
            Ty::Con { module, name, args } => {
                let mut arg_vars = Vec::with_capacity(args.len());
                for a in args {
                    arg_vars.push(self.instantiate_in(a, vars, rigid)?);
                }
                self.structure(FlatType::Con {
                    module: module.clone(),
                    name: *name,
                    args: arg_vars,
                })
            }
        }
    }

    // ── the walk ────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)] // Handler expansion block (E-12) pushes it over 100
    fn constrain_def(&mut self, def: &canon::Def) -> DResult<()> {
        // Track which source module this def belongs to so every `regions.insert`
        // in the sub-expression walk uses `(home, span)` as the key, preventing
        // cross-module span collisions after `link::link` merges dep modules.
        self.current_home = def.home().to_vec();
        match def {
            canon::Def::Typed {
                name,
                patterns,
                body,
                ty,
                free_vars,
                ..
            } => {
                // Instantiate the WHOLE signature through one shared map so every
                // occurrence of an annotation variable (`a` in `a -> a`) becomes
                // the *same* rigid (skolem) node, and distinct variables become
                // distinct rigids. Checking the body against rigids is what makes
                // the annotation a genuine contract: `f : a -> a; f x = x + 1`
                // (body pins `a` to `Int`) and `f : a -> b; f x = x` (body
                // conflates `a` and `b`) are both mismatches rather than silently
                // accepted. Per-call-site uses instead instantiate the binding's
                // type as fresh *flex* variables (see [`Self::instantiate`]).
                // ── Handler alias expansion (T0004 fix) ───────────────
                // `Handler` is the stdlib alias `Request -> Task Error Response`
                // (Ipe.Http.Server).  A binding annotated as `Handler` with one
                // parameter (e.g. `handleHome : Handler; handleHome req = …`)
                // would fire T0004 because the annotation is a nullary `Con`, not
                // a `Lambda`.  Expand it to the full arrow type here, before the
                // parameter-loop runs, so the loop can peel the arrow normally.
                //
                // The expansion is purely canonical — it mirrors exactly what
                // `canonicalise_type` would produce for an explicit
                // `Request -> Task Error Response` annotation.  `handler_expansion`
                // is kept as an owned `canon::Type` so `cursor` (a reference) can
                // point into it when the annotation is `Handler`.
                let handler_expansion: Option<canon::Type> = {
                    if let canon::Type::Con {
                        name: tname, args, ..
                    } = ty
                    {
                        if *tname == self.builtins.handler
                            && args.is_empty()
                            && !patterns.is_empty()
                        {
                            let task_resp = canon::Type::Con {
                                home: Vec::new(),
                                name: self.builtins.task,
                                args: vec![
                                    canon::Type::Con {
                                        home: Vec::new(),
                                        name: self.builtins.error,
                                        args: Vec::new(),
                                    },
                                    canon::Type::Con {
                                        home: Vec::new(),
                                        name: self.builtins.server_response,
                                        args: Vec::new(),
                                    },
                                ],
                            };
                            Some(canon::Type::Lambda(
                                Box::new(canon::Type::Con {
                                    home: Vec::new(),
                                    name: self.builtins.server_request,
                                    args: Vec::new(),
                                }),
                                Box::new(task_resp),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                let mut rigid_vars = BTreeMap::new();
                let mut local = BTreeMap::new();
                let mut cursor: &canon::Type = handler_expansion.as_ref().unwrap_or(ty);
                for pat in patterns {
                    let (arg_ty, rest) = match cursor {
                        canon::Type::Lambda(a, b) => (a.as_ref(), b.as_ref()),
                        // The binding writes more parameter patterns than its
                        // annotation has arrows (`f a b = …` with `f : Int`).
                        // Parse-don't-validate: surface a user-facing
                        // IPE-T0004 with the binding span + the written
                        // signature, not a CompilerBug.
                        _ => return Err(self.too_many_parameters(name, ty)),
                    };
                    let arg = self.normalize_annotation_ty(from_canon(arg_ty), name.span)?;
                    let arg_var = self.instantiate_rigid(&arg, &mut rigid_vars)?;
                    self.constrain_pattern(&mut local, pat, arg_var)?;
                    // Record the param pattern's region so the lowerer can read the
                    // solved param type (record-param field-set completion, IPE-T0015
                    // path). Keyed by `(current_home, pat.span)` to prevent collisions
                    // across dep modules (see `Builder::regions` doc comment).
                    self.regions
                        .insert((self.current_home.clone(), pat.span), arg_var);
                    cursor = rest;
                }
                let ret_ty = self.normalize_annotation_ty(from_canon(cursor), name.span)?;
                let ret_var = self.instantiate_rigid(&ret_ty, &mut rigid_vars)?;
                let body_var = self.constrain_expr(&local, body)?;
                // A typed binding's body expects its annotation return type —
                // the strongest completion signal: `f : Color; f = ⟨|⟩` offers
                // `Color`'s constructors first.
                self.record_expected(body.span, ret_var);
                self.eq(body.span, body_var, ret_var);
                // A binding whose RETURN annotation is the bare wildcard `any`
                // severs its body's settled type from every use site (each `any`
                // occurrence instantiates its own fresh flex). Record the body
                // var so [`Self::tie_wildcard_any_uses_to_bodies`] can re-connect
                // it to every use, undoing the severance at its root (a
                // `view = <this binding>` with an `Html` body then reaches the
                // shape's `Element` requirement as an ordinary mismatch). The
                // guard mirrors the registration pass exactly
                // ([`Self::annotation_returns_wildcard_any`]): a point-free def
                // (`alias : Model -> any; alias = view`, zero written patterns)
                // leaves `ret_ty` as the whole `Model -> any` arrow, which the
                // tie peels along with the use — so both def forms are recorded.
                if self.annotation_returns_wildcard_any(&ret_ty) {
                    self.wildcard_any_return_bodies
                        .insert((self.current_home.clone(), name.value), body_var);
                }
                // Record the skolem each annotation variable instantiated to, so
                // its body-imposed super-type obligations can be read back for
                // generalisation. Keyed by the variable's symbol (the lowerer's
                // `free_vars` are these same symbols).
                let mut var_rigids = BTreeMap::new();
                for fv in free_vars {
                    if let Some(rigid) = rigid_vars.get(&fv.as_raw()) {
                        var_rigids.insert(*fv, *rigid);
                    }
                }
                self.typed_rigids
                    .push(((self.current_home.clone(), name.value), var_rigids));
                Ok(())
            }
            canon::Def::Untyped {
                name,
                patterns,
                body,
                ..
            } => {
                let mut local = BTreeMap::new();
                let mut param_vars = Vec::with_capacity(patterns.len());
                for pat in patterns {
                    let v = self.flex()?;
                    self.constrain_pattern(&mut local, pat, v)?;
                    self.regions
                        .insert((self.current_home.clone(), pat.span), v);
                    param_vars.push(v);
                }
                let body_var = self.constrain_expr(&local, body)?;
                // Reconstruct the binding's full type as the right-nested arrow
                // `p0 -> p1 -> … -> body`, so `env[f]` for `f a b = a` is
                // `a -> b -> a`, not just the body's type. A binding with no
                // parameters is just its body's type.
                let mut arrow = body_var;
                for pv in param_vars.into_iter().rev() {
                    arrow = self.structure(FlatType::Fun(pv, arrow))?;
                }
                // Tie the reconstructed type to the shared variable minted in the
                // registration pass, which every reference resolves to.
                // Use the same (home, name) key that the registration pass used.
                let shared_key = (def.home().to_vec(), name.value);
                let Some(shared) = self.untyped.get(&shared_key).copied() else {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: format!(
                            "untyped binding `{}` was not registered",
                            self.interner.resolve(name.value).unwrap_or("<unknown>")
                        ),
                    });
                };
                self.eq(name.span, arrow, shared);
                Ok(())
            }
        }
    }

    /// Build the IPE-T0004 diagnostic for a binding with more parameter
    /// patterns than its annotation has arrows. Resolving the name / rendering
    /// the signature can itself only fail on a forged symbol, in which case
    /// that internal bug is surfaced instead.
    fn too_many_parameters(
        &self,
        name: &ipe_diagnostics::Located<Symbol>,
        ty: &canon::Type,
    ) -> Diagnostic {
        let binding = match self.interner.resolve(name.value) {
            Some(s) => Box::from(s),
            None => {
                return Diagnostic::CompilerBug {
                    where_: "intern.resolve",
                    detail: format!("no backing string for symbol {}", name.value.as_raw()),
                };
            }
        };
        match canon_type_to_doc(ty, self.interner) {
            Ok(signature) => Diagnostic::Type {
                span: name.span,
                msg: TypeError::TooManyParameters {
                    binding,
                    signature: Box::new(signature),
                },
            },
            Err(bug) => bug,
        }
    }

    /// Whether `ty` is the bare wildcard `any` annotation type — a `Ty::Var`
    /// whose interned symbol resolves to `"any"`. Mirrors the `Ty::Var` "any"
    /// arm in [`Self::instantiate_in`]: `any` is Ipê's wildcard type-variable
    /// name, distinct from a genuine named parameter (`a`, `msg`).
    fn is_wildcard_any_ty(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Var(id) if self
            .interner
            .resolve(Symbol::from_raw(*id))
            .is_some_and(|name| name == "any"))
    }

    /// Whether an annotation type's final RETURN (after peeling every leading
    /// `_ -> _` arrow) is the bare wildcard `any`. Such a binding's body is
    /// severed from its uses by the wildcard and must be re-tied — see
    /// [`Self::tie_wildcard_any_uses_to_bodies`].
    fn annotation_returns_wildcard_any(&self, ty: &Ty) -> bool {
        let mut cur = ty;
        while let Ty::Fun(_, ret) = cur {
            cur = ret;
        }
        self.is_wildcard_any_ty(cur)
    }

    /// Reduce a 2-arg `Task Error a` annotation type to the internal unary
    /// `Task a`, validating that the error channel is the `Error` type, and
    /// recursively normalise nested occurrences in any composite type.
    ///
    /// Ipê mandates `Task Error a` as the canonical user-facing form, but the
    /// type-checker's internal model is unary `Task a` — the error channel is
    /// always `Error` and therefore implicit in the IR.  This bridge is applied
    /// to every result of [`from_canon`] so user annotations unify with the
    /// kernel-built unary forms.
    ///
    /// # Errors
    ///
    /// Returns `IPE-T0001` when the error channel is not `Error` (e.g.
    /// `Task String a` or `Task Int a`).  Returns `IPE-T0016`
    /// ([`TypeError::TaskArity`]) when a `Task` annotation has a number of type
    /// arguments other than 1 or 2 — reachable from source (a bare `Task`, or
    /// `Task Error Int Bool`), because canonicalisation validates arity only for
    /// type *aliases*, never for a non-alias constructor application like `Task`.
    #[allow(clippy::too_many_lines)]
    fn normalize_annotation_ty(&self, ty: Ty, span: Span) -> DResult<Ty> {
        match ty {
            Ty::Con { module, name, args } => {
                if name == self.builtins.task {
                    match args.len() {
                        // 1-arg: already the internal unary form; recurse inside.
                        1 => {
                            let inner =
                                args.into_iter()
                                    .next()
                                    .ok_or_else(|| Diagnostic::CompilerBug {
                                        where_: STAGE,
                                        detail: "Task 1-arg: iterator exhausted (internal)".into(),
                                    })?;
                            let inner = self.normalize_annotation_ty(inner, span)?;
                            Ok(Ty::Con {
                                module,
                                name,
                                args: vec![inner],
                            })
                        }
                        // 2-arg: `Task Error a` — validate error channel, reduce.
                        2 => {
                            let mut it = args.into_iter();
                            let e_ty = it.next().ok_or_else(|| Diagnostic::CompilerBug {
                                where_: STAGE,
                                detail: "Task 2-arg: first arg missing (internal)".into(),
                            })?;
                            let a_ty = it.next().ok_or_else(|| Diagnostic::CompilerBug {
                                where_: STAGE,
                                detail: "Task 2-arg: second arg missing (internal)".into(),
                            })?;
                            if !self.is_error_ty(&e_ty) {
                                // Render both sides for a clear IPE-T0001 diagnostic.
                                let mut namer = VarNamer::new();
                                let expected = ty_to_doc(
                                    &Ty::Con {
                                        module: Vec::new(),
                                        name: self.builtins.error,
                                        args: Vec::new(),
                                    },
                                    self.interner,
                                    &mut namer,
                                )?;
                                let found = ty_to_doc(&e_ty, self.interner, &mut namer)?;
                                return Err(Diagnostic::Type {
                                    span,
                                    msg: TypeError::TypeMismatch {
                                        expected: Box::new(expected),
                                        found: Box::new(found),
                                        definition: None,
                                        path: Box::new([]),
                                    },
                                });
                            }
                            let inner = self.normalize_annotation_ty(a_ty, span)?;
                            Ok(Ty::Con {
                                module,
                                name,
                                args: vec![inner],
                            })
                        }
                        // A `Task` applied to any other arity (bare `Task`, or
                        // `Task Error Int Bool`) is ill-formed. It reaches here
                        // from source because canonicalisation validates arity
                        // only for type *aliases* (`NameError::AliasArity`), never
                        // for a non-alias type-constructor application like `Task`.
                        // Fail closed with a clean IPE-T0016 diagnostic naming the
                        // found argument count instead of raising a `CompilerBug`.
                        n => Err(Diagnostic::Type {
                            span,
                            msg: TypeError::TaskArity {
                                carrier: "Task",
                                found: n,
                            },
                        }),
                    }
                } else if (name == self.builtins.cmd || name == self.builtins.sub)
                    && args.len() != 1
                {
                    // `Cmd` / `Sub` take exactly one message type. A mis-arity
                    // application (bare `Cmd`, `Cmd Int Bool`) would otherwise
                    // reach the lowerer's `ir_type_from_canon` catch-all and
                    // ICE (IPE-I0001) — the Cmd/Sub sibling of the Task gate.
                    // Fail closed here with the same clean IPE-T0016, symmetric
                    // with the `Task` arm above.
                    let carrier = if name == self.builtins.cmd {
                        "Cmd"
                    } else {
                        "Sub"
                    };
                    Err(Diagnostic::Type {
                        span,
                        msg: TypeError::TaskArity {
                            carrier,
                            found: args.len(),
                        },
                    })
                } else if args.is_empty() && self.interner.resolve(name) == Some("HttpRequest") {
                    // `HttpRequest` is a stdlib type alias for a structural record
                    // (`{ body, followRedirects, headers, maxRedirects, method,
                    // timeout, url }`).  The Rust port has no Ipê-source stdlib
                    // files, so the canonicaliser never registers `HttpRequest` as a
                    // type alias — it falls through to an opaque `Con`.  Expand it
                    // here so user annotations like `upstreamRequest : HttpRequest`
                    // unify with the structural record that kernels such as
                    // `HttpStreamOpen` / `HttpGet` / `HttpPost` expect.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let int = || mk(self.builtins.int);
                    let bool_ty = || mk(self.builtins.bool);
                    let http_method_ty = || mk(self.builtins.http_method);
                    let list = |t: Ty| Ty::Con {
                        module: Vec::new(),
                        name: self.builtins.list,
                        args: vec![t],
                    };
                    let mut req_fields = BTreeMap::new();
                    req_fields.insert(self.builtins.http_f_body, string());
                    req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty());
                    req_fields.insert(
                        self.builtins.http_f_headers,
                        list(Ty::Tuple(vec![string(), string()])),
                    );
                    req_fields.insert(self.builtins.http_f_max_redirects, int());
                    req_fields.insert(self.builtins.http_f_method, http_method_ty());
                    req_fields.insert(self.builtins.http_f_timeout, int());
                    req_fields.insert(self.builtins.http_f_url, string());
                    Ok(Ty::Record(req_fields, RowTail::Closed))
                } else if args.is_empty() && self.interner.resolve(name) == Some("HttpResponse") {
                    // `HttpResponse` is a stdlib type alias for `{ body : String,
                    // headers : Dict String String, status : Int }`.  Expand for the
                    // same reason as `HttpRequest` above.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let int = || mk(self.builtins.int);
                    let dict = |k: Ty, v: Ty| Ty::Con {
                        module: Vec::new(),
                        name: self.builtins.dict,
                        args: vec![k, v],
                    };
                    let mut resp_fields = BTreeMap::new();
                    resp_fields.insert(self.builtins.http_f_body, string());
                    resp_fields.insert(self.builtins.http_f_headers, dict(string(), string()));
                    resp_fields.insert(self.builtins.http_f_status, int());
                    Ok(Ty::Record(resp_fields, RowTail::Closed))
                } else if args.is_empty() && self.interner.resolve(name) == Some("Response") {
                    // `Ipe.Http.Server.Response` is a record alias
                    // `{ status : Int, body : String, headers : Dict String
                    // String, contentType : String }` (reference
                    // `Ipê/Http/Server.ipe:66`). Expand structurally — same
                    // mechanism as `HttpResponse` above — so a handler can build
                    // it as a record literal and read fields off it.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let int = || mk(self.builtins.int);
                    let dict = |k: Ty, v: Ty| Ty::Con {
                        module: Vec::new(),
                        name: self.builtins.dict,
                        args: vec![k, v],
                    };
                    let mut resp_fields = BTreeMap::new();
                    resp_fields.insert(self.builtins.http_f_body, string());
                    resp_fields.insert(self.builtins.server_f_content_type, string());
                    resp_fields.insert(self.builtins.http_f_headers, dict(string(), string()));
                    resp_fields.insert(self.builtins.http_f_status, int());
                    Ok(Ty::Record(resp_fields, RowTail::Closed))
                } else if args.is_empty() && self.interner.resolve(name) == Some("Migration") {
                    // `Ipe.Db.Migration` is a record alias
                    // `{ name : String, sql : String }`. Expand structurally so a
                    // program can build migrations as record literals in a
                    // `List Migration`.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let mut m_fields = BTreeMap::new();
                    m_fields.insert(self.builtins.migration_f_name, string());
                    m_fields.insert(self.builtins.migration_f_sql, string());
                    Ok(Ty::Record(m_fields, RowTail::Closed))
                } else {
                    // Non-Task constructor: recurse into type arguments.
                    let args = args
                        .into_iter()
                        .map(|a| self.normalize_annotation_ty(a, span))
                        .collect::<DResult<Vec<_>>>()?;
                    Ok(Ty::Con { module, name, args })
                }
            }
            Ty::Fun(a, b) => {
                let a = self.normalize_annotation_ty(*a, span)?;
                let b = self.normalize_annotation_ty(*b, span)?;
                Ok(Ty::Fun(Box::new(a), Box::new(b)))
            }
            Ty::Tuple(elems) => {
                let elems = elems
                    .into_iter()
                    .map(|e| self.normalize_annotation_ty(e, span))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Ty::Tuple(elems))
            }
            Ty::Record(fields, tail) => {
                let fields = fields
                    .into_iter()
                    .map(|(k, v)| self.normalize_annotation_ty(v, span).map(|v| (k, v)))
                    .collect::<DResult<_>>()?;
                Ok(Ty::Record(fields, tail))
            }
            // Leaf types: pass through unchanged.
            other @ (Ty::Var(_) | Ty::Unit) => Ok(other),
        }
    }

    /// Check whether `ty` is the built-in `Error` type — a nullary type
    /// constructor named `"Error"`.  The module path is intentionally ignored so
    /// both bare `Error` and fully-qualified `Ipe.Error.Error` are accepted.
    fn is_error_ty(&self, ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::Con { name, args, .. } if *name == self.builtins.error && args.is_empty()
        )
    }

    /// Constrain a reference to a top-level binding. A typed binding is
    /// instantiated fresh (flex) at this use site so it unifies against its own
    /// concrete arguments without pinning the binding's other call sites, and the
    /// alpha-renaming map is recorded for the post-solve super-type obligation
    /// check. An untyped binding resolves to its shared monomorphic variable; a
    /// name that is not a binding of this module stays fully flexible.
    ///
    /// `module` is the **home** module path carried by the `VarTopLevel` node —
    /// i.e. the path of the module that *declares* the binding, not the module
    /// that *uses* it.  Using this path as part of the lookup key (see
    /// [`Builder::top_level`]) ensures that a `Lib.helper` reference resolves to
    /// `Lib.helper`'s own annotation type even when a same-named `Main.helper`
    /// exists in the merged def list.
    fn constrain_var_top_level(
        &mut self,
        module: &[Symbol],
        name: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        let key = (module.to_vec(), name);
        if let Some(ty) = self.top_level.get(&key).cloned() {
            let (var, vars) = self.instantiate_tracked(&ty)?;
            self.scheme_apps.push(SchemeApp {
                home: module.to_vec(),
                name,
                vars,
                span,
            });
            // A reference to a wildcard-`any`-return binding: record this use's
            // instantiated arrow so [`Self::tie_wildcard_any_uses_to_bodies`]
            // (after all defs are constrained) ties its result to the binding's
            // body — undoing the wildcard severance so the body's real type
            // reaches this use site.
            if self.wildcard_any_return_bindings.contains(&key) {
                self.wildcard_any_use_results.push((var, key));
            }
            Ok(var)
        } else if let Some(v) = self.untyped.get(&key).copied() {
            if key.0 == self.current_home {
                // Same-module: still the one shared monomorphic var — an
                // untyped binding is monomorphic *within its home module*
                // (matches the reference's `CLocal` semantics exactly; see
                // `untyped_polymorphic_use_at_two_types_is_rejected`).
                Ok(v)
            } else {
                // Cross-module: isolate this reference behind its own fresh
                // placeholder instead of sharing the binding's program-wide
                // var. `promote_untyped_boundaries` (in `lib.rs`, post-solve)
                // discharges it against the source binding's generalized
                // scheme, once that scheme exists.
                let placeholder = self.flex()?;
                self.pending_instantiations.push(PendingInstantiation {
                    source: key,
                    placeholder,
                    use_home: self.current_home.clone(),
                    span,
                });
                Ok(placeholder)
            }
        } else {
            Err(Diagnostic::CompilerBug {
                where_: "ipe_types::constrain_var_top_level",
                detail: format!(
                    "unknown top-level binding (symbol {}); \
                     post-link every name must be in top_level or untyped",
                    name.as_raw()
                ),
            })
        }
    }

    /// The Ipê `comparable`-key obligation a kernel's element/key variable
    /// carries, keyed off the resolved [`StdlibKernel`] id via its
    /// `decl().qualifier` (parse-once — never a re-inspected module string).
    /// `Set`'s element is keyed by `BTreeSet` (`Ord`) and `Dict`'s key by a
    /// determinism-sorted `HashMap` (`Hash + Eq + Ord`); the obligation is
    /// attached to raw scheme-variable 0, the element/key in every `Set` /
    /// `Dict` kernel scheme.
    fn key_obligation_for(k: StdlibKernel) -> Option<TyBounds> {
        match k.decl().qualifier {
            "Set" => Some(TyBounds::set_elem()),
            "Dict" => Some(TyBounds::dict_key()),
            // `Ipe.Cache`'s key variable is raw scheme-var 0 in `get` /
            // `put` / `remove` (`Int -> k -> …`), and the runtime scans keys by
            // `PartialEq` (`cache_get`/`cache_put`/`cache_remove` bound
            // `K: PartialEq`). Attaching the EQ obligation lifts `PartialEq`
            // onto the emitted `Ipe.Cache` wrapper's key type parameter. The
            // key-less kernels (`newRaw`/`clear`/`size`/`stats`) have no
            // scheme-var 0, so the `vars.get(&0)` tie is a no-op for them.
            "Cache" => Some(TyBounds::eq()),
            _ => None,
        }
    }

    /// The raw scheme-var id of the CALLBACK-RESULT slot of a `Maybe`/`Result`
    /// higher-order kernel — the variable that must not itself instantiate to
    /// a function ([`TyBounds::hof_kernel_result`]).
    ///
    /// Slot ids follow each kernel's scheme in [`Self::stdlib_scheme`] and are
    /// asserted against those schemes by
    /// `hof_result_slots_match_scheme_shapes` (this module's tests): `map`'s
    /// `(a -> b)` result `b` is `var(1)`; `mapError`'s `(e -> f)` result `f`
    /// is `var(1)`; `mapN`'s `(a -> … -> v)` final result `v` is `var(N)`;
    /// `andMap`'s payload `Con (a -> b)` result `b` is `var(1)`.
    ///
    /// Deliberately EXCLUDED, with reasons:
    /// * `MaybeAndThen` / `ResultAndThen` / `ResultTraverse` — their callback
    ///   results are `Con`-headed in the scheme itself (`a -> Maybe b`, `a ->
    ///   Result e b`), so a curried callback is already a plain type mismatch
    ///   (`Fun` vs `Con`); there is no bare var for an arrow to escape into.
    /// * `MaybeWithDefault` / `ResultWithDefault` / `MaybeCombine` /
    ///   `ResultCombine` — no callback is applied by the kernel; a
    ///   function-valued payload flows through by value in its (consistently
    ///   flattened) representation, which is sound.
    /// * `Task` / `Cmd` / `Sub` / `Decoder` kernels — out of scope:
    ///   their heads are exempted from the ctor-payload region gate
    ///   (`is_opaque_boxed_wrapper`), so any curried-callback
    ///   hazard there is tracked separately (the
    ///   `Decoder` family in particular must NOT be gated — its runtime has
    ///   genuine `curry1..curry10` currying support the applicative decoder
    ///   pipeline depends on).
    const fn hof_result_slot_for(k: StdlibKernel) -> Option<u32> {
        use StdlibKernel as K;
        match k {
            K::MaybeMap | K::ResultMap | K::ResultMapError | K::MaybeAndMap | K::ResultAndMap => {
                Some(1)
            }
            K::MaybeMap2 | K::ResultMap2 => Some(2),
            K::MaybeMap3 | K::ResultMap3 => Some(3),
            K::MaybeMap4 | K::ResultMap4 => Some(4),
            K::MaybeMap5 | K::ResultMap5 => Some(5),
            _ => None,
        }
    }

    /// The type of a kernel reference (`Math.min`, `Set.insert`, …).
    ///
    /// Most kernels take the declarative scheme from [`Self::stdlib_scheme`] via
    /// `instantiate`. Two families instead mint super-typed obligations so a
    /// generic use lifts the matching Rust trait bound onto its annotation
    /// skolem and a non-comparable argument fails closed at type-check:
    ///
    /// * `Math.min` / `Math.max` — `Comparable a => a -> a -> a`: the shared
    ///   variable carries the ORDERING obligation, exactly as the `< > <= >=`
    ///   operators and the user-fn `maxOf` do, so a generic use emits Rust
    ///   `T: PartialOrd` and a function / record argument is rejected rather than
    ///   emitting an unbounded `math_min<T>(…)` that `cargo` rejects.
    /// * `Set` / `Dict` kernels — the element / key (raw scheme-variable 0 in
    ///   every Set / Dict kernel) carries the Ipê `comparable`-key obligation
    ///   ([`Self::key_obligation_for`]). The base scheme (now in
    ///   [`Self::stdlib_scheme`]) is instantiated, then variable 0 is tied to a
    ///   fresh super-typed variable carrying that obligation, so a
    ///   non-comparable element / key (record, ADT, function) fails closed
    ///   instead of emitting an unbounded `set_insert::<T>` / `dict_insert::<T>`
    ///   call `cargo` rejects, and a generic `a -> Set a` lifts `Ord` (Set) /
    ///   `Hash + Eq + Ord` (Dict) onto its annotation skolem (see `bounds_for`).
    ///   This is also more conservative than Ipê's runtime, which keys a Set /
    ///   Dict on a stringified value.
    #[allow(clippy::too_many_lines)]
    fn constrain_var_kernel(
        &mut self,
        id: Option<StdlibKernel>,
        module: Symbol,
        name: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        // ── Obligation pre-checks (keyed off the resolved `id`,
        //    not a re-inspected module string). They live OUTSIDE the scheme
        //    tables and must fire BEFORE the registry/legacy delegation, so the
        //    bounded super-var reaches the caller instead of the bare base
        //    scheme now sitting in `stdlib_scheme`. ──
        if let Some(k) = id {
            // `Math.min` / `Math.max`: `Comparable a => a -> a -> a`. The bounded
            // super-var (reused across BOTH arrow argument positions AND the
            // result) is what rejects `Math.min f g` / `Math.min recA recB`
            // (`golden_m4c_math_gate`). This is a DIRECT-build bounded
            // scheme, NOT `stdlib_scheme` + a tie, because min/max's base scheme
            // has three independent `var(0)`s and the gate needs all three tied
            // to one bounded var.
            if matches!(k, StdlibKernel::MathMin | StdlibKernel::MathMax) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let inner = self.structure(FlatType::Fun(s, s))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // `Basics.clamp lo hi x : comparable -> comparable -> comparable ->
            // comparable`. Same ORDERING obligation as min/max, but arity 3:
            // ONE bounded super-var reused across all three argument positions
            // AND the result, so `clamp recA recB recC` (records / functions /
            // ADTs) fails closed instead of emitting an unbounded
            // `basics_clamp::<T>` that `cargo` rejects. DIRECT-build (not
            // `stdlib_scheme` + tie) because the base scheme has three
            // independent `var(0)`s that must collapse to one bounded var.
            if matches!(k, StdlibKernel::BasicsClamp) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let inner1 = self.structure(FlatType::Fun(s, s))?;
                let inner2 = self.structure(FlatType::Fun(s, inner1))?;
                return self.structure(FlatType::Fun(s, inner2));
            }
            // ── Basics numerics ────────────────────────────────────────
            // `negate / abs : number a => a -> a`. SUB obligation (Number
            // super-type — same as the unary-minus operator). A function / record
            // argument fails closed (T0001) before reaching a runtime that would
            // panic. Base scheme for the totality gate is in `stdlib_scheme`.
            if matches!(k, StdlibKernel::BasicsNegate | StdlibKernel::BasicsAbs) {
                let s = self.super_var(TyBounds::sub(), span)?;
                return self.structure(FlatType::Fun(s, s));
            }
            // `min / max : comparable a => a -> a -> a` — same Comparable (Ord)
            // obligation as `Math.min` / `Math.max`. DIRECT-build (not
            // `stdlib_scheme` + tie) so all three positions collapse to ONE
            // bounded super-var, rejecting function / record arguments closed.
            if matches!(k, StdlibKernel::BasicsMin | StdlibKernel::BasicsMax) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let inner = self.structure(FlatType::Fun(s, s))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // `compare : comparable a => a -> a -> Order`. Direct-build
            // (not stdlib_scheme + tie): both argument positions share one
            // Ord-bounded super-var; the return is the monomorphic Order type.
            if matches!(k, StdlibKernel::BasicsCompare) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let order_var = self.structure(FlatType::Con {
                    module: Vec::new(),
                    name: self.builtins.order,
                    args: Vec::new(),
                })?;
                let inner = self.structure(FlatType::Fun(s, order_var))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // ── end Basics numerics ────────────────────────────────────
            // `List.sum : number a => List a -> a` / `List.product`. The list
            // element and the result share ONE number-bounded super-var (ADD for
            // sum, MUL for product — the same obligation `+` / `*` mint), so a
            // non-numeric element fails closed instead of emitting an unbounded
            // `list_sum::<T>`. Direct-build (not `stdlib_scheme` + tie) so both
            // the element and the result collapse to one bounded var.
            if matches!(k, StdlibKernel::ListSum | StdlibKernel::ListProduct) {
                let bound = if matches!(k, StdlibKernel::ListSum) {
                    TyBounds::add()
                } else {
                    TyBounds::mul()
                };
                let s = self.super_var(bound, span)?;
                let list_s = self.list_var(s)?;
                return self.structure(FlatType::Fun(list_s, s));
            }
            // `List.maximum / minimum : comparable a => List a -> Maybe a`. The
            // element carries the ORDERING obligation (same as `Math.min/max`);
            // the result is `Maybe a` over that bounded var. Direct-build so the
            // element and the Maybe payload share the one bounded super-var.
            if matches!(k, StdlibKernel::ListMaximum | StdlibKernel::ListMinimum) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let list_s = self.list_var(s)?;
                let maybe_s = self.structure(FlatType::Con {
                    module: Vec::new(),
                    name: self.builtins.maybe,
                    args: vec![s],
                })?;
                return self.structure(FlatType::Fun(list_s, maybe_s));
            }
            // `List.sort : comparable a => List a -> List a`. The element carries
            // the ORDERING obligation; input and output share the one bounded
            // super-var. Direct-build (not `stdlib_scheme` + tie).
            if matches!(k, StdlibKernel::ListSort) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let list_s = self.list_var(s)?;
                let list_s2 = self.list_var(s)?;
                return self.structure(FlatType::Fun(list_s, list_s2));
            }
            // `Basics.toString : a -> String`. The argument carries the
            // STRINGIFY obligation (a bounded super-var → Rust `IpeStringify`):
            // a scalar / record / ADT satisfies it, a bare function (or a value
            // nesting one) fails CLOSED at type-check rather than emitting an
            // unbounded `basics_to_string::<T>` that `cargo` rejects. Direct-build
            // (not stdlib_scheme + tie): only the argument position is bounded.
            // This is the shared lever for the whole Stringify-bounded family
            // (Log.*With / Debug.toString) — wire those the same way.
            if matches!(
                k,
                StdlibKernel::BasicsToString | StdlibKernel::ErrorToString
            ) {
                let s = self.super_var(TyBounds::show(), span)?;
                let string_ty = self.string_var()?;
                return self.structure(FlatType::Fun(s, string_ty));
            }
            // Dict / Set element-key `comparable` obligation. The base
            // scheme is relocated into `stdlib_scheme`; we instantiate
            // it and tie key-position raw var 0 to a bounded super-var. Only
            // key-position `var(0)` carries the bound, so this is `stdlib_scheme`
            // + a tie (unlike min/max's direct-build shape above).
            if let Some(bound) = Self::key_obligation_for(k) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&key_var) = vars.get(&0) {
                    let s = self.super_var(bound, span)?;
                    self.eq(span, key_var, s);
                }
                // `Set.map : (a -> b) -> Set a -> Set b` — the RESULT element
                // `b` (raw scheme-var 1) also backs a `BTreeSet<b>`, so it
                // carries the same `set_elem` (Ord) obligation as the source
                // element. Without this a generic `Set.map` would emit an
                // unbounded `set_map::<A, B>` that `cargo` rejects (B: Ord unmet).
                if matches!(k, StdlibKernel::SetMap)
                    && let Some(&res_var) = vars.get(&1)
                {
                    let s = self.super_var(bound, span)?;
                    self.eq(span, res_var, s);
                }
                return Ok(var);
            }
            // `Db.exec` / `Db.query` / `Db.queryDecode`: the params-LIST
            // ELEMENT (raw scheme-var 0 for `exec`/`query`; var 1 for
            // `queryDecode`, whose var 0 is the decoder's result type — see
            // the scheme comments above) carries the SQL-bind-parameter
            // obligation. Same `stdlib_scheme` + tie shape as the Set/Dict
            // key obligation directly above: only the params-element
            // position is bounded, so a generic wrapper around `Db.exec` /
            // `Db.query` (`Database.exec label queryStr args` in
            // `examples/17-ipemon`) lifts `Into<SqlParam>` onto its own
            // emitted Rust generic (closing the E0277 half), and an
            // empty-list call site whose element type is otherwise
            // completely unconstrained defaults to `SqlValue` at solve time
            // instead of the wildcard-`any` fallback (closing the E0283
            // half — see the `sql_param` arm of the numeric-defaulting loop
            // in `crate::lib`), rather than emitting a bare `Vec::new()`
            // `cargo` cannot infer.
            if matches!(
                k,
                StdlibKernel::DbExec
                    | StdlibKernel::DbQuery
                    | StdlibKernel::DbQueryDecode
                    | StdlibKernel::DbConnQueryDecode
            ) {
                // The params-list element var is index 1 for both `queryDecode`
                // shapes (they carry a decoder var 0 ahead of it), index 0 for the
                // bare `exec`/`query`.
                let raw_idx = u32::from(matches!(
                    k,
                    StdlibKernel::DbQueryDecode | StdlibKernel::DbConnQueryDecode
                ));
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&params_var) = vars.get(&raw_idx) {
                    let s = self.super_var(TyBounds::sql_param(), span)?;
                    self.eq(span, params_var, s);
                }
                return Ok(var);
            }
            // Higher-order-kernel callback-result obligation
            // (primary/Tier-2 mechanism — see
            // `docs/adr/0016-andmap-arity-gate-type-obligation.md`).
            // Every `Maybe`/`Result` higher-order kernel FULLY APPLIES its
            // callback at runtime (`FnOnce(..) -> R` with an exact arity),
            // while the IR flattens a curried Ipê function into one
            // multi-parameter `Fun` — so a callback with residual arity (its
            // final result var instantiates to another arrow) has no sound
            // lowering and would reach `cargo build` as E0277/E0308. Tie the
            // callback's final-result raw scheme-var (see
            // [`Self::hof_result_slot_for`]) to a fresh super-typed variable
            // carrying the `hof_kernel_result` obligation — same
            // `stdlib_scheme` + tie shape as the Dict/Set key obligation
            // above, so this is a genuine TYPE-LEVEL check that survives
            // arbitrary Ipê-level aliasing (direct call, piped, `let`-bound,
            // bare-value re-export, higher-order argument, record-field
            // extraction, import alias) by construction — the obligation is
            // attached to the union-find variable `constrain_var_kernel`
            // mints for THIS kernel reference, not to any particular AST
            // shape a later use might take.
            if let Some(slot) = Self::hof_result_slot_for(k) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&callback_result_var) = vars.get(&slot) {
                    let s = self.super_var(TyBounds::hof_kernel_result(), span)?;
                    self.eq(span, callback_result_var, s);
                }
                return Ok(var);
            }
            // `Log.*With : String -> List a -> Task Error ()` — the attr-list
            // ELEMENT `a` carries the STRINGIFY obligation. Same
            // `stdlib_scheme` + tie shape as Dict/Set: instantiate the base
            // scheme and tie its list-element `var(0)` to a Show super-var, so a
            // non-showable element (a function) fails closed at type-check.
            if matches!(
                k,
                StdlibKernel::LogInfoWith
                    | StdlibKernel::LogDebugWith
                    | StdlibKernel::LogWarnWith
                    | StdlibKernel::LogErrorWith
            ) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&elem_var) = vars.get(&0) {
                    let s = self.super_var(TyBounds::show(), span)?;
                    self.eq(span, elem_var, s);
                }
                return Ok(var);
            }
            // `Debug.log : String -> a -> a` — the value `a` (shared by the
            // argument and result, raw scheme-var 0) carries the STRINGIFY
            // obligation (the runtime stringifies it through the same
            // `IpeStringify` path as `Basics.toString`). Same `stdlib_scheme` +
            // tie shape as `Log.*With`: tying the ONE super-var to both
            // positions keeps `Debug.log Int 5` (concrete, satisfies `show`)
            // accepted while a bare-function value fails closed — no spurious
            // IPE-L0108 for a well-typed showable value.
            if matches!(k, StdlibKernel::DebugLog) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&value_var) = vars.get(&0) {
                    let s = self.super_var(TyBounds::show(), span)?;
                    self.eq(span, value_var, s);
                }
                return Ok(var);
            }
            // `Web.app` — post-solve routed-Web check.
            //
            // The open-record cfg scheme for K::WebApp is shared by both routed
            // apps (Model has a `page : Page` field) and non-routed apps (Model
            // has no `page` field).  We cannot express the conditional
            // `Model.page ≡ notFound` constraint at build time because a blanket
            // `var(0) ≡ { page : var(2) | ρ }` would break every non-routed
            // app whose Model has no `page` field.
            //
            // Instead: instantiate the scheme with `instantiate_tracked`, record
            // the Model var (var index 0) and notFound var (var index 2), then
            // push a `RoutedWebCheck` so `resolve_routed_web_checks` can run
            // the gate after the HM solver settles.
            if matches!(k, StdlibKernel::WebApp) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let (Some(&model_var), Some(&not_found_var)) = (vars.get(&0), vars.get(&2)) {
                    self.routed_web_checks.push(RoutedWebCheck {
                        model_var,
                        not_found_var,
                        span,
                    });
                }
                return Ok(var);
            }
            // `Web.route` — per-route page witness.
            //
            // The scheme types the page-builder argument with var(1) DISTINCT
            // from the result's page var(0): the argument is EITHER a nullary
            // page value (`Web.route "/" HomePage`) OR a params-consuming
            // constructor (`Web.route "/apps/:slug" AppDetailPage` — type
            // `String -> Page`).  That disjunction is not expressible as a
            // plain HM constraint, so — like `RoutedWebCheck` above — the
            // relation is deferred: record both instantiated vars and push a
            // `RouteWitnessCheck`; `resolve_route_witness_checks` peels the
            // builder's settled leading arrows and unifies the resulting page
            // type with var(0) after the main solve.
            if matches!(k, StdlibKernel::WebRoute) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let (Some(&page_var), Some(&builder_var)) = (vars.get(&0), vars.get(&1)) {
                    self.route_witness_checks.push(RouteWitnessCheck {
                        builder_var,
                        page_var,
                        span,
                    });
                }
                return Ok(var);
            }
        }
        // ── Parse-once registry lookup ──
        //
        // `stdlib_scheme` is TOTAL over the reachable kernel set and
        // WILDCARD-FREE, so every reachable kernel resolves via the
        // `StdlibKernel` id. There is no legacy string-keyed `kernel_ty`
        // table carrying a `Ty::Var(u32::MAX)` exit-0 sentinel for un-typed
        // kernels. A `None` id (FFI `Rust.*`) or an excluded bucket
        // (`WebAppRouted` — unlowered) misses the registry and is
        // fail-closed with IPE-L0108 (loud) via `kernel_scheme_or_unsupported`,
        // never silently typed as a free variable that `cargo` later rejects.
        let _ = (module, name); // retained for diagnostics
        // Route through `resolve_scheme`, not `stdlib_scheme` directly, so a
        // kernel carrying a structural `TyShape` resolves via the interpreter and
        // one without a shape resolves through the table — a single adapter, so
        // the two paths can never resolve to different types.
        let registry = id.and_then(|k| self.resolve_scheme(SchemeKey(k)));
        let ty = Self::kernel_scheme_or_unsupported(registry, None, span)?;
        self.instantiate(&ty)
    }

    /// Combine the parse-once registry scheme (`id` path) with the legacy
    /// string-table scheme, failing closed with IPE-L0108 (`Feature::Kernels`,
    /// the same shape lower raises at `lower_callee`) when NEITHER supplies a
    /// type. Extracted as a pure fn so the fail-closed arm is unit-testable
    /// independently of the (currently total) legacy table — see
    /// `both_miss_is_fail_closed`.
    fn kernel_scheme_or_unsupported(
        registry: Option<Ty>,
        legacy: Option<Ty>,
        span: Span,
    ) -> DResult<Ty> {
        registry.or(legacy).ok_or(Diagnostic::Lower {
            span,
            msg: LowerError::Unsupported(Feature::Kernels),
        })
    }

    #[allow(clippy::too_many_lines)] // one arm per canonical expression form
    fn constrain_expr(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        e: &canon::Expr,
    ) -> DResult<VarId> {
        let span = e.span;
        let var = match &e.value {
            // An integer literal is `Number`-polymorphic (Elm/Ipê `number`): it
            // may resolve to `Int` OR `Float` depending on context, and defaults
            // to `Int` when the program never pins it (the post-solve defaulting
            // loop closes an unpinned `Super { Number }` to `Int`).  This lets
            // `pct 100` — where `pct : Float -> Length` — accept the literal `100`
            // as a `Float`, matching the reference compiler.  A *float* literal
            // (`1.6`) is concretely `Float`, never `Int` (Elm keeps `1.6 : Float`
            // distinct from the polymorphic `number`).
            canon::Expr_::Int(_) => self.super_var(TyBounds::add(), span)?,
            canon::Expr_::Float(_) => self.float_var()?,
            canon::Expr_::Str(_) => self.string_var()?,
            canon::Expr_::PathLit(_) => self.path_var()?,
            canon::Expr_::Char(_) => self.char_var()?,
            canon::Expr_::Unit => self.structure(FlatType::Unit)?,
            canon::Expr_::VarLocal(s) => match local.get(s) {
                Some(v) => *v,
                None => {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: format!(
                            "unbound local `{}`",
                            self.interner.resolve(*s).unwrap_or("<unknown symbol>")
                        ),
                    });
                }
            },
            canon::Expr_::VarTopLevel { module, name } => {
                self.constrain_var_top_level(module, *name, span)?
            }
            canon::Expr_::VarKernel { id, module, name } => {
                // the pre-resolved `id` selects the parse-once
                // registry scheme (`stdlib_scheme`) for migrated families,
                // falling back to the legacy symbol-keyed table otherwise.
                self.constrain_var_kernel(*id, *module, *name, span)?
            }
            canon::Expr_::ForeignCall { args, .. } => {
                // A foreign wrapper call is the annotation-trusted boundary:
                // the enclosing FfiInterface binding is REQUIRED to carry a
                // full annotation (canon fails closed otherwise), and that
                // annotation pins every parameter and the result. Arguments
                // are constrained so their vars exist for the lowerer's
                // region map; the call's own type is a fresh flexible var the
                // annotation immediately determines.
                for a in args {
                    self.constrain_expr(local, a)?;
                }
                self.flex()?
            }
            canon::Expr_::VarCtor {
                home,
                type_name,
                name,
                ..
            } => self.constrain_var_ctor(home, *type_name, *name)?,
            canon::Expr_::Call(callee, args) => {
                let callee_var = self.constrain_expr(local, callee)?;
                // Each argument gets a FRESH param var rather than flowing its
                // own var straight into the callee's arrow shape. Two payoffs:
                //  1. the callee-vs-shape constraint below is solved FIRST, so
                //     each param var adopts the callee's DECLARED param type;
                //  2. the per-arg constraint then unifies found=actual-arg
                //     against expected=declared-param AT THE ARG'S SPAN —
                //     `Task.fail "str"` reads "expected Error, found String",
                //     never the inversion (and blames the argument, not the
                //     callee name).
                // A non-function callee still reports found=callee's type,
                // expected=`a -> b` via the callee-vs-shape constraint.
                let mut arg_pairs = Vec::with_capacity(args.len());
                for a in args {
                    let arg_var = self.constrain_expr(local, a)?;
                    let param_var = self.flex()?;
                    // The callee's declared slot is exactly the type this
                    // argument position expects: after the callee-vs-shape
                    // constraint solves, `param_var` adopts the declared param
                    // type, so completion at this span offers only candidates
                    // whose type unifies with the declared parameter.
                    self.record_expected(a.span, param_var);
                    arg_pairs.push((a.span, arg_var, param_var));
                }
                let ret = self.flex()?;
                // Fold a right-associative arrow over the fresh param vars:
                // p0 -> p1 -> … -> ret.
                let mut fun_shape = ret;
                for (_, _, param_var) in arg_pairs.iter().rev() {
                    fun_shape = self.structure(FlatType::Fun(*param_var, fun_shape))?;
                }
                // Order matters: callee-vs-shape first (see above).
                self.eq(callee.span, callee_var, fun_shape);
                for (arg_span, arg_var, param_var) in arg_pairs {
                    self.eq(arg_span, arg_var, param_var);
                }
                ret
            }
            canon::Expr_::Case(scrut, branches) => self.constrain_case(local, scrut, branches)?,
            canon::Expr_::Lambda(params, body) => self.constrain_lambda(local, params, body)?,
            canon::Expr_::Binop { func, lhs, rhs, .. } => {
                self.constrain_binop(local, *func, lhs, rhs)?
            }
            canon::Expr_::Let(bindings, body) => {
                // Sequential, monomorphic `let`: each binding's value is
                // constrained against the scope built so far, and its name binds
                // to that value's variable for the bindings that follow and the
                // `in` body. The whole `let`'s type is the body's type. It does
                // not generalise let-bound names — no let-polymorphism.
                let mut let_local = local.clone();
                for b in bindings {
                    let bv = self.constrain_expr(&let_local, &b.body)?;
                    // The binder may be a plain name or an irrefutable destructure
                    // (tuple / record); `constrain_pattern` ties the binder's
                    // shape to the value's type and binds every leaf variable.
                    self.constrain_pattern(&mut let_local, &b.pat, bv)?;
                }
                self.constrain_expr(&let_local, body)?
            }
            canon::Expr_::If(branches, else_expr) => {
                // Every condition is `Bool`; every branch and the final `else`
                // unify to one shared result type, which is the whole `if`'s
                // type. Mirrors `Ipe.Type.Constrain.Expression.constrainIf`.
                let result = self.flex()?;
                for (cond, body) in branches {
                    let cond_var = self.constrain_expr(local, cond)?;
                    let want_bool = self.bool_var()?;
                    // A condition expects `Bool`; a branch body expects the
                    // shared `if` result type.
                    self.record_expected(cond.span, want_bool);
                    self.eq(cond.span, cond_var, want_bool);
                    let body_var = self.constrain_expr(local, body)?;
                    self.record_expected(body.span, result);
                    self.eq(body.span, body_var, result);
                }
                let else_var = self.constrain_expr(local, else_expr)?;
                self.record_expected(else_expr.span, result);
                self.eq(else_expr.span, else_var, result);
                result
            }
            canon::Expr_::Tuple(elems) => {
                // A tuple's type is the product of its elements' types, each
                // constrained independently. Mirrors
                // `Ipe.Type.Constrain.Expression`'s tuple arm.
                let mut elem_vars = Vec::with_capacity(elems.len());
                for elem in elems {
                    elem_vars.push(self.constrain_expr(local, elem)?);
                }
                self.structure(FlatType::Tuple(elem_vars))?
            }
            canon::Expr_::List(elems) => self.constrain_list(local, elems)?,
            canon::Expr_::Cons(head, tail) => self.constrain_cons(local, head, tail)?,
            canon::Expr_::Record(fields) => self.constrain_record(local, fields)?,
            canon::Expr_::Access(record, field) => {
                self.constrain_access(local, record, *field, span)?
            }
            canon::Expr_::Update(base, fields) => {
                self.constrain_update(local, base, fields, span)?
            }
        };
        self.regions.insert((self.current_home.clone(), span), var);
        Ok(var)
    }

    /// Constrain a lambda `\p0 p1 ... -> body`. Each parameter gets a fresh
    /// flexible variable bound in the body's scope; the body is constrained
    /// there. The lambda's type is the right-nested arrow `p0 -> p1 -> … -> body`,
    /// so a surrounding `Call` unifies its callee against exactly this shape.
    /// Mirrors `Ipe.Type.Constrain.Expression`'s lambda arm.
    fn constrain_lambda(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        params: &[canon::Pattern],
        body: &canon::Expr,
    ) -> DResult<VarId> {
        let mut lam_local = local.clone();
        let mut param_vars = Vec::with_capacity(params.len());
        for p in params {
            let v = self.flex()?;
            self.constrain_pattern(&mut lam_local, p, v)?;
            // Record each lambda param's region so the lowerer can source a
            // record-param's complete field set from its solved type (one path
            // shared with the typed-def sites).  Keyed by `(current_home, span)`
            // to prevent cross-module span collisions.
            self.regions.insert((self.current_home.clone(), p.span), v);
            param_vars.push(v);
        }
        let mut arrow = self.constrain_expr(&lam_local, body)?;
        for pv in param_vars.into_iter().rev() {
            arrow = self.structure(FlatType::Fun(pv, arrow))?;
        }
        Ok(arrow)
    }

    /// Constrain a record literal `{ name = value, ... }`. Its type is the
    /// closed record `{ name : <field type>, ... }`, each field value
    /// constrained independently. Canonicalisation has already rejected a
    /// duplicate field name, so the resulting field map is exact.
    ///
    /// User-written record literals are always **closed** — they carry an
    /// `EmptyRecord` tail so the unifier rejects extra fields on either side.
    fn constrain_record(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        fields: &[(Symbol, canon::Expr)],
    ) -> DResult<VarId> {
        let mut field_vars = BTreeMap::new();
        for (name, value) in fields {
            let v = self.constrain_expr(local, value)?;
            field_vars.insert(*name, v);
        }
        let ext = self.empty_record_tail()?;
        self.structure(FlatType::Record(field_vars, ext))
    }

    /// Tie each reference to a wildcard-`any`-return binding to that binding's
    /// body result, so the body's settled type flows to every use site — closing
    /// the wildcard severance at its root. Run once EVERY def is constrained
    /// (so all body vars exist and the tie is independent of source order),
    /// before the main solve, so the tied type propagates through the same
    /// unification the use participates in. A `view = <binding>` whose body is
    /// `Html` therefore reaches the shape's `Element` requirement as an ordinary
    /// mismatch (rendered as IPE-T0020), rather than passing ipe and failing
    /// `cargo build`. Covers every indirection — direct reference, `let` alias
    /// chains, eta-expansion — because it is plain unification, not a syntactic
    /// reference walk.
    fn tie_wildcard_any_uses_to_bodies(&mut self) -> DResult<()> {
        let ties = std::mem::take(&mut self.wildcard_any_use_results);
        for (use_arrow, binding) in ties {
            let Some(&body_var) = self.wildcard_any_return_bodies.get(&binding) else {
                continue;
            };
            // Peel BOTH the use's instantiated arrow and the recorded body to
            // their final results, then tie the two result slots. The use arrow
            // is `param0 -> … -> any`; the body is either the applied result
            // (a def written with parameters) OR the same arrow shape (a
            // point-free def, `alias = view`), so peeling both reaches the
            // matching `any`/`Html` slot regardless of the def form or arity.
            let use_result = self.peel_arrow_result(use_arrow)?;
            let body_result = self.peel_arrow_result(body_var)?;
            self.eq(Span::DUMMY, use_result, body_result);
        }
        Ok(())
    }

    /// Follow a variable's settled structure, peeling leading `_ -> rest`
    /// arrows, and return the final non-arrow result. Bounded fuel guards a
    /// pathological cyclic chain.
    fn peel_arrow_result(&mut self, var: VarId) -> DResult<VarId> {
        let mut cur = self.uf.find(var)?;
        let mut fuel: u32 = 1024;
        while fuel > 0 {
            match self.uf.content(cur)? {
                Content::Structure(FlatType::Fun(_, ret)) => cur = self.uf.find(ret)?,
                _ => break,
            }
            fuel -= 1;
        }
        Ok(cur)
    }

    /// Constrain a record field access `record.field`. With closed records (no
    /// row variable), the field cannot be resolved until the record's type
    /// settles, so the access is deferred: a fresh result variable is its region
    /// type now, and [`crate::resolve_field_accesses`] links it to the field's
    /// type after the main solve.
    fn constrain_access(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        record: &canon::Expr,
        field: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        let record_var = self.constrain_expr(local, record)?;
        let result = self.flex()?;
        self.field_accesses.push(FieldAccess {
            record: record_var,
            field,
            result,
            span,
            home: self.current_home.clone(),
        });
        Ok(result)
    }

    /// Constrain a record update `{ base | field = value, ... }`. The result
    /// type is the base record's type (an update copies-and-replaces, changing
    /// no field's type), so the update's region variable *is* the base's. The
    /// field-existence + per-field type checks are deferred — closed records
    /// carry no row variable, so the base's type may not be settled yet —
    /// recorded here and discharged by [`crate::resolve_record_updates`] after
    /// the main solve.
    fn constrain_update(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        base: &canon::Expr,
        fields: &[(Symbol, canon::Expr)],
        span: Span,
    ) -> DResult<VarId> {
        let record_var = self.constrain_expr(local, base)?;
        let mut field_vars = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            let v = self.constrain_expr(local, value)?;
            field_vars.push((*name, v));
        }
        self.record_updates.push(RecordUpdate {
            record: record_var,
            fields: field_vars,
            span,
            home: self.current_home.clone(),
        });
        Ok(record_var)
    }

    /// Constrain a `case scrut of …`: the scrutinee shares one type, every arm
    /// pattern is checked against it, and every arm body unifies to one shared
    /// result — the whole `case`'s type.
    fn constrain_case(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        scrut: &canon::Expr,
        branches: &[canon::CaseBranch],
    ) -> DResult<VarId> {
        let scrut_var = self.constrain_expr(local, scrut)?;
        let result = self.flex()?;
        for br in branches {
            let mut br_local = local.clone();
            self.constrain_pattern(&mut br_local, &br.pat, scrut_var)?;
            let body_var = self.constrain_expr(&br_local, &br.body)?;
            // Every arm body expects the shared `case` result type.
            self.record_expected(br.body.span, result);
            self.eq(br.body.span, body_var, result);
        }
        Ok(result)
    }

    /// Constrain a constructor referenced as a value: its scheme instantiated
    /// fresh. A nullary constructor's value type is the enum itself; a payload
    /// constructor's is the curried arrow `field0 -> … -> T vars`. Each reference
    /// instantiates independently, so the same generic constructor used at `Int`
    /// and at `Bool` in one module yields two separately-satisfiable types. A
    /// constructor with no registered scheme (imported, outside the single-module
    /// subset) falls back to the bare enum type, sound for the nullary case.
    fn constrain_var_ctor(
        &mut self,
        home: &[Symbol],
        type_name: Symbol,
        name: Symbol,
    ) -> DResult<VarId> {
        if let Some(scheme) = self.ctors.get(&name).cloned() {
            let (arg_vars, result_var) = self.instantiate_ctor(&scheme)?;
            let mut t = result_var;
            for av in arg_vars.into_iter().rev() {
                t = self.structure(FlatType::Fun(av, t))?;
            }
            Ok(t)
        } else {
            self.con_var(home.to_vec(), type_name, Vec::new())
        }
    }

    /// Constrain a `case` arm pattern against the scrutinee's variable, binding
    /// any pattern variables into `local`.
    #[allow(clippy::too_many_lines)]
    fn constrain_pattern(
        &mut self,
        local: &mut BTreeMap<Symbol, VarId>,
        pat: &canon::Pattern,
        scrut_var: VarId,
    ) -> DResult<()> {
        match &pat.value {
            canon::Pattern_::PAnything => Ok(()),
            canon::Pattern_::PVar(s) => {
                local.insert(*s, scrut_var);
                Ok(())
            }
            canon::Pattern_::PCtor {
                home,
                type_name,
                name,
                args,
                ..
            } => {
                if let Some(scheme) = self.ctors.get(name).cloned() {
                    // A constructor pattern binds exactly its declared fields. A
                    // mismatch (`Just` with no payload, `Node l r` for a three-field
                    // `Node`) is a user error, surfaced as IPE-T0013 rather than
                    // silently constraining a prefix.
                    if args.len() != scheme.arg_tys.len() {
                        return Err(self.ctor_pattern_arity(
                            pat.span,
                            *name,
                            scheme.arg_tys.len(),
                            args.len(),
                        ));
                    }
                    // Instantiate the scheme fresh, tie the result to the
                    // scrutinee, and constrain each payload sub-pattern against its
                    // field's (now use-site) type. Recursing handles a nested
                    // sub-pattern's typing too; the lowerer is what restricts
                    // payloads to variables / wildcards.
                    let (arg_vars, result_var) = self.instantiate_ctor(&scheme)?;
                    self.eq(pat.span, result_var, scrut_var);
                    for (sub, av) in args.iter().zip(arg_vars) {
                        self.constrain_pattern(local, sub, av)?;
                        // Record this sub-pattern's own instantiated field type so
                        // the lowerer can recover a NESTED record / list sub-pattern's
                        // complete shape the same way a top-level `case` / `let` binder
                        // already does (identical precedent in `constrain_lambda`, the
                        // `regions.insert` on every lambda-parameter span below).
                        // Class 4 item C —
                        // docs/adr/0010-pattern-and-lowering-completeness.md.
                        self.regions
                            .insert((self.current_home.clone(), sub.span), av);
                    }
                } else {
                    // A constructor with no registered scheme (imported, outside the
                    // single-module subset): fall back to the bare enum type.
                    // We still must recurse into every argument sub-pattern so that
                    // pattern variables (e.g. `Chunk text` where `Chunk` is an
                    // imported ctor) get bound into `local`.  Without the recursion
                    // the body sees `VarLocal("text")` that is absent from the local
                    // map and fires the "unbound local" ICE.  Use a fresh flex
                    // variable per arg since the field types are unknown.
                    let ctor = self.con_var(home.clone(), *type_name, Vec::new())?;
                    self.eq(pat.span, ctor, scrut_var);
                    for sub in args {
                        let av = self.flex()?;
                        self.constrain_pattern(local, sub, av)?;
                    }
                }
                Ok(())
            }
            canon::Pattern_::PTuple(elems) => {
                // A tuple pattern matches a Tuple type element-wise: mint one
                // fresh variable per element, tie the scrutinee to the product
                // over them, and constrain each sub-pattern against its element's
                // variable. Nested sub-patterns recurse; the lowerer restricts
                // which element shapes it can actually emit.
                let mut elem_vars = Vec::with_capacity(elems.len());
                for _ in elems {
                    elem_vars.push(self.flex()?);
                }
                let tuple = self.structure(FlatType::Tuple(elem_vars.clone()))?;
                self.eq(pat.span, tuple, scrut_var);
                for (sub, ev) in elems.iter().zip(elem_vars) {
                    self.constrain_pattern(local, sub, ev)?;
                    // Same region-threading as the `PCtor` arm above so a record
                    // (or list) nested inside a TUPLE element (`(Ok {name}, y)`)
                    // recovers its complete shape in the lowerer. Class 4 item C.
                    self.regions
                        .insert((self.current_home.clone(), sub.span), ev);
                }
                Ok(())
            }
            canon::Pattern_::PRecord(fields) => {
                // A field-pun record pattern `{ x, y }` binds each named field of
                // the scrutinee record. Closed records carry no row variable, so
                // the scrutinee's full field set may not be settled here; instead
                // of forcing an exact-shape unification (which would reject the
                // legal subset pattern `{ x }` on a `{ x, y }` record), each
                // field is pulled out with the SAME deferred field-access channel
                // a `record.field` expression uses. After the main solve,
                // `resolve_field_accesses` links each binder to the field's type.
                for f in fields {
                    let result = self.flex()?;
                    self.field_accesses.push(FieldAccess {
                        record: scrut_var,
                        field: f.value,
                        result,
                        span: f.span,
                        home: self.current_home.clone(),
                    });
                    local.insert(f.value, result);
                }
                Ok(())
            }
            // A literal pattern pins the scrutinee to the literal's type. It
            // binds no names. A mismatch (`case n of "x" -> …` with `n : Int`)
            // surfaces as the ordinary IPE-T0001 type mismatch.
            canon::Pattern_::PInt(_) => {
                let lit = self.int_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PBool(_) => {
                let lit = self.bool_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PChar(_) => {
                let lit = self.char_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PStr(_) => {
                let lit = self.string_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            // An alias `inner as name` binds `name` to the whole scrutinee and
            // additionally constrains the inner pattern against it.
            canon::Pattern_::PAlias(inner, name) => {
                local.insert(name.value, scrut_var);
                self.constrain_pattern(local, inner, scrut_var)
            }
            // A list pattern `[a, b]` matches a `List elem`: each element
            // sub-pattern is constrained against one shared element variable, and
            // the scrutinee is tied to the list over it.
            canon::Pattern_::PList(elems) => {
                let elem = self.flex()?;
                let list = self.list_var(elem)?;
                self.eq(pat.span, list, scrut_var);
                for sub in elems {
                    self.constrain_pattern(local, sub, elem)?;
                }
                Ok(())
            }
            // A cons pattern `head :: tail` matches a `List elem`: `head : elem`,
            // `tail : List elem` (the scrutinee's own type), scrutinee `List elem`.
            canon::Pattern_::PCons(head, tail) => {
                let elem = self.flex()?;
                let list = self.list_var(elem)?;
                self.eq(pat.span, list, scrut_var);
                self.constrain_pattern(local, head, elem)?;
                self.constrain_pattern(local, tail, list)
            }
            // An or-pattern `p1 | p2 | …`: every alternative is constrained
            // against the SAME scrutinee variable, and its binders are unified
            // name-by-name with the first alternative's, so the arm body reads
            // one binder environment. Canon already proved the alternatives bind
            // the identical name set (IPE-T0019); unifying each shared name's
            // var here is the same-type half of the rule — a failure surfaces as
            // the ordinary IPE-T0001 mismatch attributed to the alternative. The
            // body is constrained ONCE afterwards, in `local`, never per
            // alternative.
            canon::Pattern_::POr(alts) => {
                let Some((first, rest)) = alts.split_first() else {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: "an or-pattern reached type inference with no alternatives"
                            .to_owned(),
                    });
                };
                // The first alternative binds directly into the shared `local`.
                self.constrain_pattern(local, first, scrut_var)?;
                for alt in rest {
                    let mut alt_local: BTreeMap<Symbol, VarId> = BTreeMap::new();
                    self.constrain_pattern(&mut alt_local, alt, scrut_var)?;
                    // Unify each of this alternative's binders with the reference
                    // binder of the same name established by the first alternative.
                    for (name, var) in alt_local {
                        if let Some(reference) = local.get(&name).copied() {
                            self.eq(alt.span, reference, var);
                        } else {
                            // Unreachable: canon proved every alternative binds
                            // the same names. Adopt the binder rather than drop it.
                            local.insert(name, var);
                        }
                    }
                }
                Ok(())
            }
        }
    }

    /// Build the IPE-T0013 diagnostic for a constructor pattern that binds the
    /// wrong number of payload fields. A forged constructor symbol surfaces the
    /// underlying intern bug instead.
    fn ctor_pattern_arity(
        &self,
        span: Span,
        ctor: Symbol,
        expected: usize,
        found: usize,
    ) -> Diagnostic {
        self.interner.resolve(ctor).map_or_else(
            || Diagnostic::CompilerBug {
                where_: "intern.resolve",
                detail: format!("no backing string for constructor symbol {}", ctor.as_raw()),
            },
            |s| Diagnostic::Type {
                span,
                msg: TypeError::CtorPatternArity {
                    ctor: Box::from(s),
                    expected,
                    found,
                },
            },
        )
    }

    /// Resolve a [`SchemeKey`] carried on a [`ipe_kernels::KernelDef`] to its
    /// concrete HM type scheme.
    ///
    /// A [`SchemeKey`] names a kernel's scheme without carrying it (the scheme is
    /// built from interned `Symbol`s that exist only after the `Interner` runs,
    /// so it cannot be a `'static` value on the descriptor). This is the single
    /// interpreter that turns the key back into a `Ty`: it delegates to
    /// [`Self::stdlib_scheme`], the authoritative scheme table, keyed on the
    /// kernel identity the key wraps. `None` mirrors `stdlib_scheme` — the kernel
    /// has no registry scheme (a routed / unlowered bucket). Routing every
    /// `def().scheme` read through this one adapter means the descriptor's scheme
    /// reference and the table can never resolve to different types.
    fn resolve_scheme(&self, key: SchemeKey) -> Option<Ty> {
        // A kernel that carries a structural `TyShape` is resolved by
        // interpreting it; the result is byte-identical to the `stdlib_scheme`
        // table's (pinned by `interpreted_shape_matches_legacy`). One without a
        // shape (`shape == None`) resolves through the table.
        if let Some(shape) = key.0.def().shape {
            return Some(self.interpret_shape(shape));
        }
        self.stdlib_scheme(key.0)
    }

    /// Interpret a `'static` [`TyShape`] into a concrete [`Ty`], resolving each
    /// [`BuiltinTag`] against the interned-symbol cache.
    ///
    /// The single interpreter a structural kernel scheme routes through. Its
    /// output is byte-identical to the `Ty` [`Self::stdlib_scheme`] produces for
    /// the same kernel (proven per-kernel by the `interpreted_shape_matches_legacy`
    /// tripwire).
    ///
    /// It touches no union-find state even for the polymorphic [`TyShape::Var`]
    /// node: a scheme var is interpreted to the SAME placeholder `Ty::Var` the
    /// `stdlib_scheme` table's `var(i)` builder produces — the bare positional
    /// index raw, in annotation-symbol space — NOT a fresh union-find var.
    /// Generalization / instantiation with fresh solver vars happens later at the
    /// use site (`instantiate_in`), exactly as for a table-built scheme, so this
    /// interpreter still takes `&self`. Because `Ty::Var` is `Eq`, repeating an
    /// index reuses one variable structurally without any shared-cell handling.
    fn interpret_shape(&self, shape: &TyShape) -> Ty {
        match shape {
            TyShape::Fun(arg, res) => Ty::Fun(
                Box::new(self.interpret_shape(arg)),
                Box::new(self.interpret_shape(res)),
            ),
            TyShape::Con(tag, args) => Ty::Con {
                module: self.builtin_con_module(*tag),
                name: self.builtin_symbol(*tag),
                args: args.iter().map(|a| self.interpret_shape(a)).collect(),
            },
            // Element order is preserved, matching the hand-built
            // `Ty::Tuple(vec![…])` a `stdlib_scheme` arm produces.
            TyShape::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|e| self.interpret_shape(e)).collect())
            }
            // The `BTreeMap` re-sorts by the resolved field `Symbol`, so the
            // key order is byte-identical to the hand-built `Ty::Record`
            // regardless of the declared slice order (the declared order is
            // additionally pinned ascending by `interpreted_shape_matches_legacy`).
            TyShape::Record { fields, tail } => {
                let mut map = BTreeMap::new();
                for (name, field) in *fields {
                    map.insert(self.field_symbol(*name), self.interpret_shape(field));
                }
                let tail = match tail {
                    RowTailShape::Closed => RowTail::Closed,
                    RowTailShape::Open(i) => RowTail::Open(u32::from(*i)),
                };
                Ty::Record(map, tail)
            }
            // The `stdlib_scheme` table binds `let var = Ty::Var`, so its
            // `var(i)` is `Ty::Var(i)`: a scheme-local variable's raw is its bare
            // positional index. Match that exactly for byte-identity.
            TyShape::Var(i) => Ty::Var(u32::from(*i)),
            // The `stdlib_scheme` table materialises `()` as the bare `Ty::Unit`
            // leaf; match it exactly.
            TyShape::Unit => Ty::Unit,
        }
    }

    /// Resolve a structural [`BuiltinTag`] to the interned type-constructor
    /// [`Symbol`] the `stdlib_scheme` table uses for the same built-in, so an
    /// interpreted shape is byte-identical to the hand-built `Ty`.
    const fn builtin_symbol(&self, tag: BuiltinTag) -> Symbol {
        match tag {
            BuiltinTag::Int => self.builtins.int,
            BuiltinTag::Float => self.builtins.float,
            BuiltinTag::Bool => self.builtins.bool,
            BuiltinTag::String => self.builtins.string,
            BuiltinTag::Char => self.builtins.char,
            BuiltinTag::Bytes => self.builtins.bytes,
            BuiltinTag::List => self.builtins.list,
            BuiltinTag::Maybe => self.builtins.maybe,
            BuiltinTag::Result => self.builtins.result,
            BuiltinTag::Set => self.builtins.set,
            BuiltinTag::Dict => self.builtins.dict,
            BuiltinTag::Order => self.builtins.order,
            BuiltinTag::Error => self.builtins.error,
            BuiltinTag::ErrorKind => self.builtins.errorkind,
            BuiltinTag::ErrorDetails => self.builtins.errordetails,
            BuiltinTag::Decimal => self.builtins.decimal,
            BuiltinTag::Task => self.builtins.task,
            BuiltinTag::Cmd => self.builtins.cmd,
            BuiltinTag::Sub => self.builtins.sub,
            BuiltinTag::Topic => self.builtins.topic_con,
            BuiltinTag::Decoder => self.builtins.decoder,
            BuiltinTag::Db => self.builtins.db,
            BuiltinTag::SqlValue => self.builtins.sqlvalue,
            BuiltinTag::SqlField => self.builtins.sqlfield,
            BuiltinTag::SqlFragment => self.builtins.sqlfragment,
            BuiltinTag::Secret => self.builtins.secret,
            BuiltinTag::Path => self.builtins.path,
            BuiltinTag::Regex => self.builtins.regex,
            BuiltinTag::Url => self.builtins.url,
            BuiltinTag::Dsn => self.builtins.dsn,
            BuiltinTag::Connection => self.builtins.connection,
            BuiltinTag::ConnReadOnly => self.builtins.conn_read_only,
            BuiltinTag::ConnReadWrite => self.builtins.conn_read_write,
            BuiltinTag::Locale => self.builtins.locale,
            BuiltinTag::HttpMethod => self.builtins.http_method,
            BuiltinTag::CryptoKey => self.builtins.crypto_key,
            BuiltinTag::CryptoMac => self.builtins.crypto_mac,
            BuiltinTag::EmailAddress => self.builtins.email_address,
            BuiltinTag::Claims => self.builtins.jwt_claims,
            BuiltinTag::Algorithm => self.builtins.jwt_algorithm,
            BuiltinTag::JsonValue => self.builtins.json_value,
            BuiltinTag::StreamId => self.builtins.stream_id,
            BuiltinTag::StreamWriter => self.builtins.stream_writer,
            BuiltinTag::WsServer => self.builtins.ws_server,
            BuiltinTag::WsServerCfg => self.builtins.ws_server_cfg,
            BuiltinTag::ServerRequest => self.builtins.server_request,
            BuiltinTag::ServerCookie => self.builtins.server_cookie,
            BuiltinTag::ServerRoute => self.builtins.server_route,
            // `Ipe.Ui.Attribute` and `Ipe.Html.Attribute` share this interned
            // `Attribute` name; they differ only in the module path
            // (`builtin_con_module`).
            BuiltinTag::UiAttribute | BuiltinTag::HtmlAttribute => self.builtins.attribute,
            BuiltinTag::UiElement => self.builtins.element,
            BuiltinTag::Html => self.builtins.html_con,
            BuiltinTag::UiLength => self.builtins.length,
            BuiltinTag::UiColor => self.builtins.color,
            BuiltinTag::UiDescription => self.builtins.description,
            BuiltinTag::UiPseudoClass => self.builtins.pseudo_class,
            BuiltinTag::InputLabel => self.builtins.input_label_con,
            BuiltinTag::InputPlaceholder => self.builtins.input_placeholder_con,
            BuiltinTag::InputRadioOption => self.builtins.input_radio_option_con,
            BuiltinTag::WebReq => self.builtins.web_req,
            BuiltinTag::WebRoute => self.builtins.live_route_con,
            BuiltinTag::EmailProvider => self.builtins.email_provider,
        }
    }

    /// Resolve a structural [`FieldTag`] to the interned field-name [`Symbol`]
    /// the `stdlib_scheme` table uses as the `Ty::Record` `BTreeMap` key for the
    /// same field, so an interpreted record shape is byte-identical to the
    /// hand-built `Ty::Record`.
    const fn field_symbol(&self, tag: FieldTag) -> Symbol {
        match tag {
            FieldTag::MigrationName => self.builtins.migration_f_name,
            FieldTag::MigrationSql => self.builtins.migration_f_sql,
            FieldTag::HttpBody => self.builtins.http_f_body,
            FieldTag::HttpHeaders => self.builtins.http_f_headers,
            FieldTag::HttpStatus => self.builtins.http_f_status,
            FieldTag::HttpMethod => self.builtins.http_f_method,
            FieldTag::HttpUrl => self.builtins.http_f_url,
            FieldTag::HttpTimeout => self.builtins.http_f_timeout,
            FieldTag::HttpFollowRedirects => self.builtins.http_f_follow_redirects,
            FieldTag::HttpMaxRedirects => self.builtins.http_f_max_redirects,
            FieldTag::ServerContentType => self.builtins.server_f_content_type,
            FieldTag::CsvHeader => self.builtins.csv_f_header,
            FieldTag::CsvRows => self.builtins.csv_f_rows,
            FieldTag::CacheMaxEntries => self.builtins.cache_f_max_entries,
            FieldTag::CacheTtlMs => self.builtins.cache_f_ttl_ms,
            FieldTag::CacheMaxBytes => self.builtins.cache_f_max_bytes,
            FieldTag::CacheHits => self.builtins.cache_f_hits,
            FieldTag::CacheMisses => self.builtins.cache_f_misses,
            FieldTag::CacheEvictions => self.builtins.cache_f_evictions,
            FieldTag::WsUrl => self.builtins.ws_f_url,
            FieldTag::WsHeaders => self.builtins.ws_f_headers,
            FieldTag::WsTimeout => self.builtins.ws_f_timeout,
            FieldTag::WsPingInterval => self.builtins.ws_f_ping_interval,
            FieldTag::EmailFrom => self.builtins.email_f_from,
            FieldTag::EmailTo => self.builtins.email_f_to,
            FieldTag::EmailCc => self.builtins.email_f_cc,
            FieldTag::EmailBcc => self.builtins.email_f_bcc,
            FieldTag::EmailSubject => self.builtins.email_f_subject,
            FieldTag::EmailTextBody => self.builtins.email_f_text_body,
            FieldTag::EmailHtmlBody => self.builtins.email_f_html_body,
            FieldTag::EmailAttachments => self.builtins.email_f_attachments,
            FieldTag::EmailReplyTo => self.builtins.email_f_reply_to,
            FieldTag::EmailFilename => self.builtins.email_f_filename,
            FieldTag::EmailMimeType => self.builtins.email_f_mime_type,
            FieldTag::EmailContent => self.builtins.email_f_content,
            FieldTag::RetryBaseMs => self.builtins.retry_f_base_ms,
            FieldTag::RetryJitter => self.builtins.retry_f_jitter,
            FieldTag::RetryKind => self.builtins.retry_f_kind,
            FieldTag::RetryMaxAttempts => self.builtins.retry_f_max_attempts,
            FieldTag::RetryShouldRetry => self.builtins.retry_f_should_retry,
            FieldTag::LayoutWrapperAttrs => self.builtins.lw_wrapper_attrs,
            FieldTag::LayoutRootAttrs => self.builtins.lw_root_attrs,
            FieldTag::ButtonOnPress => self.builtins.btn_f_on_press,
            FieldTag::Label => self.builtins.btn_f_label,
            FieldTag::AppInit => self.builtins.live_f_init,
            FieldTag::AppUpdate => self.builtins.live_f_update,
            FieldTag::AppView => self.builtins.live_f_view,
            FieldTag::AppSubscriptions => self.builtins.live_f_subscriptions,
            FieldTag::AppRoutes => self.builtins.live_f_routes,
            FieldTag::AppNotFound => self.builtins.live_f_not_found,
            FieldTag::TerminalOnKey => self.builtins.tui_f_on_key,
            FieldTag::TerminalKeyKind => self.builtins.tui_f_key_kind,
            FieldTag::TerminalKeyValue => self.builtins.tui_f_key_value,
            FieldTag::TerminalOnLine => self.builtins.cli_f_on_line,
            FieldTag::WebViewWindow => self.builtins.webview_f_window,
            FieldTag::WebViewTitle => self.builtins.webview_f_title,
            FieldTag::WebViewSize => self.builtins.webview_f_size,
            FieldTag::EdgeTop => self.builtins.edge_f_top,
            FieldTag::EdgeRight => self.builtins.edge_f_right,
            FieldTag::EdgeBottom => self.builtins.edge_f_bottom,
            FieldTag::EdgeLeft => self.builtins.edge_f_left,
            FieldTag::InputOnChange => self.builtins.input_f_on_change,
            FieldTag::InputText => self.builtins.input_f_text,
            FieldTag::InputPlaceholder => self.builtins.input_f_placeholder,
            FieldTag::InputIcon => self.builtins.input_f_icon,
            FieldTag::InputChecked => self.builtins.input_f_checked,
            FieldTag::InputSpellcheck => self.builtins.input_f_spellcheck,
            FieldTag::InputValue => self.builtins.input_f_value,
            FieldTag::InputMin => self.builtins.input_f_min,
            FieldTag::InputMax => self.builtins.input_f_max,
            FieldTag::InputStep => self.builtins.input_f_step,
            FieldTag::InputOptions => self.builtins.input_f_options,
            FieldTag::InputSelected => self.builtins.input_f_selected,
            FieldTag::ShadowOffsetX => self.builtins.shadow_f_offset_x,
            FieldTag::ShadowOffsetY => self.builtins.shadow_f_offset_y,
            FieldTag::ShadowBlur => self.builtins.shadow_f_blur,
            FieldTag::ShadowSpread => self.builtins.shadow_f_spread,
            FieldTag::ShadowColor => self.builtins.shadow_f_color,
            FieldTag::ImageSrc => self.builtins.img_f_src,
            FieldTag::ImageDescription => self.builtins.img_f_description,
        }
    }

    /// The module path an interpreted [`TyShape::Con`] carries for a given
    /// [`BuiltinTag`], mirroring the exact `Ty::Con { module, .. }` its
    /// `stdlib_scheme` arm builds.
    ///
    /// Every tag is empty-module (unqualified) EXCEPT
    /// [`BuiltinTag::HtmlAttribute`]: it shares the `Attribute` name with
    /// [`BuiltinTag::UiAttribute`] but is module-qualified with the `Html`
    /// constructor symbol, so `ir_type_from_ty`'s disambiguation selects the
    /// `Html` attribute variant that every `Ipe.Html` node kernel takes. Keeping
    /// this the ONE non-empty case preserves byte-identity for the qualified
    /// `Ipe.Html.Attribute` cons while leaving all other interpreted cons
    /// unqualified.
    fn builtin_con_module(&self, tag: BuiltinTag) -> Vec<Symbol> {
        match tag {
            BuiltinTag::HtmlAttribute => vec![self.builtins.html_con],
            _ => Vec::new(),
        }
    }

    /// Parse-once type scheme for a stdlib kernel, keyed by the pre-resolved
    /// [`StdlibKernel`] id carried on the `VarKernel` node. `None` = the
    /// kernel has no registry scheme, so the caller
    /// ([`Self::constrain_var_kernel`]) fails closed.
    ///
    /// `Math.min` / `Math.max` are EXCLUDED — they keep their dedicated
    /// `Comparable`-obligation path in `constrain_var_kernel`. The structural
    /// `Ty`-equality against the reference schemes is pinned per-kernel by the
    /// `stdlib_scheme_matches_legacy` parity tripwire, and the covered set is
    /// pinned by `migrated_set_burndown`.
    #[allow(clippy::too_many_lines)] // declarative scheme table — mirrors kernel_ty
    #[allow(clippy::match_same_arms)] // family-grouped declarative type table; merging cross-family arms with coincidentally-equal schemes would obscure the per-family structure
    fn stdlib_scheme(&self, k: StdlibKernel) -> Option<Ty> {
        use StdlibKernel as K;
        // Constructors mirror `kernel_ty`'s so the two tables stay byte-faithful
        // (verified structurally by `stdlib_scheme_matches_legacy`).
        let int = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.int,
            args: Vec::new(),
        };
        let float = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.float,
            args: Vec::new(),
        };
        let string = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.string,
            args: Vec::new(),
        };
        let bool_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.bool,
            args: Vec::new(),
        };
        let var = Ty::Var;
        let fun = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b));
        let list = |t: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.list,
            args: vec![t],
        };
        let maybe = |t: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.maybe,
            args: vec![t],
        };
        // `Char` is a zero-argument constructor (runtime rune / `char`).
        let char = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.char,
            args: Vec::new(),
        };
        // ── Scheme-builder closures (produce structurally identical `Ty`
        //    values across the kernel arms; the `stdlib_scheme_matches_legacy`
        //    tripwire proves the equality). ──
        let result = |e: Ty, a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.result,
            args: vec![e, a],
        };
        let dict = |kk: Ty, v: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.dict,
            args: vec![kk, v],
        };
        let set = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.set,
            args: vec![a],
        };
        // `Bytes` is a zero-argument constructor.
        let bytes = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.bytes,
            args: Vec::new(),
        };
        // `Order` is a zero-argument constructor.
        let order = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.order,
            args: Vec::new(),
        };
        // `Decimal` is a zero-argument constructor (Ipe.Decimal).
        let decimal = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.decimal,
            args: Vec::new(),
        };
        let error_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.error,
            args: Vec::new(),
        };
        // `ErrorKind` is a zero-argument constructor (the 11-variant kind union).
        let errorkind_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.errorkind,
            args: Vec::new(),
        };
        // `ErrorDetails` is a zero-argument constructor.
        let errordetails_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.errordetails,
            args: Vec::new(),
        };
        let tuple2 = |a: Ty, b: Ty| Ty::Tuple(vec![a, b]);
        // `task(a)` — `Task a` (the error channel is the implicit `IpeError`).
        let task = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.task,
            args: vec![a],
        };
        let task_unit = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.task,
            args: vec![Ty::Unit],
        };
        let cmd = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.cmd,
            args: vec![m],
        };
        let sub = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.sub,
            args: vec![m],
        };
        // `topic(a)` — `Topic a` — the phantom topic-handle type.
        // Erases to `String` at runtime; used only in kernel type schemes so
        // that publisher and subscriber share the same payload type variable.
        let topic = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.topic_con,
            args: vec![a],
        };
        // `dec(inner)` — `Decoder inner` — the opaque row-decoder type shared by
        // JSON decode and Db.Decode.
        let dec = |inner: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.decoder,
            args: vec![inner],
        };
        // Opaque nullary type constructors (mirror `kernel_ty`'s inline `Ty::Con`s).
        let db = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.db,
            args: Vec::new(),
        };
        // `Ipe.Db.Migration` is a record alias `{ name : String, sql : String }`.
        // `Db.migrate` schemes over `List Migration`, and `Db.defaultMigration`
        // returns one — so a program can build migrations as record literals. The
        // record folds to a synthesised `Rec…` struct; the `DbMigrate` emit
        // converts each to a `(name, sql)` tuple for the `db_migrate_apply`
        // runtime kernel.
        let migration = || {
            let string = || Ty::Con {
                module: Vec::new(),
                name: self.builtins.string,
                args: Vec::new(),
            };
            let mut m_fields = BTreeMap::new();
            m_fields.insert(self.builtins.migration_f_name, string());
            m_fields.insert(self.builtins.migration_f_sql, string());
            Ty::Record(m_fields, RowTail::Closed)
        };
        let sqlvalue = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.sqlvalue,
            args: Vec::new(),
        };
        let sqlfield = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.sqlfield,
            args: Vec::new(),
        };
        // `SqlFragment` — `Ipe.Db.Sql`'s opaque WHERE-fragment type.
        let sqlfragment = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.sqlfragment,
            args: Vec::new(),
        };
        // `Secret` — `Ipe.Secret`'s opaque sealed secret-string type.
        let secret = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.secret,
            args: Vec::new(),
        };
        // `Path` — `Ipe.Path`'s opaque validated filesystem-path type.
        let path = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.path,
            args: Vec::new(),
        };
        // `Regex` — `Ipe.Regex`'s opaque compiled-pattern handle.
        let regex = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.regex,
            args: Vec::new(),
        };
        let req = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_request,
            args: Vec::new(),
        };
        // `Ipe.Http.Server.Response` is a record alias `{ status : Int, body :
        // String, headers : Dict String String, contentType : String }`
        // (reference `Ipê/Http/Server.ipe:66`), NOT an opaque nominal. Every
        // server kernel that produces/consumes a `Response` schemes over this
        // record so a handler-built record literal — and a field read off a
        // `Response` — unify with the kernel signatures. The record folds to the
        // runtime `IrType::ServerResponse` struct at lowering (see
        // `ipe_lower::is_server_response_shape`).
        let resp = || {
            let string = || Ty::Con {
                module: Vec::new(),
                name: self.builtins.string,
                args: Vec::new(),
            };
            let mut resp_fields = BTreeMap::new();
            resp_fields.insert(self.builtins.http_f_body, string());
            resp_fields.insert(self.builtins.server_f_content_type, string());
            resp_fields.insert(
                self.builtins.http_f_headers,
                Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.dict,
                    args: vec![string(), string()],
                },
            );
            resp_fields.insert(
                self.builtins.http_f_status,
                Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.int,
                    args: Vec::new(),
                },
            );
            Ty::Record(resp_fields, RowTail::Closed)
        };
        let route = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_route,
            args: Vec::new(),
        };
        let cookie = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_cookie,
            args: Vec::new(),
        };
        // `sw()` — the opaque `StreamWriter` handle. Used by
        // `Stream.stream` callback arg and `Stream.emit`/`finish`/`withContentType`.
        let sw = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.stream_writer,
            args: Vec::new(),
        };
        // `stream_id()` — the opaque `StreamId` handle from
        // `Ipe.Http.Stream`. Backed at runtime by
        // `ipe_runtime::http_stream::IpeStreamId` (a newtype over `i64`).
        // Used as the return type of `HttpStream.open` and the first argument
        // of `forEachChunk`, `close`, and `chunks`.
        let stream_id = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.stream_id,
            args: Vec::new(),
        };
        // `wsh()` — the opaque `WsHandle` per-peer handle.
        // Used as the first arg of every WsServerCfg callback and as the
        // target of `sendToClient` / `sendBinaryToClient` / `broadcast` / `closeClient`.
        let wsh = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.ws_server,
            args: Vec::new(),
        };
        // `wscfg()` — the opaque `WsServerCfg<IpeError>` configuration type.
        // Built by `Ws.defaultCfg` and threaded through the builder chain.
        let wscfg = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.ws_server_cfg,
            args: Vec::new(),
        };
        // ── stdlib record / opaque-Con helpers ─────────────────────────
        // `Csv` — closed record `{ header : List String, rows : List (List
        // String) }` (runtime `ipe_runtime::csv::CsvDoc`).
        let csv_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.csv_f_header, list(string()));
            m.insert(self.builtins.csv_f_rows, list(list(string())));
            Ty::Record(m, RowTail::Closed)
        };
        // `CacheCfg` — closed record `{ maxEntries : Int, ttlMs : Int,
        // maxBytes : Int }`. The lowerer folds a value of this exact shape to the
        // nominal `IrType::CacheCfg` (`ipe_runtime::cache::CacheCfg`) so a
        // `Cache.defaultCfg`-built record literal constructs the runtime struct
        // the `cache_new_raw` kernel takes (mirrors the `HttpRequest` fold).
        let cachecfg_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.cache_f_max_entries, int());
            m.insert(self.builtins.cache_f_ttl_ms, int());
            m.insert(self.builtins.cache_f_max_bytes, int());
            Ty::Record(m, RowTail::Closed)
        };
        // `Cache.stats` return — closed record `{ hits : Int,
        // misses : Int, evictions : Int }` (runtime `ipe_runtime::cache::
        // CacheStats`). Consumed by field access on the kernel result, exactly
        // like `Csv`'s `CsvDoc` return, so no lowerer fold is needed on this side.
        let cache_stats_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.cache_f_hits, int());
            m.insert(self.builtins.cache_f_misses, int());
            m.insert(self.builtins.cache_f_evictions, int());
            Ty::Record(m, RowTail::Closed)
        };
        // `WebSocketCfg` — closed record `{ url : String, headers :
        // List (String, String), timeout : Int, pingInterval : Int }`. The
        // lowerer folds a value of this exact shape to the nominal
        // `IrType::WebSocketClientCfg` (`ipe_runtime::ws_client::WsClientCfg`) so
        // a `WebSocket.defaultCfg`-built record literal constructs the runtime
        // struct the `web_socket_connect_with` kernel takes (mirrors the
        // `HttpRequest` / `CacheCfg` folds).
        let wsclientcfg = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.ws_f_url, string());
            m.insert(self.builtins.ws_f_headers, list(tuple2(string(), string())));
            m.insert(self.builtins.ws_f_timeout, int());
            m.insert(self.builtins.ws_f_ping_interval, int());
            Ty::Record(m, RowTail::Closed)
        };
        // Ipe.Email: `EmailProvider` opaque ADT (runtime
        // `ipe_runtime::email::EmailProvider`). Empty-module `Con` (home-
        // insensitive lowering, same posture as `ws_server`); the Ipê
        // `type EmailProvider` declaration unifies with it structurally by name.
        let email_provider = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.email_provider,
            args: Vec::new(),
        };
        // `Key` — opaque role-typed crypto key (`ipe_runtime::crypto::Key`).
        // The ONLY constructor is `Key.fromString`/`Key.fromBytes`; no implicit
        // `String` coercion.  Lowered to `IrType::CryptoKey`.
        let crypto_key = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.crypto_key,
            args: Vec::new(),
        };
        // `Mac` — opaque role-typed MAC output (`ipe_runtime::crypto::Mac`).
        // Produced exclusively by the `*WithKey` kernels; extracted via `Mac.toHex`.
        // Lowered to `IrType::CryptoMac`.
        let crypto_mac = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.crypto_mac,
            args: Vec::new(),
        };
        // `EmailAddress` — opaque validated email address
        // (`ipe_runtime::email::EmailAddress`).  The ONLY constructor is
        // `EmailAddress.parse`; extracted via `EmailAddress.toString`.
        // Lowered to `IrType::EmailAddress`.
        let email_address = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.email_address,
            args: Vec::new(),
        };
        // `Url` — `Ipe.Url`'s opaque validated URL type
        // (`ipe_runtime::url::Url`). The ONLY constructor is `Url.fromString`;
        // extracted via `Url.toString`. Lowered to `IrType::Url`.
        let url = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.url,
            args: Vec::new(),
        };
        // `Dsn` — opaque validated connection descriptor
        // (`ipe_runtime::dsn::Dsn`). Constructed only by `Db.Dsn.parse` /
        // `Db.Dsn.build`; lowered to `IrType::Dsn`.
        let dsn = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.dsn,
            args: Vec::new(),
        };
        // External `Connection mode` — the read-only-by-type foreign-DB handle
        // (`ipe_runtime::external_conn::ExternalConnection`). The phantom `mode`
        // (`ReadOnly` / `ReadWrite`) is a real type at inference so a read-only
        // value cannot unify into a write kernel; erased at emit.
        let conn_read_only = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.conn_read_only,
            args: Vec::new(),
        };
        let conn_read_write = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.conn_read_write,
            args: Vec::new(),
        };
        let connection = |mode: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.connection,
            args: vec![mode],
        };
        // `Locale` — opaque BCP-47 locale handle
        // (`ipe_runtime::locale::Locale`).  The ONLY constructor is
        // `Locale.fromTag`; extracted via `Locale.toTag`.
        // Lowered to `IrType::Locale`.
        let locale = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.locale,
            args: Vec::new(),
        };
        // `EmailMessage` — closed 9-field record (runtime
        // `ipe_runtime::email::EmailMessage`). The lowerer folds a value of this
        // exact shape to the nominal `IrType::EmailMessage` so a
        // `defaultMessage`-built record literal constructs the runtime struct the
        // `email_send` kernel takes (mirrors the `CsvDoc` / `CacheCfg` folds).
        let email_message_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.email_f_from, string());
            m.insert(self.builtins.email_f_to, list(string()));
            m.insert(self.builtins.email_f_cc, list(string()));
            m.insert(self.builtins.email_f_bcc, list(string()));
            m.insert(self.builtins.email_f_subject, string());
            m.insert(self.builtins.email_f_text_body, string());
            m.insert(self.builtins.email_f_html_body, string());
            // `attachments : List Attachment` — the element is the runtime
            // `EmailAttachment` record shape `{ filename, mimeType, content }`.
            let mut att = BTreeMap::new();
            att.insert(self.builtins.email_f_filename, string());
            att.insert(self.builtins.email_f_mime_type, string());
            att.insert(self.builtins.email_f_content, bytes());
            m.insert(
                self.builtins.email_f_attachments,
                list(Ty::Record(att, RowTail::Closed)),
            );
            m.insert(self.builtins.email_f_reply_to, string());
            Ty::Record(m, RowTail::Closed)
        };
        let attr = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.attribute,
            args: vec![m],
        };
        // `Ipe.Html.Attribute` — SAME name as `Ipe.Ui.Attribute` (`attr` above)
        // but module-qualified with `html_con`, so `ir_type_from_ty`'s T2
        // disambiguation selects `HtmlAttribute`, matching the runtime
        // `Vec<html::Attribute<M>>` that every Ipe.Html node kernel takes
        // (div/span/a/button/p/input/img/node/styleNode/attrToString). Using the
        // bare Ui `attr` for these would mis-select the Ui attribute variant.
        let html_attr = |m: Ty| Ty::Con {
            module: vec![self.builtins.html_con],
            name: self.builtins.attribute,
            args: vec![m],
        };
        let elem_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.element,
            args: vec![m],
        };
        let html_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.html_con,
            args: vec![m],
        };
        // `label_t(msg)` — `Label msg` from `Ipe.Ui.Input`.
        // Lowered to `IrType::Ui { ctor: UiCtor::Label, msg }` via the `"Label"`
        // arm in `ipe_lower::ir_type_from_ty`. The type carries the module path
        // `[input_con]` so it doesn't collide with any user `type Label`.
        // (We use an empty module here because `ir_type_from_ty` routes all
        // unqualified `"Label"` cons to `UiCtor::Label` regardless — the name is
        // reserved in the kernel namespace and never appears as a user type.)
        let label_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.input_label_con,
            args: vec![m],
        };
        // `placeholder_t(msg)` — `Placeholder msg` from `Ipe.Ui.Input`.
        // Lowered to `IrType::Ui { ctor: UiCtor::Placeholder, msg }`.
        let placeholder_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.input_placeholder_con,
            args: vec![m],
        };
        // `radio_option_t(msg)` — `RadioOption msg` from `Ipe.Ui.Input`.
        // Lowered to `IrType::Ui { ctor: UiCtor::RadioOption, msg }`.
        let radio_option_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.input_radio_option_con,
            args: vec![m],
        };
        // Nullary Ipe.Ui plain types (`Length` / `Color`) — lowered to
        // `IrType::UiPlain(UiPlain::Length | UiPlain::Color)`.
        let length = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.length,
            args: Vec::new(),
        };
        let color = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.color,
            args: Vec::new(),
        };
        // `description()` — the opaque `Description` semantic-description type
        // produced by `Ui.descMain` / `Ui.descHeading` / …. Lowered to
        // `IrType::UiPlain(UiPlain::Description)`.
        let description = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.description,
            args: Vec::new(),
        };
        // `pseudo_class()` — the opaque `PseudoClass` selector-tag type produced
        // by `Ui.hover` / `Ui.focus` / `Ui.focusVisible` / `Ui.active` /
        // `Ui.disabled` and consumed by `Ui.onPseudo`. Lowered to
        // `IrType::UiPlain(UiPlain::PseudoClass)`.
        let pseudo_class = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.pseudo_class,
            args: Vec::new(),
        };
        // `value()` — the opaque `Value = any` JSON node produced/consumed by the
        // `JsonEnc.*` encoders. Lowered to `IrType::Json` (`JsonVal`).
        let value = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.json_value,
            args: Vec::new(),
        };
        // ── JWT builder opaque types (D-00) ─────────────────────────────────
        // `claims_ty()` — opaque JWT claims accumulator.  Backed at runtime by
        // `serde_json::Value` (maps to `IrType::Json` in the lowerer).
        let claims_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.jwt_claims,
            args: Vec::new(),
        };
        // `algorithm_ty()` — JWT signing algorithm descriptor.  Backed at
        // runtime by a sealed `Ipe.Secret` wrapping the string
        // `"HS256:<secret>"` or `"RS256:<pem>"` (maps to `IrType::Secret` in
        // the lowerer) — the key material never gets a `Debug`/`Display`/
        // stringify surface, mirroring `Ipe.Secret` itself.
        let algorithm_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.jwt_algorithm,
            args: Vec::new(),
        };
        let web_req = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.web_req,
            args: Vec::new(),
        };
        // `live_route(page)` — `WebRoute page` is parametric on the page type.
        // Its purpose is to carry the page type through HM unification so that
        //   routes : List (WebRoute var(2))            [K::WebApp]
        //   Web.route : String -> builder -> WebRoute page  [K::WebRoute]
        //   notFound : var(2)
        // all share ONE page type variable.  A `notFound = 5` in a routed app
        // that also uses `Web.route "/" CounterPage` sets `var(2) = Page`
        // (through the per-route witness — see [`RouteWitnessCheck`]) and then
        // forces `5 : Page` → IPE-T0001.  Seal fix — the
        // "exit-0-then-cargo-fail E0308" class.  Since round 4 the arg is no
        // longer phantom at the IR level: the lowerer threads it into
        // `IrType::WebRoute(page)` so the backend renders `Route<Page>`.
        let live_route = |page: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.live_route_con,
            args: vec![page],
        };
        // `HttpResponse = { body : String, headers : Dict String String, status : Int }`
        let http_response = || {
            let mut resp_fields = BTreeMap::new();
            resp_fields.insert(self.builtins.http_f_body, string());
            resp_fields.insert(self.builtins.http_f_headers, dict(string(), string()));
            resp_fields.insert(self.builtins.http_f_status, int());
            Ty::Record(resp_fields, RowTail::Closed)
        };
        // `HttpMethod` — the closed ADT (`Get | Post | Put | Delete | Patch |
        // Head | Options`).  Like `Order` and `Decimal`, it is known to the
        // type system as a zero-argument constructor with an empty module path
        // (builtins-like treatment; the Ipê source defines it in `Ipe.Http`
        // but the compiler folds it as a pre-interned nominal, analogous to
        // how `Order` is defined in `Ipe.Basics` but treated as a builtin here).
        let http_method_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.http_method,
            args: Vec::new(),
        };
        // `HttpRequest = { body, followRedirects, headers, maxRedirects, method, timeout, url }`
        // `method` is now `HttpMethod` (ADT), not `String`.
        let http_request = || {
            let mut req_fields = BTreeMap::new();
            req_fields.insert(self.builtins.http_f_body, string());
            req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty());
            req_fields.insert(
                self.builtins.http_f_headers,
                list(tuple2(string(), string())),
            );
            req_fields.insert(self.builtins.http_f_max_redirects, int());
            req_fields.insert(self.builtins.http_f_method, http_method_ty());
            req_fields.insert(self.builtins.http_f_timeout, int());
            req_fields.insert(self.builtins.http_f_url, string());
            Ty::Record(req_fields, RowTail::Closed)
        };
        // `RetryPolicy e = { baseMs : Int, jitter : Bool, kind : Int,
        //                    maxAttempts : Int, shouldRetry : e -> Bool }`
        // Fields sorted alphabetically (BTreeMap order) — this matches the
        // Rust struct `RecBaseMsJitterKindMaxAttemptsShouldRetry<T1>` that
        // the backend emits for this record type.
        let retry_policy = |e: Ty| {
            let mut rp_fields = BTreeMap::new();
            rp_fields.insert(self.builtins.retry_f_base_ms, int());
            rp_fields.insert(self.builtins.retry_f_jitter, bool_ty());
            rp_fields.insert(self.builtins.retry_f_kind, int());
            rp_fields.insert(self.builtins.retry_f_max_attempts, int());
            rp_fields.insert(self.builtins.retry_f_should_retry, fun(e, bool_ty()));
            Ty::Record(rp_fields, RowTail::Closed)
        };
        Some(match k {
            // ── List (kernel-anchored combinators) ──
            // map : (a -> b) -> List a -> List b
            K::ListMap => fun(fun(var(0), var(1)), fun(list(var(0)), list(var(1)))),
            // filter : (a -> Bool) -> List a -> List a
            K::ListFilter => fun(fun(var(0), bool_ty()), fun(list(var(0)), list(var(0)))),
            // foldl / foldr : (a -> b -> b) -> b -> List a -> b
            K::ListFoldl | K::ListFoldr => fun(
                fun(var(0), fun(var(1), var(1))),
                fun(var(1), fun(list(var(0)), var(1))),
            ),
            // length : List a -> Int
            K::ListLength => fun(list(var(0)), int()),
            // head : List a -> Maybe a
            K::ListHead => fun(list(var(0)), maybe(var(0))),
            // tail : List a -> Maybe (List a)
            K::ListTail => fun(list(var(0)), maybe(list(var(0)))),
            // member : a -> List a -> Bool
            K::ListMember => fun(var(0), fun(list(var(0)), bool_ty())),
            // range : Int -> Int -> List Int
            K::ListRange => fun(int(), fun(int(), list(int()))),
            // reverse : List a -> List a
            K::ListReverse => fun(list(var(0)), list(var(0))),
            // append : List a -> List a -> List a
            K::ListAppend => fun(list(var(0)), fun(list(var(0)), list(var(0)))),
            // concat : List (List a) -> List a
            K::ListConcat => fun(list(list(var(0))), list(var(0))),
            // take : Int -> List a -> List a
            K::ListTake => fun(int(), fun(list(var(0)), list(var(0)))),
            // drop : Int -> List a -> List a
            K::ListDrop => fun(int(), fun(list(var(0)), list(var(0)))),
            // zip : List a -> List b -> List (a, b)
            K::ListZip => fun(
                list(var(0)),
                fun(list(var(1)), list(tuple2(var(0), var(1)))),
            ),
            // cons : a -> List a -> List a
            K::ListCons => fun(var(0), fun(list(var(0)), list(var(0)))),
            // isEmpty : List a -> Bool
            K::ListIsEmpty => fun(list(var(0)), bool_ty()),
            // concatMap : (a -> List b) -> List a -> List b
            K::ListConcatMap => fun(fun(var(0), list(var(1))), fun(list(var(0)), list(var(1)))),
            // indexedMap : (Int -> a -> b) -> List a -> List b
            K::ListIndexedMap => fun(
                fun(int(), fun(var(0), var(1))),
                fun(list(var(0)), list(var(1))),
            ),
            // any / all : (a -> Bool) -> List a -> Bool
            K::ListAny | K::ListAll => fun(fun(var(0), bool_ty()), fun(list(var(0)), bool_ty())),
            // find : (a -> Bool) -> List a -> Maybe a
            K::ListFind => fun(fun(var(0), bool_ty()), fun(list(var(0)), maybe(var(0)))),
            // ── List batch ────────────────────────────────────────────
            // filterMap : (a -> Maybe b) -> List a -> List b
            K::ListFilterMap => fun(fun(var(0), maybe(var(1))), fun(list(var(0)), list(var(1)))),
            // sortBy : (a -> comparable) -> List a -> List a — BASE scheme only.
            // var(0)=a (element), var(1)=key type (Comparable obligation layered in
            // constrain_var_kernel, keyed off id, same pattern as MathMin/MathMax).
            // Production never reaches this arm (obligation pre-check early-returns
            // the bounded scheme); it exists so `stdlib_scheme` is total.
            K::ListSortBy => fun(fun(var(0), var(1)), fun(list(var(0)), list(var(0)))),
            // sort : comparable a => List a -> List a — BASE scheme only (Ord
            // obligation layered in `constrain_var_kernel`, keyed off id).
            K::ListSort => fun(list(var(0)), list(var(0))),
            // sortWith : (a -> a -> Order) -> List a -> List a — fully generic
            // (the comparator supplies the ordering), so no obligation is needed.
            K::ListSortWith => fun(
                fun(var(0), fun(var(0), order())),
                fun(list(var(0)), list(var(0))),
            ),
            // singleton : a -> List a
            K::ListSingleton => fun(var(0), list(var(0))),
            // repeat : Int -> a -> List a
            K::ListRepeat => fun(int(), fun(var(0), list(var(0)))),
            // sum / product : number a => List a -> a — BASE scheme only
            // (number obligation layered in `constrain_var_kernel`).
            K::ListSum | K::ListProduct => fun(list(var(0)), var(0)),
            // maximum / minimum : comparable a => List a -> Maybe a — BASE
            // scheme only (Ord obligation layered in `constrain_var_kernel`).
            K::ListMaximum | K::ListMinimum => fun(list(var(0)), maybe(var(0))),
            // unique : List a -> List a — fully generic (equality-only, tested
            // with `==` by the runtime; no Ord/Hash obligation, exactly like
            // `List.member`), so the scheme needs no bounded var.
            K::ListUnique => fun(list(var(0)), list(var(0))),
            // intersperse : a -> List a -> List a
            K::ListIntersperse => fun(var(0), fun(list(var(0)), list(var(0)))),
            // partition : (a -> Bool) -> List a -> (List a, List a)
            K::ListPartition => fun(
                fun(var(0), bool_ty()),
                fun(list(var(0)), tuple2(list(var(0)), list(var(0)))),
            ),
            // unzip : List (a, b) -> (List a, List b)
            K::ListUnzip => fun(
                list(tuple2(var(0), var(1))),
                tuple2(list(var(0)), list(var(1))),
            ),
            // map2 : (a -> b -> r) -> List a -> List b -> List r.
            // vars: 0=a, 1=b, 2=r.
            K::ListMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(list(var(0)), fun(list(var(1)), list(var(2)))),
            ),
            // map3 : (a -> b -> c -> r) -> List a -> List b -> List c -> List r.
            // vars: 0=a, 1=b, 2=c, 3=r.
            K::ListMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    list(var(0)),
                    fun(list(var(1)), fun(list(var(2)), list(var(3)))),
                ),
            ),
            // map4 : (a -> b -> c -> d -> r) -> List a..d -> List r. vars 0..4.
            K::ListMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    list(var(0)),
                    fun(
                        list(var(1)),
                        fun(list(var(2)), fun(list(var(3)), list(var(4)))),
                    ),
                ),
            ),
            // map5 : (a -> b -> c -> d -> e -> r) -> List a..e -> List r. vars 0..5.
            K::ListMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    list(var(0)),
                    fun(
                        list(var(1)),
                        fun(
                            list(var(2)),
                            fun(list(var(3)), fun(list(var(4)), list(var(5)))),
                        ),
                    ),
                ),
            ),

            // ── Basics core Prelude (6 — slice) ──
            K::BasicsIdentity => fun(var(0), var(0)),
            K::BasicsAlways => fun(var(0), fun(var(1), var(0))),
            K::BasicsFst => fun(tuple2(var(0), var(1)), var(0)),
            K::BasicsSnd => fun(tuple2(var(0), var(1)), var(1)),
            K::BasicsModBy => fun(int(), fun(int(), int())),
            // `clamp : comparable -> comparable -> comparable -> comparable`.
            // BASE scheme only — three independent `var(0)`s; the shared
            // `Comparable a` (Ord) obligation is layered on in
            // `constrain_var_kernel` (keyed off id), exactly as `Math.min` /
            // `Math.max`. Production never reaches this arm (the obligation
            // pre-check early-returns the bounded scheme); it exists so
            // `stdlib_scheme` is total and the burndown tripwire holds.
            K::BasicsClamp => fun(var(0), fun(var(0), fun(var(0), var(0)))),
            // toString : a -> String — base scheme for the totality gate; the
            // real STRINGIFY-bounded typing is direct-built in constrain_var_kernel,
            // same pattern as clamp/min/max.
            K::BasicsToString => fun(var(0), string()),
            // ── Basics numerics ────────────────────────────────────────
            // negate / abs: `number a => a -> a`. BASE scheme only (bounded scheme
            // is direct-built in constrain_var_kernel). Production never reaches
            // this arm (obligation pre-check early-returns); exists for the totality
            // gate (`stdlib_scheme_total_over_reachable`).
            K::BasicsNegate | K::BasicsAbs => fun(var(0), var(0)),
            // sqrt : Float -> Float — monomorphic, no obligation pre-check needed.
            // min / max: `comparable a => a -> a -> a`. BASE scheme only (bounded
            // scheme is direct-built in constrain_var_kernel, same as MathMin/MathMax).
            K::BasicsMin | K::BasicsMax => fun(var(0), fun(var(0), var(0))),
            // `compare`: base scheme (production hits the direct-build in
            // constrain_var_kernel; this arm exists for the totality gate).
            K::BasicsCompare => fun(var(0), fun(var(0), order())),
            // ── end Basics numerics ────────────────────────────────────

            // ── Math (min / max stay on the obligation path — NOT migrated) ──
            // Constants — bare Float values (arity 0).
            // isNaN : Float -> Bool.
            // abs : Int -> Int.
            // Arity-1 Float -> Float.
            // Arity-1 Float -> Int (rounding functions).
            // Arity-2 Float -> Float -> Float.
            // Math.min / max — BASE scheme only (the `Comparable a` obligation is
            // layered on top in `constrain_var_kernel`, keyed off the id). The
            // parity tripwire checks this base against `kernel_ty("Math","min")`;
            // production never reaches this arm for min/max (the obligation
            // pre-check early-returns the bounded scheme).
            K::MathMin | K::MathMax => fun(var(0), fun(var(0), var(0))),

            // ── Random seeded (Generator primitives) — pure, reproducible ──
            // seededIntRaw : Int -> Int -> Int -> (Int, Int)   (seed, lo, hi) → (value, nextSeed)
            K::RandomSeededInt => fun(int(), fun(int(), fun(int(), tuple2(int(), int())))),
            // seededFloatRaw : Int -> (Float, Int)             seed → (value, nextSeed)
            K::RandomSeededFloat => fun(int(), tuple2(float(), int())),
            // seededChoiceRaw : Int -> List a -> (Maybe a, Int)  (seed, list) → (choice, nextSeed)
            K::RandomSeededChoice => {
                fun(int(), fun(list(var(0)), tuple2(maybe(var(0)), int())))
            }

            // ── Log ──
            // info/debug/warn/error : String -> Task Error (). The
            // *With variants (List (String, a) attrs) are Stringify-bounded and
            // stay fail-closed until a Stringify obligation is added.
            K::LogInfo | K::LogDebug | K::LogWarn | K::LogError => fun(string(), task_unit()),
            // *With : String -> List a -> Task Error () where `a` is Stringify.
            // Base scheme for the totality gate; the Stringify obligation on the
            // list-element var(0) is tied in constrain_var_kernel.
            K::LogInfoWith | K::LogDebugWith | K::LogWarnWith | K::LogErrorWith => {
                fun(string(), fun(list(var(0)), task_unit()))
            }

            // ── Maybe ──
            K::MaybeWithDefault => fun(var(0), fun(maybe(var(0)), var(0))),
            K::MaybeMap => fun(fun(var(0), var(1)), fun(maybe(var(0)), maybe(var(1)))),
            K::MaybeAndThen => fun(
                fun(var(0), maybe(var(1))),
                fun(maybe(var(0)), maybe(var(1))),
            ),
            // `map2 : (a -> b -> v) -> Maybe a -> Maybe b -> Maybe v`. The N-ary
            // function is CURRIED at the Ipê type level (`a -> b -> v`); the
            // backend passes the multi-arg Rust fn value directly (mirrors
            // JsonDec.map2). var(0)=a, var(1)=b, .., last=v.
            K::MaybeMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(maybe(var(0)), fun(maybe(var(1)), maybe(var(2)))),
            ),
            K::MaybeMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    maybe(var(0)),
                    fun(maybe(var(1)), fun(maybe(var(2)), maybe(var(3)))),
                ),
            ),
            K::MaybeMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    maybe(var(0)),
                    fun(
                        maybe(var(1)),
                        fun(maybe(var(2)), fun(maybe(var(3)), maybe(var(4)))),
                    ),
                ),
            ),
            K::MaybeMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    maybe(var(0)),
                    fun(
                        maybe(var(1)),
                        fun(
                            maybe(var(2)),
                            fun(maybe(var(3)), fun(maybe(var(4)), maybe(var(5)))),
                        ),
                    ),
                ),
            ),
            // `andMap : Maybe a -> Maybe (a -> b) -> Maybe b`. var(0)=a, var(1)=b.
            K::MaybeAndMap => fun(
                maybe(var(0)),
                fun(maybe(fun(var(0), var(1))), maybe(var(1))),
            ),
            // `combine : List (Maybe a) -> Maybe (List a)`. var(0)=a.
            K::MaybeCombine => fun(list(maybe(var(0))), maybe(list(var(0)))),
            // `isJust : Maybe a -> Bool`. var(0)=a.
            K::MaybeIsJust => fun(maybe(var(0)), bool_ty()),
            // `isNothing : Maybe a -> Bool`. var(0)=a.
            K::MaybeIsNothing => fun(maybe(var(0)), bool_ty()),

            // ── Result ──
            K::ResultWithDefault => fun(var(0), fun(result(var(1), var(0)), var(0))),
            K::ResultMap => fun(
                fun(var(0), var(1)),
                fun(result(var(2), var(0)), result(var(2), var(1))),
            ),
            // `andThen : (a -> Result e b) -> Result e a -> Result e b`.
            // var(0)=a, var(1)=e, var(2)=b. The error channel `e` is shared
            // across the callback's Result, the input Result, and the output.
            K::ResultAndThen => fun(
                fun(var(0), result(var(1), var(2))),
                fun(result(var(1), var(0)), result(var(1), var(2))),
            ),
            // `mapError : (e -> f) -> Result e a -> Result f a`.
            // var(0)=e, var(1)=f, var(2)=a. Maps the error channel; the `Ok`
            // value type `a` is preserved.
            K::ResultMapError => fun(
                fun(var(0), var(1)),
                fun(result(var(0), var(2)), result(var(1), var(2))),
            ),
            // `map2 : (a -> b -> v) -> Result e a -> Result e b -> Result e v`.
            // The error channel `e` is SHARED across all input `Result`s and the
            // output. var(0)=a, var(1)=b, var(2)=v, last var = e (shared).
            K::ResultMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(
                    result(var(3), var(0)),
                    fun(result(var(3), var(1)), result(var(3), var(2))),
                ),
            ),
            K::ResultMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    result(var(4), var(0)),
                    fun(
                        result(var(4), var(1)),
                        fun(result(var(4), var(2)), result(var(4), var(3))),
                    ),
                ),
            ),
            K::ResultMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    result(var(5), var(0)),
                    fun(
                        result(var(5), var(1)),
                        fun(
                            result(var(5), var(2)),
                            fun(result(var(5), var(3)), result(var(5), var(4))),
                        ),
                    ),
                ),
            ),
            K::ResultMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    result(var(6), var(0)),
                    fun(
                        result(var(6), var(1)),
                        fun(
                            result(var(6), var(2)),
                            fun(
                                result(var(6), var(3)),
                                fun(result(var(6), var(4)), result(var(6), var(5))),
                            ),
                        ),
                    ),
                ),
            ),
            // `andMap : Result e a -> Result e (a -> b) -> Result e b`.
            // var(0)=a, var(1)=b, var(2)=e (shared).
            K::ResultAndMap => fun(
                result(var(2), var(0)),
                fun(
                    result(var(2), fun(var(0), var(1))),
                    result(var(2), var(1)),
                ),
            ),
            // `combine : List (Result e a) -> Result e (List a)`.
            // var(0)=a, var(1)=e.
            K::ResultCombine => fun(
                list(result(var(1), var(0))),
                result(var(1), list(var(0))),
            ),
            // `traverse : (a -> Result e b) -> List a -> Result e (List b)`.
            // var(0)=a, var(1)=b, var(2)=e.
            K::ResultTraverse => fun(
                fun(var(0), result(var(2), var(1))),
                fun(list(var(0)), result(var(2), list(var(1)))),
            ),
            // `toMaybe : Result e a -> Maybe a`. var(0)=e, var(1)=a.
            K::ResultToMaybe => fun(result(var(0), var(1)), maybe(var(1))),
            // `fromMaybe : e -> Maybe a -> Result e a`. var(0)=e, var(1)=a.
            K::ResultFromMaybe => fun(var(0), fun(maybe(var(1)), result(var(0), var(1)))),

            // ── Bytes ──
            K::BytesToString => fun(bytes(), maybe(string())),
            K::BytesFromHex | K::BytesFromBase64 => fun(string(), maybe(bytes())),

            // ── Task ──
            K::TaskSucceed => fun(var(0), task(var(0))),
            K::TaskFail => fun(error_ty(), task(var(0))),
            K::TaskMap => fun(fun(var(0), var(1)), fun(task(var(0)), task(var(1)))),
            // `Task.map2 : (a -> b -> r) -> Task Error a -> Task Error b -> Task Error r`.
            K::TaskMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(task(var(0)), fun(task(var(1)), task(var(2)))),
            ),
            K::TaskMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    task(var(0)),
                    fun(task(var(1)), fun(task(var(2)), task(var(3)))),
                ),
            ),
            K::TaskMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    task(var(0)),
                    fun(
                        task(var(1)),
                        fun(task(var(2)), fun(task(var(3)), task(var(4)))),
                    ),
                ),
            ),
            K::TaskMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    task(var(0)),
                    fun(
                        task(var(1)),
                        fun(
                            task(var(2)),
                            fun(task(var(3)), fun(task(var(4)), task(var(5)))),
                        ),
                    ),
                ),
            ),
            // `Task.attempt : (Result Error a -> msg) -> Task Error a -> Cmd msg`.
            // var(0)=a, var(1)=msg. Mirrors `Cmd.perform` with args reordered.
            K::TaskAttempt => fun(
                fun(result(error_ty(), var(0)), var(1)),
                fun(task(var(0)), cmd(var(1))),
            ),
            K::TaskAndThen => fun(fun(var(0), task(var(1))), fun(task(var(0)), task(var(1)))),
            K::TaskMapError => fun(fun(error_ty(), error_ty()), fun(task(var(0)), task(var(0)))),
            K::TaskOnError => fun(
                fun(error_ty(), task(var(0))),
                fun(task(var(0)), task(var(0))),
            ),
            K::TaskFromResult => fun(result(var(0), var(1)), task(var(1))),
            K::TaskAndThenResult => fun(
                fun(var(0), result(var(1), var(2))),
                fun(task(var(0)), task(var(2))),
            ),
            K::TaskSequence | K::TaskParallel => fun(list(task(var(0))), task(list(var(0)))),
            // `Task.run : Task Error a -> Result Error a`.
            // The error channel is the fixed `Error` type — using `var(1)` here
            // leaves the result's error type free, causing IPE-L0102 at the
            // `main` binding in programs that end with `|> Task.run` and have no
            // annotation that would pin `var(1)` to `Error`.
            K::TaskRun => fun(task(var(0)), result(error_ty(), var(0))),
            // `Task.perform` is a 1-arg legacy alias for `Task.run`; identical type.
            K::TaskPerform => fun(task(var(0)), result(error_ty(), var(0))),
            // `Task.lazy : (() -> Task e a) -> Task e a`
            K::TaskLazy => fun(fun(Ty::Unit, task(var(0))), task(var(0))),
            // ── Task retry surface ──────────────────────────────────────────
            // `linearBackoff : Int -> Int -> RetryPolicy e`
            K::TaskLinearBackoff => fun(int(), fun(int(), retry_policy(var(0)))),
            // `exponentialBackoff : Int -> Int -> RetryPolicy e`
            K::TaskExponentialBackoff => fun(int(), fun(int(), retry_policy(var(0)))),
            // `withJitter : RetryPolicy e -> RetryPolicy e`
            K::TaskWithJitter => fun(retry_policy(var(0)), retry_policy(var(0))),
            // `retryOn / withRetryOn : (e -> Bool) -> RetryPolicy e -> RetryPolicy e`
            K::TaskRetryOn | K::TaskWithRetryOn => {
                fun(fun(var(0), bool_ty()), fun(retry_policy(var(0)), retry_policy(var(0))))
            }
            // `defaultRetryPolicy : RetryPolicy e`
            K::TaskDefaultRetryPolicy => retry_policy(var(0)),
            // `withMaxAttempts / withBaseMs / withKind : Int -> RetryPolicy e -> RetryPolicy e`
            K::TaskWithMaxAttempts | K::TaskWithBaseMs | K::TaskWithKind => {
                fun(int(), fun(retry_policy(var(0)), retry_policy(var(0))))
            }
            // `retryWith : RetryPolicy Error -> Task Error a -> Task Error a`
            K::TaskRetryWith => {
                fun(retry_policy(error_ty()), fun(task(var(0)), task(var(0))))
            }

            // ── Io / File / System: String -> Task () ──
            K::IoWriteStdout
            | K::IoWriteStderr
            | K::IoPrintln
            | K::IoEprintln
            | K::SystemUnsetenv => fun(string(), task_unit()),
            // File path-consuming `Path -> Task ()` kernels (typed path, not
            // a raw `String` — construction is the validated boundary).
            K::FileRemove | K::FileMkdirAll | K::FileDelete => fun(path(), task_unit()),
            // () -> Task String
            K::IoReadLine | K::SystemCwd => fun(Ty::Unit, task(string())),
            // prompt String -> Task String (echo-suppressed line read)
            K::IoReadSecret => fun(string(), task(string())),
            // ── Debug (dev-only) ──
            // `Debug.log : String -> a -> a`. BASE scheme only; the argument /
            // result share `var(0)`, which carries the STRINGIFY obligation
            // (`show`), tied in `constrain_var_kernel` (like `Log.*With`). A
            // production build rejects any use before this scheme is reached
            // (IPE-L0140, `reject_dev_only_kernels`).
            K::DebugLog => fun(string(), fun(var(0), var(0))),

            // ── Time ──
            K::TimeNow | K::TimeUnixMillis => fun(Ty::Unit, task(int())),
            K::TimeSleep => fun(int(), task_unit()),
            K::TimeEvery => fun(int(), fun(var(0), sub(var(0)))),

            // ── System ──
            // `getenv` takes an env-var NAME (String); `tempFile`/`tempDir`
            // take a filename PREFIX (String, sanitised in the runtime), so
            // these stay `String -> Task String` — they do not consume a path.
            K::SystemGetenv | K::FileTempFile | K::FileTempDir => fun(string(), task(string())),
            // `readFile` consumes a validated `Path`.
            K::FileReadFile => fun(path(), task(string())),
            K::SystemArgs => fun(Ty::Unit, task(list(string()))),
            K::SystemLoadEnv => fun(Ty::Unit, task_unit()),
            K::SystemSetenv => fun(string(), fun(string(), task_unit())),
            // `writeFile`/`append` take a `Path` then the content `String`.
            K::FileWriteFile | K::FileAppend => fun(path(), fun(string(), task_unit())),
            // `copy`/`rename` take two `Path`s (source then destination).
            K::FileCopy | K::FileRename => fun(path(), fun(path(), task_unit())),
            K::SystemGetArg => fun(int(), task(maybe(string()))),
            K::SystemGetenvInt => fun(string(), task(int())),
            K::SystemGetenvBool => fun(string(), task(bool_ty())),
            // `exists`/`isDir` query a validated `Path`.
            K::FileExists | K::FileIsDir => fun(path(), task(bool_ty())),
            K::SystemExit => fun(int(), var(0)),

            // ── Random ──
            K::RandomInt => fun(int(), fun(int(), task(int()))),
            K::RandomFloat => fun(float(), fun(float(), task(float()))),
            K::RandomChoice => fun(list(var(0)), task(var(0))),
            // choice : List a -> Task Error (Maybe a)   (total; Nothing when empty)
            K::RandomChoiceMaybe => fun(list(var(0)), task(maybe(var(0)))),
            // shuffle : List a -> Task Error (List a)
            K::RandomShuffle => fun(list(var(0)), task(list(var(0)))),
            // weighted : List (Float, a) -> Task Error (Maybe a)   (total)
            K::RandomWeighted => fun(list(tuple2(float(), var(0))), task(maybe(var(0)))),

            // ── Process ──
            // `run : String -> List String -> Task Error String`
            K::ProcessRun => fun(string(), fun(list(string()), task(string()))),

            // ── File (remaining) — all consume a validated `Path` ──
            K::FileReadDir => fun(path(), task(list(string()))),
            K::FileReadFileLimit => fun(path(), fun(int(), task(string()))),
            K::FileReadFileBytes => fun(path(), task(list(int()))),

            // ── Http ──
            K::HttpGet => fun(string(), task(http_response())),
            K::HttpPost => fun(string(), fun(string(), task(http_response()))),
            K::HttpRequest => fun(http_request(), task(http_response())),
            K::HttpParseQuery => fun(string(), dict(string(), string())),
            K::HttpDefaultRequest => fun(url(), result(error_ty(), http_request())),
            K::HttpDefaultRequestFromString => fun(string(), result(error_ty(), http_request())),
            K::HttpWithMethod => fun(http_method_ty(), fun(http_request(), http_request())),
            K::HttpMethodFromString => fun(string(), maybe(http_method_ty())),
            K::HttpMethodToString => fun(http_method_ty(), string()),
            K::HttpWithTimeout => fun(int(), fun(http_request(), http_request())),
            K::HttpWithBody => fun(string(), fun(http_request(), http_request())),
            K::HttpWithHeader => fun(string(), fun(string(), fun(http_request(), http_request()))),
            K::HttpWithUrl => fun(url(), fun(http_request(), result(error_ty(), http_request()))),
            K::HttpWithFollowRedirects => fun(bool_ty(), fun(http_request(), http_request())),
            K::HttpWithMaxRedirects => fun(int(), fun(http_request(), http_request())),

            // ── Cmd ──
            K::CmdNone => cmd(var(0)),
            K::CmdBatch => fun(list(cmd(var(0))), cmd(var(0))),
            K::CmdPerform => fun(
                task(var(0)),
                fun(fun(result(error_ty(), var(0)), var(1)), cmd(var(1))),
            ),
            // `Cmd.map : (a -> msg) -> Cmd a -> Cmd msg` — retag a
            // sub-component's commands. var(0)=a (child msg), var(1)=msg (parent).
            K::CmdMap => fun(fun(var(0), var(1)), fun(cmd(var(0)), cmd(var(1)))),

            // ── Cmd.publish / Cmd.publishNoEcho ──
            // `Cmd.publish : Topic a -> a -> Cmd msg`
            // var(0) = msg, var(1) = payload type `a`
            K::CmdPublish => fun(topic(var(1)), fun(var(1), cmd(var(0)))),
            // `Cmd.publishNoEcho : Topic a -> a -> Cmd msg`
            K::CmdPublishNoEcho => fun(topic(var(1)), fun(var(1), cmd(var(0)))),

            // ── Sub ──
            K::SubNone => sub(var(0)),
            K::SubBatch => fun(list(sub(var(0))), sub(var(0))),
            K::SubEvery => fun(int(), fun(var(0), sub(var(0)))),
            // `Sub.map : (a -> msg) -> Sub a -> Sub msg` — the `Sub` twin of
            // `Cmd.map`. var(0)=a (child msg), var(1)=msg (parent).
            K::SubMap => fun(fun(var(0), var(1)), fun(sub(var(0)), sub(var(1)))),
            // `Sub.subscribeTopic : Topic a -> (a -> msg) -> Sub msg`
            // var(0) = msg, var(1) = payload type `a`
            K::SubSubscribeTopic => fun(topic(var(1)), fun(fun(var(1), var(0)), sub(var(0)))),

            // ── PubSub.publish / publishNoEcho ──
            // `PubSub.publish    : Topic a -> a -> Task Error Int`
            // `PubSub.publishNoEcho : Topic a -> a -> Task Error Int`
            // var(0) = payload type `a`.  Result is `Task Error Int` (subscriber
            // count), NOT `Cmd msg` — no `msg` type var, distinct from `Cmd.publish`.
            K::PubSubPublish => fun(topic(var(0)), fun(var(0), task(int()))),
            K::PubSubPublishNoEcho => fun(topic(var(0)), fun(var(0), task(int()))),

            // `PubSub.topic : String -> Topic a`
            // var(0) = payload type `a`
            K::PubSubTopic => fun(string(), topic(var(0))),

            // ── Server ──
            K::ServerGet
            | K::ServerPost
            | K::ServerPut
            | K::ServerDelete
            | K::ServerAny
            | K::ServerApi => fun(string(), fun(fun(req(), task(resp())), route())),
            K::ServerStatic => fun(string(), fun(string(), route())),
            K::ServerListen => fun(int(), fun(list(route()), task_unit())),
            K::ServerText | K::ServerJson | K::ServerHtml | K::ServerRedirect => {
                fun(string(), resp())
            }
            K::ServerWithStatus => fun(int(), fun(resp(), resp())),
            K::ServerWithHeader => fun(string(), fun(string(), fun(resp(), resp()))),
            K::ServerParam | K::ServerQueryParam | K::ServerHeader | K::ServerGetCookie => {
                fun(string(), fun(req(), maybe(string())))
            }
            K::ServerBody | K::ServerPath | K::ServerMethod => fun(req(), string()),
            K::ServerCookieNew => fun(string(), fun(string(), cookie())),
            K::ServerWithCookie => fun(cookie(), fun(resp(), resp())),

            // ── Middleware ──
            K::MiddlewareWithCors => fun(
                list(string()),
                fun(fun(req(), task(resp())), fun(req(), task(resp()))),
            ),
            K::MiddlewareWithLogging => fun(fun(req(), task(resp())), fun(req(), task(resp()))),
            K::MiddlewareWithBasicAuth => fun(
                string(),
                fun(
                    string(),
                    fun(fun(req(), task(resp())), fun(req(), task(resp()))),
                ),
            ),
            K::MiddlewareWithRateLimit => fun(
                string(),
                fun(
                    int(),
                    fun(
                        int(),
                        fun(fun(req(), task(resp())), fun(req(), task(resp()))),
                    ),
                ),
            ),
            K::MiddlewareWithCsrf => fun(fun(req(), task(resp())), fun(req(), task(resp()))),

            // ── Db ──
            K::DbConnect => fun(Ty::Unit, task(db())),
            K::DbOpen => fun(string(), fun(string(), task(db()))),
            K::DbClose => fun(db(), task_unit()),

            // ── Ipe.Db.Dsn — parse-don't-validate descriptor. ──
            // parse : String -> Result Error Dsn
            K::DsnParse => fun(string(), result(error_ty(), dsn())),
            // build : Int -> String -> Int -> String -> String -> Secret -> Int
            //   -> Result Error Dsn  (driverTag, host, port, database, user,
            //   password, tlsTag)
            K::DsnBuild => fun(
                int(),
                fun(
                    string(),
                    fun(
                        int(),
                        fun(
                            string(),
                            fun(
                                string(),
                                fun(secret(), fun(int(), result(error_ty(), dsn()))),
                            ),
                        ),
                    ),
                ),
            ),
            K::DsnDriverTag | K::DsnPort | K::DsnTlsTag => fun(dsn(), int()),
            K::DsnHost | K::DsnDatabase | K::DsnUser | K::DsnRedacted => fun(dsn(), string()),

            // ── External Connection — read-only-by-type foreign-DB connect. ──
            // open : Dsn -> Task Error (Connection ReadOnly)
            K::DbConnOpen => fun(dsn(), task(connection(conn_read_only()))),
            // close : Connection a -> Task Error ()  (polymorphic over the mode)
            K::DbConnClose => fun(connection(var(0)), task_unit()),
            // unsafeExecRawOn : Connection ReadWrite -> String -> Task Error Int
            K::DbConnUnsafeExecRawOn => {
                fun(connection(conn_read_write()), fun(string(), task(int())))
            }
            // ── External read path — mode-polymorphic `Connection a` first arg. ──
            // A read is available on any access mode, so the mode is a free var.
            // findWhereOn : Connection a -> String -> SqlFragment
            //               -> Task Error (List Row)
            K::DbConnFindWhere => fun(
                connection(var(0)),
                fun(
                    string(),
                    fun(sqlfragment(), task(list(dict(string(), string())))),
                ),
            ),
            // getByIdOn : Connection a -> String -> String
            //             -> Task Error (Maybe Row)
            K::DbConnGetById => fun(
                connection(var(0)),
                fun(
                    string(),
                    fun(string(), task(maybe(dict(string(), string())))),
                ),
            ),
            // queryDecodeOn : Connection a -> String -> List b -> Decoder c
            //                 -> Task Error (List c). var(2) = access mode (free,
            // never unifies with the decoder var(0) or the params-elem var(1)).
            K::DbConnQueryDecode => fun(
                connection(var(2)),
                fun(
                    string(),
                    fun(list(var(1)), fun(dec(var(0)), task(list(var(0))))),
                ),
            ),
            K::DbExecRaw => fun(db(), fun(string(), task(int()))),
            // `exec`/`query`/`queryDecode` accept `List a` (polymorphic) — any
            // Ipê type that can be bound as a SQL parameter: `List String`,
            // `List Int`, `List Float`, `List Bool`, or `List SqlValue` (typed
            // mixed-type binding introduced in v0.16.26).  The emitter routes all
            // three to `db_exec_params` / `db_query_params` /
            // `db_query_decode_params`, converting elements via
            // `ipe_runtime::db::SqlParam::from` which is implemented for every
            // Ipê-primitive type as well as for the generated `StdDbSqlValue`.
            K::DbExec => fun(db(), fun(string(), fun(list(var(0)), task(int())))),
            K::DbQuery => fun(
                db(),
                fun(
                    string(),
                    fun(list(var(0)), task(list(dict(string(), string())))),
                ),
            ),
            K::DbQueryDecode => fun(
                db(),
                fun(
                    string(),
                    // var(1) = element type of the params list (unconstrained);
                    // var(0) = decoder result type.
                    fun(list(var(1)), fun(dec(var(0)), task(list(var(0))))),
                ),
            ),
            K::DbGetString | K::DbGetField => {
                fun(string(), fun(dict(string(), string()), string()))
            }
            K::DbGetInt => fun(string(), fun(dict(string(), string()), int())),
            K::DbGetBool => fun(string(), fun(dict(string(), string()), bool_ty())),
            K::DbInsertRow => fun(
                db(),
                fun(string(), fun(dict(string(), string()), task(int()))),
            ),
            K::DbGetById => fun(
                db(),
                fun(
                    string(),
                    fun(string(), task(maybe(dict(string(), string())))),
                ),
            ),
            K::DbUpdateById => fun(
                db(),
                fun(
                    string(),
                    fun(string(), fun(dict(string(), string()), task(int()))),
                ),
            ),
            K::DbDeleteById => fun(db(), fun(string(), fun(string(), task(int())))),
            K::DbFindOneByField => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(string(), task(maybe(dict(string(), string())))),
                    ),
                ),
            ),
            K::DbFindManyByField => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(string(), task(list(dict(string(), string())))),
                    ),
                ),
            ),
            K::DbFindByConditions => fun(
                db(),
                fun(
                    string(),
                    fun(
                        dict(string(), string()),
                        task(list(dict(string(), string()))),
                    ),
                ),
            ),
            // `Db.findWhere : Db -> String -> SqlFragment -> Task (List Row)`
            // — the `SqlFragment`-typed replacement for the removed
            // `unsafeFindWhere`. A caller can never pass a raw
            // `String` WHERE clause here: only the `Sql.*` combinators below
            // produce a `SqlFragment`, so a naive string-concatenated WHERE
            // clause is a IPE-T0001 type mismatch, not a runtime risk.
            K::DbFindWhere => fun(
                db(),
                fun(string(), fun(sqlfragment(), task(list(dict(string(), string()))))),
            ),
            // `Db.deleteWhere : Db -> String -> SqlFragment -> Task Int`
            K::DbDeleteWhere => fun(db(), fun(string(), fun(sqlfragment(), task(int())))),
            // `Db.updateWhere : Db -> String -> List (String, SqlField)
            //                   -> SqlFragment -> Task Int`
            K::DbUpdateWhere => fun(
                db(),
                fun(
                    string(),
                    fun(
                        list(tuple2(string(), sqlfield())),
                        fun(sqlfragment(), task(int())),
                    ),
                ),
            ),
            K::DbInsertFields => fun(
                db(),
                fun(
                    string(),
                    fun(list(tuple2(string(), sqlfield())), task(int())),
                ),
            ),
            K::DbUpdateFields => fun(
                db(),
                fun(
                    string(),
                    fun(
                        list(tuple2(string(), sqlvalue())),
                        fun(list(tuple2(string(), sqlfield())), task(int())),
                    ),
                ),
            ),
            K::DbInsertFieldsReturning => fun(
                db(),
                fun(
                    string(),
                    fun(
                        list(tuple2(string(), sqlfield())),
                        fun(string(), fun(dec(var(0)), task(list(var(0))))),
                    ),
                ),
            ),
            K::DbWithTransaction => fun(db(), fun(fun(db(), task(var(0))), task(var(0)))),
            // `Db.migrate : Db -> List Migration -> Task Error (List String)`.
            // The record-shaped `Migration` API is the surface; the
            // `db_migrate_apply` runtime kernel still takes `(name, sql)` pairs —
            // the emitter converts at the call site.
            K::DbMigrate => fun(
                db(),
                fun(list(migration()), task(list(string()))),
            ),
            // `Db.defaultMigration : String -> Migration` — a Migration named
            // with an empty SQL body.
            K::DbDefaultMigration => fun(string(), migration()),

            // ── Db.Decode ──
            K::DbDecString => fun(string(), dec(string())),
            K::DbDecInt => fun(string(), dec(int())),
            K::DbDecFloat => fun(string(), dec(float())),
            K::DbDecBool => fun(string(), dec(bool_ty())),
            K::DbDecFail => fun(string(), dec(var(0))),
            K::DbDecNullable => fun(dec(var(0)), dec(maybe(var(0)))),
            K::DbDecMap => fun(fun(var(0), var(1)), fun(dec(var(0)), dec(var(1)))),
            K::DbDecAndThen => fun(fun(var(0), dec(var(1))), fun(dec(var(0)), dec(var(1)))),
            K::DbDecSucceed => fun(var(0), dec(var(0))),
            K::DbDecMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dec(var(0)), fun(dec(var(1)), dec(var(2)))),
            ),
            K::DbDecMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(dec(var(0)), fun(dec(var(1)), fun(dec(var(2)), dec(var(3))))),
            ),
            K::DbDecMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    dec(var(0)),
                    fun(dec(var(1)), fun(dec(var(2)), fun(dec(var(3)), dec(var(4))))),
                ),
            ),
            K::DbDecRequired => fun(
                string(),
                fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),
            ),
            K::DbDecOptional => fun(
                string(),
                fun(
                    dec(var(0)),
                    fun(var(0), fun(dec(fun(var(0), var(1))), dec(var(1)))),
                ),
            ),
            // `Db.Decode.money : String -> Decoder (Decimal, String)` — decodes a
            // `"ISO_CODE AMOUNT"` TEXT column (the lossless serialisation
            // `SqlMoney` writes on INSERT) back into its amount/currency-code
            // pair. Deliberately NOT `Decoder Money`: `Money`/`Currency` are
            // project-generated types unnameable from this crate (see
            // `docs/adr/0013-multi-driver-db-compile-time-selection.md`) — a
            // recorded divergence from the Go backend's `Decoder Money`,
            // documented in `docs/divergences-from-sky.md`.
            K::DbDecMoney => fun(string(), dec(tuple2(decimal(), string()))),
            // `Db.Decode.bytes : String -> Decoder (List Int)` — hex-decodes a
            // BYTEA/BLOB column. Ipê's `Bytes`/`List Int` representation is a
            // `List Int`; the runtime returns `Vec<u8>` which lowers identically.
            // FIRST_SCHEMED (Ipê-new, no legacy oracle).
            K::DbDecBytes => fun(string(), dec(list(int()))),

            // ── Set (base schemes; the `set_elem` obligation is layered in
            //    constrain_var_kernel, keyed off the id) ──
            K::SetEmpty => set(var(0)),
            K::SetSize => fun(set(var(0)), int()),
            K::SetInsert | K::SetRemove => fun(var(0), fun(set(var(0)), set(var(0)))),
            K::SetMember => fun(var(0), fun(set(var(0)), bool_ty())),
            K::SetToList => fun(set(var(0)), list(var(0))),
            K::SetFromList => fun(list(var(0)), set(var(0))),
            K::SetUnion | K::SetIntersect | K::SetDiff => {
                fun(set(var(0)), fun(set(var(0)), set(var(0))))
            }
            // isEmpty : Set a -> Bool
            K::SetIsEmpty => fun(set(var(0)), bool_ty()),
            // singleton : a -> Set a
            K::SetSingleton => fun(var(0), set(var(0))),
            // foldl / foldr : (a -> b -> b) -> b -> Set a -> b
            K::SetFoldl | K::SetFoldr => fun(
                fun(var(0), fun(var(1), var(1))),
                fun(var(1), fun(set(var(0)), var(1))),
            ),
            // map : (a -> b) -> Set a -> Set b (var 0=a AND var 1=b carry the
            // set_elem Ord obligation, layered in constrain_var_kernel).
            K::SetMap => fun(fun(var(0), var(1)), fun(set(var(0)), set(var(1)))),
            // filter : (a -> Bool) -> Set a -> Set a
            K::SetFilter => fun(fun(var(0), bool_ty()), fun(set(var(0)), set(var(0)))),
            // partition : (a -> Bool) -> Set a -> (Set a, Set a)
            K::SetPartition => fun(
                fun(var(0), bool_ty()),
                fun(set(var(0)), tuple2(set(var(0)), set(var(0)))),
            ),

            // ── Dict (base schemes; the `dict_key` obligation is layered in
            //    constrain_var_kernel, keyed off the id) ──
            K::DictEmpty => dict(var(0), var(1)),
            K::DictIsEmpty => fun(dict(var(0), var(1)), bool_ty()),
            K::DictSize => fun(dict(var(0), var(1)), int()),
            K::DictInsert => fun(
                var(0),
                fun(var(1), fun(dict(var(0), var(1)), dict(var(0), var(1)))),
            ),
            K::DictGet => fun(var(0), fun(dict(var(0), var(1)), maybe(var(1)))),
            K::DictRemove => fun(var(0), fun(dict(var(0), var(1)), dict(var(0), var(1)))),
            K::DictMember => fun(var(0), fun(dict(var(0), var(1)), bool_ty())),
            K::DictKeys => fun(dict(var(0), var(1)), list(var(0))),
            K::DictValues => fun(dict(var(0), var(1)), list(var(1))),
            K::DictToList => fun(dict(var(0), var(1)), list(tuple2(var(0), var(1)))),
            K::DictFromList => fun(list(tuple2(var(0), var(1))), dict(var(0), var(1))),
            K::DictMap => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dict(var(0), var(1)), dict(var(0), var(2))),
            ),
            K::DictFoldl => fun(
                fun(var(0), fun(var(1), fun(var(2), var(2)))),
                fun(var(2), fun(dict(var(0), var(1)), var(2))),
            ),
            K::DictUnion => fun(
                dict(var(0), var(1)),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),
            // singleton : k -> v -> Dict k v
            K::DictSingleton => fun(var(0), fun(var(1), dict(var(0), var(1)))),
            // foldr : (k -> v -> a -> a) -> a -> Dict k v -> a
            K::DictFoldr => fun(
                fun(var(0), fun(var(1), fun(var(2), var(2)))),
                fun(var(2), fun(dict(var(0), var(1)), var(2))),
            ),
            // filter : (k -> v -> Bool) -> Dict k v -> Dict k v
            K::DictFilter => fun(
                fun(var(0), fun(var(1), bool_ty())),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),
            // partition : (k -> v -> Bool) -> Dict k v -> (Dict k v, Dict k v)
            K::DictPartition => fun(
                fun(var(0), fun(var(1), bool_ty())),
                fun(
                    dict(var(0), var(1)),
                    tuple2(dict(var(0), var(1)), dict(var(0), var(1))),
                ),
            ),
            // intersect / diff : Dict k v -> Dict k v -> Dict k v
            K::DictIntersect | K::DictDiff => fun(
                dict(var(0), var(1)),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),
            // update : k -> (Maybe v -> Maybe v) -> Dict k v -> Dict k v
            K::DictUpdate => fun(
                var(0),
                fun(
                    fun(maybe(var(1)), maybe(var(1))),
                    fun(dict(var(0), var(1)), dict(var(0), var(1))),
                ),
            ),

            // ── Ipe.Ui layout / element / event (already schemed in kernel_ty) ──
            K::UiLayout => fun(list(attr(var(0))), fun(elem_t(var(0)), html_t(var(0)))),
            K::UiLayoutWith => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.lw_wrapper_attrs, list(attr(var(0))));
                    m.insert(self.builtins.lw_root_attrs, list(attr(var(0))));
                    m
                }, RowTail::Closed);
                fun(cfg_rec, fun(elem_t(var(0)), html_t(var(0))))
            }
            // `Ui.node : Description -> List (Attribute msg) -> List (Element msg) -> Element msg`
            // — the container-element primitive backing `el`/`row`/`column`/
            // `wrappedRow`/`grid` in `Ipe/Ui.ipe`.
            K::UiNode => fun(
                description(),
                fun(list(attr(var(0))), fun(list(elem_t(var(0))), elem_t(var(0)))),
            ),
            // `Ui.taggedNode : String -> Description -> List (Attribute msg) -> List (Element msg) -> Element msg`
            // — the tagged-element primitive backing `paragraph`/`textColumn`/
            // `form`/`input`.
            K::UiTaggedNode => fun(
                string(),
                fun(
                    description(),
                    fun(list(attr(var(0))), fun(list(elem_t(var(0))), elem_t(var(0)))),
                ),
            ),
            // ── Ipe.Ui nearby attribute builders ─────────────────────────────────
            // `Ui.above/below/onLeft/onRight/inFront/behind : Element msg -> Attribute msg`
            K::UiAbove
            | K::UiBelow
            | K::UiOnLeft
            | K::UiOnRight
            | K::UiInFront
            | K::UiBehind => fun(elem_t(var(0)), attr(var(0))),
            K::UiButton => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.btn_f_on_press, maybe(var(0)));
                    m.insert(self.builtins.btn_f_label, elem_t(var(0)));
                    m
                }, RowTail::Closed);
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            K::UiOnClick | K::UiOnFocus | K::UiOnBlur | K::UiOnMouseOver | K::UiOnMouseOut => {
                fun(var(0), attr(var(0)))
            }
            K::UiOnInput | K::UiOnChange | K::UiOnKeyDown | K::UiOnKeyUp | K::UiOnFile => {
                fun(fun(string(), var(0)), attr(var(0)))
            }
            K::UiOnBool => fun(fun(bool_ty(), var(0)), attr(var(0))),
            // `Ui.onSubmit : (a -> msg) -> Attribute msg`
            // var(1) = the form-data record type (decoupled from var(0) = msg)
            K::UiOnSubmit => fun(fun(var(1), var(0)), attr(var(0))),

            // ── Ipe.Html.Events builders — produce `Ipe.Html.Attribute
            // msg` (`html_attr`), matching the `Ipe.Html.Attributes` builders
            // and the element builders' `List (html_attr msg)` slot. The arg
            // shape is dictated by `html_event_shape`; the `Raw` (onSubmit) form
            // DECOUPLES the handler type (`var(1)`) from `msg` (`var(0)`) so a
            // form handler `LoginForm -> Msg` does not leak into the surrounding
            // `Html msg` — exactly as the `.ipe` `onSubmit : a -> Attribute msg`.
            K::HtmlOnClick
            | K::HtmlOnFocus
            | K::HtmlOnBlur
            | K::HtmlOnMouseOver
            | K::HtmlOnMouseOut
            | K::HtmlOnSubmit
            | K::HtmlOnInput
            | K::HtmlOnChange
            | K::HtmlOnKeyDown
            | K::HtmlOnKeyUp
            | K::HtmlOnBool => match k.html_event_shape()? {
                ipe_kernels::HtmlEventShape::Msg => fun(var(0), html_attr(var(0))),
                ipe_kernels::HtmlEventShape::String => {
                    fun(fun(string(), var(0)), html_attr(var(0)))
                }
                ipe_kernels::HtmlEventShape::Bool => fun(fun(bool_ty(), var(0)), html_attr(var(0))),
                // `onSubmit : a -> Attribute msg` — the handler `var(1)` stays
                // an unconstrained HM var here (Ipê-level polymorphism only —
                // see `html.rs`'s `Event::OnForm` for the runtime-typed
                // construction, not `Event::OnRaw`, which no longer exists).
                ipe_kernels::HtmlEventShape::Raw => fun(var(1), html_attr(var(0))),
            },

            // ── Ipe.Web app-entry (open 6-field scheme) ──
            //
            // Mirrors `../ipe/src/Ipe/Type/Constrain/Expression.hs:2674-2695`.
            // The cfg record is OPEN (row variable `var(3)` = `appExt`) so the
            // user can supply optional extra fields (`head`, `consoleAuth`,
            // `guard`, `status`, `auth`, …) without the type checker rejecting
            // them as unknown extras.  The six named fields (indices 0-5) are
            // the REQUIRED fields; the row variable absorbs all additional ones.
            //
            // var index mapping:
            //   var(0) = model      var(1) = msg
            //   var(2) = page       var(3) = appExt (open row tail)
            //
            // `routes : List WebRoute` and `notFound : page` are required fields
            // even for non-routed apps (they default to `[]` / `CounterPage`).
            // The emit stage branches on Model.page at code-gen time
            // (emit_web.rs T5) — not at type time.
            //
            // Removes #[allow(dead_code)] from `live_f_routes` / `live_f_not_found`.
            K::WebApp => {
                // `view : Model -> Element Msg`; the framework applies
                // `Ui.layout` internally, unifying the graphical shapes on
                // `Element`. Raw HTML is reached through the `Ui.html` node
                // inside this single `Element` view.
                let view_ret = elem_t(var(1));
                let init_ret = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(web_req(), init_ret.clone()));
                        m.insert(
                            self.builtins.live_f_update,
                            fun(var(1), fun(var(0), init_ret)),
                        );
                        m.insert(self.builtins.live_f_view, fun(var(0), view_ret));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        // routes : List (WebRoute page)  — page = var(2).
                        // Parametrising WebRoute on the page type variable
                        // connects each route ctor's page type to `notFound`'s
                        // page type through the SAME var(2), so a type mismatch
                        // between them is caught here (IPE-T0001) instead of
                        // passing ipe and failing later in cargo (E0308).
                        m.insert(self.builtins.live_f_routes, list(live_route(var(2))));
                        // notFound : page
                        m.insert(self.builtins.live_f_not_found, var(2));
                        m
                    },
                    // Open row tail — var(3) absorbs optional extra fields.
                    RowTail::Open(3),
                );
                fun(cfg_rec, task_unit())
            }
            // `Web.route : String -> builder -> WebRoute page`
            // with builder = var(1) DISTINCT from page = var(0).
            //
            // The second argument is either a nullary page VALUE
            // (`route "/" HomePage` — builder : Page) or a params-consuming
            // page CONSTRUCTOR (`route "/apps/:slug" AppPage` — builder :
            // String -> Page; multi-`:param` routes curry further).  Sharing
            // ONE variable for both (the pre-round-4 `fun(var(0),
            // live_route(var(0)))` shape) forced `Page ≟ String -> Page` on
            // every param route — a false IPE-T0001 on the CANONICAL corpus
            // shape (`route "/apps/:slug" AppDetailPage`).
            //
            // Instead the builder var is related to the page var by a deferred
            // per-route witness ([`RouteWitnessCheck`], pushed in the
            // `constrain_kernel` special-case below and discharged by
            // `crate::resolve_route_witness_checks` after the main solve):
            // peel the builder's settled leading arrows, then unify the result
            // with `page`.  A nullary route witnesses `page` directly; a param
            // ctor witnesses it with its RESULT type; a wrong-ADT ctor still
            // fails unification → IPE-T0001.
            //
            // The result `WebRoute page` places every route of a list in
            // `List (WebRoute var(2))` (K::WebApp scheme), so all routes AND
            // `notFound : var(2)` share one page variable.  The page arg is no
            // longer phantom at the IR level: the lowerer threads it into
            // `IrType::WebRoute(page)` and the backend renders `Route<Page>`.
            K::WebRoute => fun(string(), fun(var(1), live_route(var(0)))),
            K::WebRenderStatic => fun(fun(var(0), html_t(var(1))), fun(var(0), task_unit())),

            // ── Ipe.Terminal full-screen app-entry (`appScreen`) ────────────────
            //
            // `view : Model -> Element Msg`, driven by `onKey`. `onKey` is
            // REQUIRED because the runtime's `tui_app_ui` entry takes a concrete
            // `FOnKey: Fn(String, String) -> Msg` bound (no `Option` form), so a
            // `Msg` cannot be fabricated when the handler is absent.
            //
            // Variable assignment:
            //   var(0) = model
            //   var(1) = msg
            //   var(3) = appExt     (open-row tail, absorbs guard/canvasWidth/…)
            //
            // `onKey`'s parameter is PINNED to the closed record
            // `{ kind : String, value : String }` (the KeyEvent shape): the
            // emitted handler must satisfy the runtime's
            // `FOnKey: Fn(String, String) -> Msg` bound, so an unconstrained
            // param would type-check yet break `cargo build` (E0593).
            K::TerminalAppScreen => {
                let key_event = Ty::Record(
                    {
                        let mut k = BTreeMap::new();
                        k.insert(self.builtins.tui_f_key_kind, string());
                        k.insert(self.builtins.tui_f_key_value, string());
                        k
                    },
                    RowTail::Closed,
                );
                let tup = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                        m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                        m.insert(self.builtins.live_f_view, fun(var(0), elem_t(var(1))));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        // onKey : { kind : String, value : String } -> msg (pinned).
                        m.insert(self.builtins.tui_f_on_key, fun(key_event, var(1)));
                        m
                    },
                    // Open row: absorbs optional fields (guard, canvasWidth, canvasHeight, …).
                    RowTail::Open(3),
                );
                fun(cfg_rec, task_unit())
            }

            // ── Ipe.Terminal line-oriented app-entry (`appLines`) ───────────────
            // `Terminal.appLines : { init : () -> (model, Cmd msg)
            //                      , update : msg -> model -> (model, Cmd msg)
            //                      , view : model -> String
            //                      , subscriptions : model -> Sub msg
            //                      , onLine : String -> msg
            //                      } -> Task () ()`
            K::TerminalAppLines => {
                let tup = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                        m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                        m.insert(self.builtins.live_f_view, fun(var(0), string()));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        m.insert(self.builtins.cli_f_on_line, fun(string(), var(1)));
                        m
                    },
                    // Closed cfg record — like `appScreen` / `WebView.app`, the
                    // line cfg takes exactly its named fields (the open
                    // row is a `Web.app`-only surface).
                    RowTail::Closed,
                );
                fun(cfg_rec, task_unit())
            }

            // ── Ipe.WebView app-entry (already schemed in kernel_ty) ──
            //
            // `view : Model -> Element Msg`; the framework applies `Ui.layout`,
            // the same unification as Web. Raw HTML is reached through the
            // `Ui.html` node inside this single `Element` view.
            K::WebViewApp => {
                let view_ret = elem_t(var(1));
                let tup = tuple2(var(0), cmd(var(1)));
                let window_ty = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.webview_f_title, string());
                        m.insert(self.builtins.webview_f_size, tuple2(int(), int()));
                        m
                    },
                    RowTail::Closed,
                );
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                        m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                        m.insert(self.builtins.live_f_view, fun(var(0), view_ret));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        m.insert(self.builtins.webview_f_window, window_ty);
                        m
                    },
                    RowTail::Closed,
                );
                fun(cfg_rec, task_unit())
            }

            // ══ FIRST-SCHEMED families ══
            // These have NO legacy scheme (`kernel_ty` → `Ty::Var(u32::MAX)`
            // hole); they get their scheme here, authored from the runtime
            // signature + `.ipe` HM signature. No parity oracle exists, so
            // correctness is pinned by `first_schemed_were_holes` (each is a
            // genuine hole) plus ipe→cargo build fixtures. Every arrow-count
            // equals `decl().arity` — the invariant
            // `eta_expand_partial` relies on when peeling `arity` arrows off the
            // inferred callee type.

            // ── String (33 — the kernels beyond `fromInt`/`fromFloat`) ──
            K::StringToInt => fun(string(), maybe(int())),
            K::StringToFloat => fun(string(), maybe(float())),
            K::StringFromList => fun(list(char()), string()),
            K::StringConcat => fun(list(string()), string()),
            K::StringWords | K::StringLines => fun(string(), list(string())),
            K::StringToList => fun(string(), list(char())),
            K::StringJoin => fun(string(), fun(list(string()), string())),
            K::StringSplit => fun(string(), fun(string(), list(string()))),
            // uncons : String -> Maybe (Char, String)
            K::StringUncons => fun(string(), maybe(tuple2(char(), string()))),
            // indexes : String -> String -> List Int
            K::StringIndexes => fun(string(), fun(string(), list(int()))),
            // foldl / foldr : (Char -> b -> b) -> b -> String -> b
            K::StringFoldl | K::StringFoldr => fun(
                fun(char(), fun(var(0), var(0))),
                fun(var(0), fun(string(), var(0))),
            ),

            // ── Crypto AEAD / Result-returning arms (the monomorphic hash /
            //    HMAC / verify kernels carry a shape; these keep a table arm for
            //    their `Result`/`Task` return). AEAD `decl().arity` is 2 (a fresh
            //    random nonce is prepended internally by the runtime). ──
            //    registry `decl().arity` was corrected 3→2 to match the Rust
            //    runtime (`ipe_aes_gcm_encrypt(key, plaintext)` — a fresh random
            //    nonce is prepended internally, so no third arg). Both take
            //    `key -> plaintext/ciphertext -> Result Error String`. ──
            K::CryptoRsaSha256Sign
            | K::CryptoAesGcmEncrypt
            | K::CryptoAesGcmDecrypt
            | K::CryptoChacha20Encrypt
            | K::CryptoChacha20Decrypt => {
                fun(string(), fun(string(), result(error_ty(), string())))
            }
            K::CryptoRandomBytes | K::CryptoRandomToken => fun(int(), task(string())),

            // ── Jwt (4) — `secret -> token/claims -> Result Error String`.
            //    Decode returns the decoded claims JSON as a String; encode
            //    (`ipe_jwt_encode_hs256(secret, claims_json)`) takes the secret/
            //    key and a claims-JSON String and returns the signed token — the
            //    registry `decl().arity` was corrected 3→2 to match. ──
            K::JwtDecodeHs256 | K::JwtDecodeRs256 | K::JwtEncodeHs256 | K::JwtEncodeRs256 => {
                fun(string(), fun(string(), result(error_ty(), string())))
            }

            // ── Jwt builder API (D-00) ──────────────────────────────────
            // `Jwt.claims : Claims` — nullary: returns an empty claims object.
            K::JwtClaims => claims_ty(),
            // `Jwt.hs256 : String -> Algorithm`
            // `Jwt.rs256 : String -> Algorithm`
            K::JwtHs256 | K::JwtRs256 => fun(string(), algorithm_ty()),
            // `Jwt.subject / .issuer / .audience / .jwtId : String -> Claims -> Claims`
            K::JwtSubject | K::JwtIssuer | K::JwtAudience | K::JwtJwtId => {
                fun(string(), fun(claims_ty(), claims_ty()))
            }
            // `Jwt.expiresAt / .notBefore / .issuedAt : Int -> Claims -> Claims`
            K::JwtExpiresAt | K::JwtNotBefore | K::JwtIssuedAt => {
                fun(int(), fun(claims_ty(), claims_ty()))
            }
            // `Jwt.withClaim : String -> JsonEnc.Value -> Claims -> Claims`
            // Matches the reference `Ipê/Core/Jwt.ipe:79` — the value is any
            // encoded JSON node (`JsonEnc.string`/`.int`/`.object`/…), so an
            // `Int`/`Bool`/nested-object custom claim round-trips with the right
            // token bytes. Both `Value` and `Claims` are `serde_json::Value` at
            // runtime, so the runtime insert is a direct move.
            K::JwtWithClaim => fun(string(), fun(value(), fun(claims_ty(), claims_ty()))),
            // `Jwt.encode : Algorithm -> Claims -> Result Error String`
            K::JwtEncode => fun(algorithm_ty(), fun(claims_ty(), result(error_ty(), string()))),
            // `Jwt.decode : Algorithm -> Int -> String -> Result Error String`
            K::JwtDecode => fun(algorithm_ty(), fun(int(), fun(string(), result(error_ty(), string())))),

            // ── Encoding decoders — `String -> Result Error String` (decoded
            //    bytes must be valid UTF-8 — non-UTF-8 payloads surface as `Err`;
            //    raw bytes go through `Ipe.Bytes`). The `String -> String`
            //    encoders carry a shape and resolve via `resolve_scheme`. ──
            K::EncodingBase64Decode | K::EncodingUrlDecode | K::EncodingHexDecode => {
                fun(string(), result(error_ty(), string()))
            }

            // ── Ipe.Html / Ipe.Ui / Ipe.Web rendering (42) ──
            // The Html/Ui/Background/Border/Font rendering family. `attr(m)` /
            // `elem_t(m)` / `html_t(m)` are the msg-polymorphic opaque cons;
            // `length()` / `color()` are the nullary value cons. Each is a
            // genuine `Ty::Var(u32::MAX)` hole (legacy `kernel_ty` has no Html/
            // Ui/Background/Border/Font arm), so all land in FIRST_SCHEMED.
            // Verified vs runtime fn params + lower `callee_arity` per
            // docs/adr/0020-html-ui-live-kernel-arity-tripwire.md. `Web.appRouted`
            // is EXCLUDED (REACHABLE_BUT_UNLOWERED) — its lowering is
            // `Feature::RoutedWebApp` unsupported, so a caller fails closed.

            // Ipe.Html serialise / escape (arity 1).
            K::HtmlRender => fun(html_t(var(0)), string()),
            K::HtmlAttrToString => fun(html_attr(var(0)), string()),

            // Ipe.Ui element builders (arity 0 / 1).
            K::UiNone => elem_t(var(0)),
            K::UiText => fun(string(), elem_t(var(0))),
            K::UiHtml => fun(html_t(var(0)), elem_t(var(0))),
            K::UiCells => fun(list(list(char())), elem_t(var(0))),

            // Ipe.Ui / Font attribute builders — nullary (arity 0).
            K::UiCenterX
            | K::UiCenterY
            | K::UiAlignLeft
            | K::UiAlignRight
            | K::UiAlignTop
            | K::UiAlignBottom
            | K::UiPointer
            | K::UiClip
            | K::UiClipX
            | K::UiClipY
            | K::UiScrollbars
            | K::UiScrollbarX
            | K::UiScrollbarY
            | K::FontBold
            | K::FontItalic
            // Tier 1 — nullary Attr
            | K::UiSquare
            | K::UiWidescreen
            | K::UiCinemascope
            | K::BorderSolid
            | K::BorderDashed
            | K::BorderDotted
            | K::FontSemiBold
            | K::FontRegular
            | K::FontLight
            | K::FontExtraBold
            | K::FontBlack
            | K::FontUnderline
            | K::FontNoDecoration
            | K::FontLineThrough
            | K::FontAlignLeft
            | K::FontAlignRight
            | K::FontAlignCenter
            | K::FontCenter
            | K::FontJustify => attr(var(0)),

            // Attribute builders — single Int arg.
            K::UiSpacing
            | K::UiPadding
            | K::UiGridColumns
            | K::BorderWidth
            | K::BorderRounded
            | K::FontSize
            // Tier 1 — Int → Attr
            | K::FontWeight
            | K::FontHoverSize
            | K::BorderHoverWidth
            | K::BorderHoverRounded => fun(int(), attr(var(0))),

            // Attribute builders — single Float arg.
            K::FontLetterSpacing | K::FontWordSpacing | K::UiAspectRatio => {
                fun(float(), attr(var(0)))
            }

            // Attribute builders — Length arg.
            K::UiWidth | K::UiHeight => fun(length(), attr(var(0))),

            // Attribute builders — Color arg.
            K::BackgroundColor
            | K::BorderColor
            | K::FontColor
            // Tier 1 — Color pseudo-class attrs
            | K::BackgroundHoverColor
            | K::BackgroundFocusColor
            | K::BackgroundActiveColor
            | K::BackgroundDisabledColor
            | K::BorderHoverColor
            | K::BorderFocusColor
            | K::BorderActiveColor
            | K::FontHoverColor
            | K::FontFocusColor
            | K::FontActiveColor
            | K::FontDisabledColor => fun(color(), attr(var(0))),

            // Attribute builders — String arg.
            K::BackgroundImage => fun(string(), attr(var(0))),
            K::FontFamily => fun(string(), attr(var(0))),

            // ── Background.linearGradient ────────────────────────────────────────
            // linearGradient : Float -> List (Float, Color) -> Attribute msg
            K::BackgroundLinearGradient => fun(
                float(),
                fun(list(Ty::Tuple(vec![float(), color()])), attr(var(0))),
            ),

            // Ipe.Ui — two Int args (arity 2).
            K::UiPaddingXY => fun(int(), fun(int(), attr(var(0)))),

            // ── Ui.paddingEach ──────────────────────────────────────────────────
            // paddingEach : { top : Int, right : Int, bottom : Int, left : Int }
            //             -> Attribute msg  (same record shape/symbols as
            // Border.widthEach — the `*Each` family shares field names).
            K::UiPaddingEach => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.edge_f_top, int());
                    m.insert(self.builtins.edge_f_right, int());
                    m.insert(self.builtins.edge_f_bottom, int());
                    m.insert(self.builtins.edge_f_left, int());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // Tier 1 — two-arg attrs.
            K::UiAspectRatioWH => fun(int(), fun(int(), attr(var(0)))),
            K::UiHtmlAttribute => fun(string(), fun(string(), attr(var(0)))),
            K::UiName => fun(string(), attr(var(0))),
            K::UiStyle => fun(string(), fun(string(), attr(var(0)))),
            // `Ui.transition : String -> Bool -> Attribute msg` — the CSS
            // transition shorthand + a respect-`prefers-reduced-motion` flag.
            // Native surface backing `Ipe.Ui.Transition.attribute` /
            // `attributeUnsafe`.
            K::UiTransitionRaw => fun(string(), fun(bool_ty(), attr(var(0)))),
            // `Ui.gridTracks : String -> String -> Attribute msg` — CSS
            // grid-template-columns (first arg) and grid-template-rows (second arg).
            // Native surface backing `Ipe.Ui.Grid.columns`/`rows`/`tracks`.
            K::UiGridTracksRaw => fun(string(), fun(string(), attr(var(0)))),
            // `Ui.animate : String -> String -> String -> Bool -> Attribute msg`
            // — keyframe-animation name, the animation shorthand tail
            // (`<dur>ms <easing> <delay>ms <iter> <fill>`), the `@keyframes`
            // body, and a respect-`prefers-reduced-motion` flag. Native surface
            // backing `Ipe.Ui.Animation.attribute`.
            K::UiAnimateRaw => fun(
                string(),
                fun(string(), fun(string(), fun(bool_ty(), attr(var(0))))),
            ),

            // Ui.breakpoint + Breakpoint constants.
            //
            // Sanctioned divergence from Ipê Go: `Breakpoint` is typed as
            // `String` in the Rust port rather than as a distinct opaque type
            // (see `docs/divergences-from-sky.md` §B-Breakpoint).  Users cannot
            // fabricate arbitrary `Breakpoint` values because all constructors
            // (`mobile`, `tablet`, …) are kernels whose schemes return `string()`;
            // the only type-safety gap vs. the Go backend is that a plain `String`
            // literal would also unify — an accepted limitation.
            //
            // `Ui.breakpoint : String -> List (Attribute msg) -> Element msg -> Element msg`
            // `Ui.mediaQuery : String -> List (Attribute msg) -> Element msg -> Element msg`
            // (same shape — `breakpoint` delegates to `mediaQuery` at runtime;
            // `mediaQuery` is the raw-query escape hatch.)
            K::UiBreakpoint | K::UiMediaQuery => fun(
                string(),
                fun(list(attr(var(0))), fun(elem_t(var(0)), elem_t(var(0)))),
            ),

            // ── PseudoClass opaque constants + Ui.onPseudo ──────────────────
            // Typed-constant shortcuts — all return the opaque `PseudoClass` type
            // (mirrors `ipe_runtime::ui::element::PseudoClass`'s 5 constructors).
            K::UiHover | K::UiFocus | K::UiFocusVisible | K::UiActive | K::UiDisabled => {
                pseudo_class()
            }
            // `Ui.onPseudo : PseudoClass -> List (Attribute msg) -> Attribute msg`
            K::UiOnPseudo => fun(
                pseudo_class(),
                fun(list(attr(var(0))), attr(var(0))),
            ),

            // Ipe.Html leaf nodes (arity 1).
            K::HtmlTextNode | K::HtmlRawNode => fun(string(), html_t(var(0))),

            // Ipe.Html generic node (arity 3 — tag, attrs, children). Attrs are
            // `Ipe.Html.Attribute` (html_attr) — matches `Vec<html::Attribute>`.
            K::HtmlNode => fun(
                string(),
                fun(
                    list(html_attr(var(0))),
                    fun(list(html_t(var(0))), html_t(var(0))),
                ),
            ),

            // `Html.voidNode : String -> List Attr -> Html msg` — the generic
            // void counterpart of `Html.node` (arbitrary runtime tag, no
            // children arg). Routes through the same `html_node_` runtime sink
            // with an emit-baked empty children vec.
            K::HtmlVoidNode => fun(string(), fun(list(html_attr(var(0))), html_t(var(0)))),

            // `Html.doctype : List Html -> Html msg` — wraps children in the
            // `!doctype-wrapper` pseudo-tag; `html::render_into_ctx` already
            // recognises it and emits the literal `<!DOCTYPE html>` prefix.
            K::HtmlDoctype => fun(list(html_t(var(0))), html_t(var(0))),

            // `Html.titleNode : String -> Html msg` — wraps a raw string
            // directly in `<title>`.
            K::HtmlTitleNode => fun(string(), html_t(var(0))),

            // `Html.toString : Html msg -> String` — alias of `Html.render`.
            K::HtmlToString => fun(html_t(var(0)), string()),

            // Ipe.Html styleNode (arity 2 — attrs, css string; F7). The
            // runtime bakes `strip_style_close` on the css. RELOCATED — matches
            // the legacy `kernel_ty(Html, styleNode)` byte-for-byte (html_attr +
            // html_t). `List (Ipe.Html.Attribute msg) -> String -> Html msg`.
            K::HtmlStyleNode => fun(list(html_attr(var(0))), fun(string(), html_t(var(0)))),

            // `Html.Unsafe.unsafeScript : String -> Html msg` — an inline
            // `<script>` with a verbatim JavaScript body (FIRST_SCHEMED, Ipê-new,
            // no legacy oracle). The runtime kernel neutralises a `</script`
            // breakout at construction.
            K::HtmlScriptNode => fun(string(), html_t(var(0))),

            // ── Ipe.Html.Attributes retained primitives ─────────────────
            // The fixed-key builders are pure Ipê in `Ipe/Html/Attributes.ipe`
            // over these three `Attribute`-value constructors.
            K::HtmlAttribute => fun(string(), fun(string(), html_attr(var(0)))),
            K::HtmlBoolAttribute => fun(string(), fun(bool_ty(), html_attr(var(0)))),
            K::HtmlNoAttr => html_attr(var(0)),

            // ── Ipe.Ui.Keyed ──────────────────────────────────────────────────
            // `Keyed.column / Keyed.row : List (Attribute msg) -> List (String, Element msg) -> Element msg`
            K::KeyedColumn
            | K::KeyedRow => {
                fun(
                    list(attr(var(0))),
                    fun(list(tuple2(string(), elem_t(var(0)))), elem_t(var(0))),
                )
            }

            // ── Ipe.Decimal ───────────────────────────────────────────────────
            // Construction.
            K::DecZero | K::DecOne | K::DecOneHundred => decimal(),
            K::DecFromString => fun(string(), result(error_ty(), decimal())),
            K::DecFromInt    => fun(int(),    decimal()),
            K::DecFromFloat  => fun(float(),  decimal()),
            K::DecFromMinor  => fun(int(), fun(int(), decimal())),
            // Conversion.
            K::DecToString       => fun(decimal(), string()),
            K::DecToStringFixed  => fun(int(), fun(decimal(), string())),
            K::DecToFloat        => fun(decimal(), float()),
            K::DecToInt          => fun(decimal(), int()),
            K::DecToMinor        => fun(int(), fun(decimal(), int())),
            // Arithmetic.
            K::DecAdd | K::DecSub | K::DecMul => {
                fun(decimal(), fun(decimal(), decimal()))
            }
            K::DecDiv | K::DecMod => {
                fun(decimal(), fun(decimal(), result(error_ty(), decimal())))
            }
            K::DecNeg | K::DecAbs | K::DecFloor | K::DecCeil => {
                fun(decimal(), decimal())
            }
            // Rounding.
            K::DecRound | K::DecRoundHalfUp | K::DecTruncate => {
                fun(int(), fun(decimal(), decimal()))
            }
            // Comparison.
            K::DecCompare => fun(decimal(), fun(decimal(), int())),
            K::DecEq
            | K::DecNeq
            | K::DecLt
            | K::DecLte
            | K::DecGt
            | K::DecGte => fun(decimal(), fun(decimal(), bool_ty())),
            K::DecMin | K::DecMax => fun(decimal(), fun(decimal(), decimal())),
            // Predicates.
            K::DecIsZero | K::DecIsPositive | K::DecIsNegative => {
                fun(decimal(), bool_ty())
            }
            // Percent helpers.
            K::DecPercentOf | K::DecAddPercent | K::DecSubPercent => {
                fun(decimal(), fun(decimal(), decimal()))
            }
            // Formatting.
            // `formatWith : String -> String -> Int -> Decimal -> String`
            K::DecFormatWith => {
                fun(string(), fun(string(), fun(int(), fun(decimal(), string()))))
            }

            // ── Ipe.Money ──────────────────────────────────────────────────────
            // Every kernel takes the currency's ISO 4217 code (a `String`); the
            // compiled-source `Ipe.Money` wrappers do the `Currency -> code`
            // conversion before the call. `Error` here is the runtime `IpeError`
            // channel (`error_ty()`), matching the `Result Error _` runtime sigs.
            K::MoneyFormat | K::MoneyFormatWithCode => {
                fun(string(), fun(decimal(), string()))
            }
            K::MoneyAllocate => {
                fun(int(), fun(int(), fun(decimal(), list(decimal()))))
            }
            K::MoneySetRate => fun(
                string(),
                fun(string(), fun(decimal(), result(error_ty(), Ty::Unit))),
            ),
            K::MoneyGetRate => {
                fun(string(), fun(string(), result(error_ty(), decimal())))
            }
            K::MoneyClearRates => fun(Ty::Unit, result(error_ty(), Ty::Unit)),

            // ── Ipe.Ui.Region ──────────────────────────────────────────
            // Nullary region landmark attrs — `Attribute msg`.
            K::RegionMainContent
            | K::RegionNavigation
            | K::RegionFooter
            | K::RegionAside
            | K::RegionAnnounce
            | K::RegionAnnounceUrgently => attr(var(0)),
            // Arity-1 region attrs.
            K::RegionHeading => fun(int(), attr(var(0))),
            K::RegionLabel => fun(string(), attr(var(0))),

            // ── Ui.describe + desc* constructors ──────────────────────────────
            // `Ui.describe : Description -> Attribute msg`
            K::UiDescribe => fun(description(), attr(var(0))),
            // Nullary `Description` constructors — return `Description`.
            // `descNone`/`descParagraph` back the `node`/`taggedNode` sugar in
            // `Ipe/Ui.ipe`; the rest are `Ui.describe` roles.
            K::UiDescNone
            | K::UiDescParagraph
            | K::UiDescMain
            | K::UiDescNavigation
            | K::UiDescContentInfo
            | K::UiDescComplementary
            | K::UiDescLivePolite
            | K::UiDescLiveAssertive => description(),
            // Arity-1 `Description` constructors.
            K::UiDescHeading => fun(int(), description()),
            K::UiDescLabel => fun(string(), description()),

            // ── Ipe.Ui.Input ──────────────────────────────────────────
            //
            // Label constructors: `List (Attribute msg) -> Element msg -> Label msg`
            K::InputLabelAbove | K::InputLabelBelow | K::InputLabelLeft | K::InputLabelRight => {
                fun(list(attr(var(0))), fun(elem_t(var(0)), label_t(var(0))))
            }
            // `Input.labelHidden : String -> Label msg`
            K::InputLabelHidden => fun(string(), label_t(var(0))),
            // `Input.placeholder : List (Attribute msg) -> Element msg -> Placeholder msg`
            K::InputPlaceholder => {
                fun(list(attr(var(0))), fun(elem_t(var(0)), placeholder_t(var(0))))
            }
            // `Input.text / email / username / search / currentPassword / newPassword`:
            //   List (Attribute msg)
            //   -> { onChange : String -> msg
            //      , text : String
            //      , placeholder : Maybe (Placeholder msg)
            //      , label : Label msg
            //      }
            //   -> Element msg
            K::InputText
            | K::InputEmail
            | K::InputUsername
            | K::InputSearch
            | K::InputCurrentPassword
            | K::InputNewPassword => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(self.builtins.input_f_text, string());
                        m.insert(
                            self.builtins.input_f_placeholder,
                            maybe(placeholder_t(var(0))),
                        );
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            // `Input.multiline`:
            //   List (Attribute msg)
            //   -> { onChange : String -> msg
            //      , text : String
            //      , placeholder : Maybe (Placeholder msg)
            //      , label : Label msg
            //      , spellcheck : Bool
            //      }
            //   -> Element msg
            K::InputMultiline => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(self.builtins.input_f_text, string());
                        m.insert(
                            self.builtins.input_f_placeholder,
                            maybe(placeholder_t(var(0))),
                        );
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m.insert(self.builtins.input_f_spellcheck, bool_ty());
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            // `Input.checkbox`:
            //   List (Attribute msg)
            //   -> { onChange : Bool -> msg
            //      , icon : Bool -> Element msg
            //      , checked : Bool
            //      , label : Label msg
            //      }
            //   -> Element msg
            K::InputCheckbox => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(bool_ty(), var(0)));
                        m.insert(self.builtins.input_f_icon, fun(bool_ty(), elem_t(var(0))));
                        m.insert(self.builtins.input_f_checked, bool_ty());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // `Input.slider`:
            //   List (Attribute msg)
            //   -> { onChange : String -> msg
            //      , value   : String
            //      , min     : String
            //      , max     : String
            //      , step    : String
            //      , label   : Label msg
            //      }
            //   -> Element msg
            //
            // All numeric values are passed as `String` (matching the DOM's
            // `<input type="range">` wire format); the user parses to a numeric
            // type as needed.
            K::InputSlider => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(self.builtins.input_f_value, string());
                        m.insert(self.builtins.input_f_min, string());
                        m.insert(self.builtins.input_f_max, string());
                        m.insert(self.builtins.input_f_step, string());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Ipe.Ui.Input radio group ───────────────────────────────
            //
            // `Input.option : String -> Element msg -> RadioOption msg`
            K::InputOption => fun(string(), fun(elem_t(var(0)), radio_option_t(var(0)))),
            //
            // `Input.radio : List (Attr msg) ->
            //   { onChange : String -> msg
            //   , options  : List (RadioOption msg)
            //   , selected : String
            //   , label    : Label msg
            //   } -> Element msg`
            K::InputRadio => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(
                            self.builtins.input_f_options,
                            list(radio_option_t(var(0))),
                        );
                        m.insert(self.builtins.input_f_selected, string());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            //
            // `Input.radioRow` — identical signature to `radio`.
            K::InputRadioRow => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(
                            self.builtins.input_f_options,
                            list(radio_option_t(var(0))),
                        );
                        m.insert(self.builtins.input_f_selected, string());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Ipe.Ui.Lazy ────────────────────────────────────────────
            // lazy  : (a -> Element msg) -> a -> Element msg
            K::LazyLazy => fun(
                fun(var(0), elem_t(var(1))),
                fun(var(0), elem_t(var(1))),
            ),
            // lazy2 : (a -> b -> Element msg) -> a -> b -> Element msg
            K::LazyLazy2 => fun(
                fun(var(0), fun(var(1), elem_t(var(2)))),
                fun(var(0), fun(var(1), elem_t(var(2)))),
            ),
            // lazy3 : (a -> b -> c -> Element msg) -> a -> b -> c -> Element msg
            K::LazyLazy3 => fun(
                fun(var(0), fun(var(1), fun(var(2), elem_t(var(3))))),
                fun(var(0), fun(var(1), fun(var(2), elem_t(var(3))))),
            ),
            // lazy4 : (a -> b -> c -> d -> Element msg) -> a -> b -> c -> d -> Element msg
            K::LazyLazy4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), elem_t(var(4)))))),
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), elem_t(var(4)))))),
            ),
            // lazy5 : (a -> b -> c -> d -> e -> Element msg) -> a -> b -> c -> d -> e -> Element msg
            K::LazyLazy5 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), fun(var(4), elem_t(var(5))))))),
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), fun(var(4), elem_t(var(5))))))),
            ),

            // ── Json.Decode (17) — mirrors the already-relocated `Db.Decode`
            //    shapes (function-first `map`/`andThen`; `dec(a)` is the opaque
            //    `Decoder a`). Primitives are arity-0 bare decoders. ──
            K::JsonDecString => dec(string()),
            K::JsonDecInt => dec(int()),
            K::JsonDecFloat => dec(float()),
            K::JsonDecBool => dec(bool_ty()),
            K::JsonDecValue => dec(value()),
            K::JsonDecDecodeString => fun(dec(var(0)), fun(string(), result(error_ty(), var(0)))),
            K::JsonDecDecodeValue => fun(dec(var(0)), fun(value(), result(error_ty(), var(0)))),
            K::JsonDecField => fun(string(), fun(dec(var(0)), dec(var(0)))),
            K::JsonDecAt => fun(list(string()), fun(dec(var(0)), dec(var(0)))),
            K::JsonDecIndex => fun(int(), fun(dec(var(0)), dec(var(0)))),
            K::JsonDecList => fun(dec(var(0)), dec(list(var(0)))),
            K::JsonDecMap => fun(fun(var(0), var(1)), fun(dec(var(0)), dec(var(1)))),
            K::JsonDecAndThen => fun(fun(var(0), dec(var(1))), fun(dec(var(0)), dec(var(1)))),
            K::JsonDecSucceed => fun(var(0), dec(var(0))),
            K::JsonDecFail => fun(string(), dec(var(0))),
            K::JsonDecOneOf => fun(list(dec(var(0))), dec(var(0))),
            K::JsonDecMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dec(var(0)), fun(dec(var(1)), dec(var(2)))),
            ),
            K::JsonDecMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(dec(var(0)), fun(dec(var(1)), fun(dec(var(2)), dec(var(3))))),
            ),
            K::JsonDecMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    dec(var(0)),
                    fun(dec(var(1)), fun(dec(var(2)), fun(dec(var(3)), dec(var(4))))),
                ),
            ),

            // ── Json.Decode.Pipeline (4) — mirrors `Db.Decode.required` /
            //    `optional`; `next_decoder : Decoder (a -> b)`. ──
            K::JsonDecPRequired => fun(
                string(),
                fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),
            ),
            K::JsonDecPRequiredAt => fun(
                list(string()),
                fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),
            ),
            K::JsonDecPOptional => fun(
                string(),
                fun(
                    dec(var(0)),
                    fun(var(0), fun(dec(fun(var(0), var(1))), dec(var(1)))),
                ),
            ),
            K::JsonDecPCustom => fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),

            // ── Ipe.Config (16) — the shared `Decoder a` carrier (`dec(a)`),
            //    over TOML/YAML/JSON. Combinator/primitive schemes are identical
            //    to `Json.Decode`'s (same runtime `decode_*` fns); the format
            //    front-ends put the source `String` FIRST, then the decoder. ──
            K::ConfigString => dec(string()),
            K::ConfigInt => dec(int()),
            K::ConfigFloat => dec(float()),
            K::ConfigBool => dec(bool_ty()),
            K::ConfigNullable => fun(dec(var(0)), dec(maybe(var(0)))),
            K::ConfigField => fun(string(), fun(dec(var(0)), dec(var(0)))),
            K::ConfigAt => fun(list(string()), fun(dec(var(0)), dec(var(0)))),
            K::ConfigList => fun(dec(var(0)), dec(list(var(0)))),
            K::ConfigSucceed => fun(var(0), dec(var(0))),
            K::ConfigFail => fun(string(), dec(var(0))),
            K::ConfigMap => fun(fun(var(0), var(1)), fun(dec(var(0)), dec(var(1)))),
            K::ConfigAndThen => fun(fun(var(0), dec(var(1))), fun(dec(var(0)), dec(var(1)))),
            // `Config.map2..8 : (a -> .. -> r) -> Decoder a -> .. -> Decoder r`.
            K::ConfigMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dec(var(0)), fun(dec(var(1)), dec(var(2)))),
            ),
            K::ConfigMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    dec(var(0)),
                    fun(dec(var(1)), fun(dec(var(2)), dec(var(3)))),
                ),
            ),
            K::ConfigMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(dec(var(2)), fun(dec(var(3)), dec(var(4)))),
                    ),
                ),
            ),
            K::ConfigMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(dec(var(3)), fun(dec(var(4)), dec(var(5)))),
                        ),
                    ),
                ),
            ),
            K::ConfigMap6 => fun(
                fun(
                    var(0),
                    fun(
                        var(1),
                        fun(var(2), fun(var(3), fun(var(4), fun(var(5), var(6))))),
                    ),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(
                                dec(var(3)),
                                fun(dec(var(4)), fun(dec(var(5)), dec(var(6)))),
                            ),
                        ),
                    ),
                ),
            ),
            K::ConfigMap7 => fun(
                fun(
                    var(0),
                    fun(
                        var(1),
                        fun(
                            var(2),
                            fun(var(3), fun(var(4), fun(var(5), fun(var(6), var(7))))),
                        ),
                    ),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(
                                dec(var(3)),
                                fun(
                                    dec(var(4)),
                                    fun(dec(var(5)), fun(dec(var(6)), dec(var(7)))),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            K::ConfigMap8 => fun(
                fun(
                    var(0),
                    fun(
                        var(1),
                        fun(
                            var(2),
                            fun(
                                var(3),
                                fun(var(4), fun(var(5), fun(var(6), fun(var(7), var(8))))),
                            ),
                        ),
                    ),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(
                                dec(var(3)),
                                fun(
                                    dec(var(4)),
                                    fun(
                                        dec(var(5)),
                                        fun(dec(var(6)), fun(dec(var(7)), dec(var(8)))),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            // `Config.oneOf : List (Decoder a) -> Decoder a`.
            K::ConfigOneOf => fun(list(dec(var(0))), dec(var(0))),
            // `Config.index : Int -> Decoder a -> Decoder a`.
            K::ConfigIndex => fun(int(), fun(dec(var(0)), dec(var(0)))),
            // `Config.keyValuePairs : Decoder a -> Decoder (List (String, a))`.
            K::ConfigKeyValuePairs => {
                fun(dec(var(0)), dec(list(Ty::Tuple(vec![string(), var(0)]))))
            }
            // `Config.maybe : Decoder a -> Decoder (Maybe a)`.
            K::ConfigMaybe => fun(dec(var(0)), dec(maybe(var(0)))),
            // `Config.dict : Decoder a -> Decoder (Dict String a)`.
            K::ConfigDict => fun(dec(var(0)), dec(dict(string(), var(0)))),
            K::ConfigDecodeToml => {
                fun(string(), fun(dec(var(0)), result(error_ty(), var(0))))
            }
            K::ConfigDecodeYaml => {
                fun(string(), fun(dec(var(0)), result(error_ty(), var(0))))
            }
            K::ConfigDecodeJson => {
                fun(string(), fun(dec(var(0)), result(error_ty(), var(0))))
            }
            K::ConfigLoadFromFile => fun(string(), fun(dec(var(0)), task(var(0)))),

            // ── Result (internal) — `okDefault : a -> Result e a`, the Ok-wrap
            //    used during lowering (runtime `ok_res(a) -> Result e a`). ──
            K::ResultOkDefault => fun(var(0), result(var(1), var(0))),

            // ── Ipe.Ui Length builders (result type `Length`) — runtime
            //    `ui_px_(i64) -> Length`, `ui_fill_() -> Length`, etc. `Length`
            //    lowers to `IrType::UiPlain(UiPlain::Length)`. Arrow-count ==
            //    `decl().arity` for every arm. ──
            K::UiPx | K::UiFillPortion | K::UiVh | K::UiVw => fun(int(), length()),
            K::UiFill | K::UiContent | K::UiShrink => length(),
            K::UiMinimum | K::UiMaximum => fun(int(), fun(length(), length())),

            // ── Ipe.Ui Color builders (result type `Color`) — runtime
            //    `ui_rgb_(i64,i64,i64) -> Color`, `ui_rgba_(i64,i64,i64,f64) ->
            //    Color`, `ui_white_() -> Color`, etc. `Color` lowers to
            //    `IrType::UiPlain(UiPlain::Color)`. ──
            K::UiRgb => fun(int(), fun(int(), fun(int(), color()))),
            K::UiRgba => fun(int(), fun(int(), fun(int(), fun(float(), color())))),
            K::UiWhite | K::UiBlack | K::UiTransparent => color(),
            // colorCss : Color -> String
            K::UiColorCss => fun(color(), string()),

            // ── Ipe.Json.Encode (8) — the `JsonEnc.*` encoders. `Value =
            //    any` maps to `IrType::Json` (`JsonVal`) via the `"Value"` arm in
            //    `ipe_lower::ir_type_from_ty`. Runtime: `json_enc_string(String)
            //    -> JsonVal`, `json_enc_null() -> JsonVal` (arity 0),
            //    `json_enc_list(impl Fn(A) -> JsonVal, Vec<A>) -> JsonVal`,
            //    `json_enc_object(Vec<(String, JsonVal)>) -> JsonVal`,
            //    `json_enc_encode(i64, JsonVal) -> String`. Scheming these closes
            //    the former `Ty::Var(u32::MAX)` exit-0 hole (the lowerer's
            //    hardcoded `kernel_native_ir_type` fallback stays as a safety
            //    net for bare-value references). ──
            K::JsonEncString => fun(string(), value()),
            K::JsonEncInt => fun(int(), value()),
            K::JsonEncFloat => fun(float(), value()),
            K::JsonEncBool => fun(bool_ty(), value()),
            K::JsonEncNull => value(),
            K::JsonEncList => fun(fun(var(0), value()), fun(list(var(0)), value())),
            K::JsonEncObject => fun(list(tuple2(string(), value())), value()),
            K::JsonEncEncode => fun(int(), fun(value(), string())),

            // ── Ipe.Error (15 — real Error/ErrorKind/ErrorDetails ADT) ──
            //    `Error` is `Error ErrorKind ErrorInfo` (`Error`'s own ctor scheme
            //    is registered in `ctor_schemes()`), backed at runtime by the real
            //    `ipe_runtime::error::IpeError` enum (`IrType::Error`), not
            //    string-backed. The eight message constructors are `String ->
            //    Error` (each classifies its `ErrorKind` at construction);
            //    `timeout`/`notFound`/`permissionDenied` are nullary `Error`;
            //    `toString : Error -> String` routes through the shared
            //    Stringify-bounded mechanism (see the `BasicsToString |
            //    ErrorToString` special case above — this scheme arm is a
            //    shadowed fallback, never actually reached); `withMessage :
            //    String -> Error -> Error`; `isRetryable : Error -> Bool`
            //    classifies on kind alone; `withDetails : ErrorDetails -> Error ->
            //    Error` attaches the `ErrorDetails` union
            //    to `ErrorInfo.details : Maybe ErrorDetails` (`ErrorDetails`'s own
            //    5-variant ctor scheme is registered in `ctor_schemes()`).
            K::ErrorUnexpected
            | K::ErrorInvalidInput
            | K::ErrorIo
            | K::ErrorNetwork
            | K::ErrorFfi
            | K::ErrorDecode
            | K::ErrorConflict
            | K::ErrorUnavailable => fun(string(), error_ty()),
            K::ErrorTimeout | K::ErrorNotFound | K::ErrorPermissionDenied => error_ty(),
            K::ErrorToString => fun(var(0), string()),
            K::ErrorWithMessage => fun(string(), fun(error_ty(), error_ty())),
            K::ErrorIsRetryable => fun(error_ty(), bool_ty()),
            K::ErrorWithDetails => fun(errordetails_ty(), fun(error_ty(), error_ty())),
            //    Inspectors: `kind : Error -> ErrorKind` and `message : Error ->
            //    String` destructure a live error; `kindName : ErrorKind ->
            //    String` renders a kind's stable label (the same label
            //    `Error.toString` prefixes with).
            K::ErrorKind => fun(error_ty(), errorkind_ty()),
            K::ErrorMessage => fun(error_ty(), string()),
            K::ErrorKindName => fun(errorkind_ty(), string()),

            // ── Ipe.CssSafety (4 — Ipe.Css leaf security kernels) ──
            //    The three parsers are `String -> Maybe String` (`None` => the
            //    Ipê side drops the declaration/rule via `CssDropped` /
            //    `CssRuleDropped`); `stripStyleClose` is the `String -> String`
            //    breakout floor. Runtime `safe_value` / `safe_prop_name` /
            //    `safe_selector` return `IpeMaybe<String>` (mirrors `uuid_parse`);
            //    `strip_style_close_kernel` returns `String`.
            K::CssSafetySafeValue
            | K::CssSafetySafePropName
            | K::CssSafetySafeSelector
            | K::CssSafetySanitizeRawBody => fun(string(), maybe(string())),

            // ── Ipe.Uuid (3) — ENTROPY IS AN EFFECT ──
            //    `v4`/`v7` draw fresh entropy per call, so they are typed on the
            //    effect tier `() -> Task Error String` (runtime `uuid_v4::<E>(_:
            //    ())` / `uuid_v7::<E>(_: ())` return `IpeTask<E, String>`),
            //    called `Uuid.v4 ()` exactly like `Time.now ()`. This makes
            //    "entropy typed as a memoizable pure `String`" unrepresentable —
            //    a pure `String` is CSE/memoization-eligible, so two references
            //    could collapse to one shared value (the soundness lie the Go
            //    backend still carries via bare `Uuid.v4 : String`). `parse`
            //    stays PURE (`String -> Maybe String`): it inspects an existing
            //    string with no entropy — a genuine parser, NOT the arity-0
            //    codegen artifact.
            K::UuidV4 | K::UuidV7 => fun(Ty::Unit, task(string())),
            K::UuidParse => fun(string(), maybe(string())),

            // EXCLUDED — the ONLY kernels without a scheme. This is an
            // EXPLICIT wildcard-free arm, so F1 is structurally
            // unrepresentable here: a future `StdlibKernel` variant fails to
            // compile in `ipe_types` until it is either schemed above or added to
            // one of the two exclusion buckets below).
            //
            //  * `Web.appRouted` — REACHABLE_BUT_UNLOWERED: has a runtime fn +
            //    qualifier, but its lowering is `Feature::RoutedWebApp`
            //    unsupported and its type is a closed record, not a curried `Ty`.
            //    A caller fails closed at type-check until routed lowering lands.
            //
            // Gate-checked (`known_unbacked_never_schemed`,
            // `stdlib_scheme_total_over_reachable`, the REACHABLE_BUT_UNLOWERED
            // disjointness guard). Do NOT add a bare `_` back — it reopens F1.
            //
            //  * `Sub.subscribeTopic` / `Cmd.publish` / `Cmd.publishNoEcho` /
            //    `PubSub.publish` / `PubSub.publishNoEcho` are wired and have
            //    their schemes above; not in this arm.

            // ── Shape-carrying monomorphic families ──
            // Every kernel below carries a structural `TyShape`
            // (`StdlibKernel::scheme_shape`), so `resolve_scheme` types it by
            // interpreting that shape and never consults this table — its scheme
            // lives once, on the descriptor, not as an arm here. Each resolves to
            // `Some` through `resolve_scheme`; the byte-identity of every
            // interpreted shape is pinned by `interpreted_shape_matches_legacy`.
            // The explicit `return None` keeps this match wildcard-free (a new
            // variant must still be classified) while the shape stays the SSOT.
            K::BasicsNot | K::BasicsSqrt | K::BitwiseAnd | K::BitwiseComplement |
            K::BitwiseOr | K::BitwiseShiftLeftBy | K::BitwiseShiftRightBy | K::BitwiseShiftRightZfBy |
            K::BitwiseXor | K::BytesAppend | K::BytesEmpty | K::BytesFromString |
            K::BytesIsEmpty | K::BytesLength | K::BytesSlice | K::BytesToBase64 |
            K::BytesToHex | K::CharFromCode | K::CharIsAlpha | K::CharIsAlphaNum |
            K::CharIsDigit | K::CharIsHexDigit | K::CharIsLower | K::CharIsOctDigit |
            K::CharIsUpper | K::CharToCode | K::CharToLower | K::CharToUpper |
            K::CryptoAesKeyFromPassword | K::CryptoChachaKeyFromPassword | K::CryptoConstantTimeEqual | K::CryptoHmacSha256 |
            K::CryptoHmacSha512 | K::CryptoMd5 | K::CryptoRsaSha256Verify | K::CryptoSha1 |
            K::CryptoSha256 | K::CryptoSha512 | K::CssSafetyStripStyleClose | K::EncodingBase64Encode |
            K::EncodingHexEncode | K::EncodingUrlEncode | K::FontMonospace | K::FontSansSerif |
            K::FontSerif | K::HtmlEscapeAttr | K::HtmlEscapeText | K::MathAbs |
            K::MathAcos | K::MathAcosh | K::MathAsin | K::MathAsinh |
            K::MathAtan | K::MathAtan2 | K::MathAtanh | K::MathCbrt |
            K::MathCeil | K::MathCos | K::MathCosh | K::MathE |
            K::MathExp | K::MathExp2 | K::MathFloor | K::MathHypot |
            K::MathInf | K::MathIsNaN | K::MathLog | K::MathLog10 |
            K::MathLog2 | K::MathMod | K::MathNan | K::MathPhi |
            K::MathPi | K::MathPow | K::MathRemainder | K::MathRound |
            K::MathSin | K::MathSinh | K::MathSqrt | K::MathSqrt2 |
            K::MathTan | K::MathTanh | K::MathTrunc | K::MoneyCurrencyName |
            K::MoneyHasRate | K::MoneyIsKnownCurrency | K::MoneyMinorUnits | K::MoneySymbol |
            K::RateLimitAllow | K::StringAll | K::StringAny | K::StringAppend |
            K::StringCasefold | K::StringCons | K::StringContains | K::StringContainsIn |
            K::StringDropLeft | K::StringDropRight | K::StringEndsWith | K::StringEndsWithIn |
            K::StringEqualFold | K::StringFilter | K::StringFromChar | K::StringFromFloat |
            K::StringFromInt | K::StringIsEmail | K::StringIsEmpty | K::StringIsUrl |
            K::StringLeft | K::StringLength | K::StringMap | K::StringPad |
            K::StringPadLeft | K::StringPadRight | K::StringRepeat | K::StringReplace |
            K::StringReverse | K::StringRight | K::StringSlice | K::StringStartsWith |
            K::StringStartsWithIn | K::StringToLower | K::StringToUpper | K::StringTrim |
            K::StringTrimEnd | K::StringTrimStart | K::SystemGetenvOr | K::TimeDaysInMonth |
            K::TimeIsLeapYear | K::TimeTimeString | K::UiDarkMode | K::UiDesktop |
            K::UiLightMode | K::UiMobile | K::UiReducedMotion | K::UiTablet => return None,

            K::WebAppRouted => return None,

            // ── Ipe.Auth (9 kernels) ──────────────────────────────────────
            // hashPassword : String -> Result Error String
            K::AuthHashPassword => fun(string(), result(error_ty(), string())),
            // hashPasswordCost : String -> Int -> Result Error String
            K::AuthHashPasswordCost => fun(string(), fun(int(), result(error_ty(), string()))),
            // verifyPassword : String -> String -> Result Error Bool
            K::AuthVerifyPassword => fun(string(), fun(string(), result(error_ty(), bool_ty()))),
            // passwordStrength : String -> Result Error String
            K::AuthPasswordStrength => fun(string(), result(error_ty(), string())),
            // signToken : Secret -> Dict String String -> Int -> Result Error String
            // AUD-06 (seal): a flex `claims` `var(0)` would unify with ANY
            // type, so ipe would accept a record/Int/whatever as claims while
            // the emitted wrapper is pinned to `HashMap<String,String>`
            // (project.rs AUTH_WRAPPERS + runtime/auth.rs), no coercion at
            // lowering → cargo fail on any non-Dict claims (exit-0-then-cargo-
            // fail). Pinned concrete per the concrete-over-generic rule — this
            // was never genuine polymorphism, just an unpinned wildcard.
            // Diverges from Go's polymorphic `a`; see divergences-from-sky.md.
            //
            // the signing secret is `Secret`, not `String` — "secrets
            // are typed" (PRINCIPLES.md). Re-typed in the same change as `Secret`
            // itself; zero migration cost (no fixture calls this kernel yet).
            // `project.rs`'s `AUTH_WRAPPERS` reveals the `Secret` to the runtime's
            // `String`-typed `auth_sign_token` at the wrapper boundary.
            K::AuthSignToken => fun(
                secret(),
                fun(dict(string(), string()), fun(int(), result(error_ty(), string()))),
            ),
            // verifyToken : Secret -> String -> Result Error (Dict String String)
            // same re-typing as `signToken` above.
            K::AuthVerifyToken => {
                fun(secret(), fun(string(), result(error_ty(), dict(string(), string()))))
            }
            // register : Db -> String -> String -> Task Error Int
            K::AuthRegister => fun(db(), fun(string(), fun(string(), task(int())))),
            // login : Db -> String -> String -> Task Error Int
            K::AuthLogin => fun(db(), fun(string(), fun(string(), task(int())))),
            // setRole : Db -> Int -> String -> Task Error ()
            K::AuthSetRole => fun(db(), fun(int(), fun(string(), task_unit()))),

            // ── Ipe.Secret — opaque secret-string wrapper ────
            // `fromString` is the seal (construction boundary); `reveal` is the
            // single greppable un-parse; `redacted` is the explicit "<redacted>"
            // accessor (also what `toString`/interpolation gives automatically —
            // see `ipe_runtime::secret`'s hand-written `IpeStringify` impl). No
            // `ty_is_equatable`/`has_show` denylist needed: `Secret` is a bare
            // nullary `Ty::Con`, so `==`/`toString` stay permitted (safe by
            // construction — see the fix spec §1) while Dict-key/Set-elem/`<`/`>`
            // are already rejected by the existing scalar allowlist in
            // `ipe_types::{concrete_super_ok, emitted_bound_satisfied}`.
            K::SecretFromString => fun(string(), secret()),
            K::SecretReveal => fun(secret(), string()),
            // `Secret.use : Secret -> (String -> a) -> a` — apply the caller's
            // function to the revealed plaintext, return its result. Secret-first
            // (pipe-friendly), matching the `secret_use(s, f)` runtime arg order,
            // so it stays off the `kernel_swaps_first_two` list.
            K::SecretUse => fun(secret(), fun(fun(string(), var(0)), var(0))),
            K::SecretRedacted => fun(secret(), string()),

            // ── Ipe.Http.Server.Stream (4 kernels) ────────────────────────
            // stream : String -> (StreamWriter -> Task Error ()) -> Task Error Response
            // The callback receives an opaque `StreamWriter` handle; emit/finish
            // consume the same handle directly (no Int unwrap layer needed).
            K::StreamStream => fun(string(), fun(fun(sw(), task_unit()), task(resp()))),
            // emit : String -> StreamWriter -> Task Error ()
            K::StreamEmit => fun(string(), fun(sw(), task_unit())),
            // finish : StreamWriter -> Task Error ()
            K::StreamFinish => fun(sw(), task_unit()),
            // withContentType : String -> StreamWriter -> Task Error ()
            K::StreamWithContentType => fun(string(), fun(sw(), task_unit())),

            // ── Ipe.Http.Stream (4 kernels) ──────────────────────────
            // open : HttpRequest -> Task Error StreamId
            //
            // Returns an opaque `StreamId` handle wrapping the raw i64 stream
            // registry key.  Typed to match upstream
            // `Ipe.Http.Stream.open`'s declared return type.
            K::HttpStreamOpen => fun(http_request(), task(stream_id())),
            // forEachChunk : StreamId -> (String -> Task Error ()) -> Task Error ()
            K::HttpStreamForEachChunk => {
                fun(stream_id(), fun(fun(string(), task_unit()), task_unit()))
            }
            // close : StreamId -> Task Error ()
            K::HttpStreamClose => fun(stream_id(), task_unit()),
            // chunks : StreamId -> (ChunkEvent -> msg) -> Sub msg
            // ChunkEvent is opaque from the runtime; modelled as `var(0)`.
            K::HttpStreamChunks => fun(stream_id(), fun(fun(var(0), var(1)), sub(var(1)))),

            // ── Ipe.Http.Server.WebSocket (12 kernels) ────────────────────
            // defaultCfg : WebSocketServerCfg
            // Arity-0: the return type IS the scheme (no `fun` wrapper).
            K::WsDefaultCfg => wscfg(),
            // withOnConnect : (WebSocketServer -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnConnect => fun(fun(wsh(), task_unit()), fun(wscfg(), wscfg())),
            // withOnMessage : (WebSocketServer -> String -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnMessage => {
                fun(fun(wsh(), fun(string(), task_unit())), fun(wscfg(), wscfg()))
            }
            // withOnClose : (WebSocketServer -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnClose => fun(fun(wsh(), task_unit()), fun(wscfg(), wscfg())),
            // withOnError : (WebSocketServer -> Error -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnError => {
                fun(fun(wsh(), fun(error_ty(), task_unit())), fun(wscfg(), wscfg()))
            }
            // withMaxMessageBytes : Int -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithMaxMessageBytes => fun(int(), fun(wscfg(), wscfg())),
            // withOriginPatterns : List String -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOriginPatterns => fun(list(string()), fun(wscfg(), wscfg())),
            // upgrade : Request -> WebSocketServerCfg -> Task Error Response
            K::WsUpgrade => fun(req(), fun(wscfg(), task(resp()))),
            // sendToClient : WebSocketServer -> String -> Task Error ()
            K::WsSendToClient => fun(wsh(), fun(string(), task_unit())),
            // sendBinaryToClient : WebSocketServer -> Bytes -> Task Error ()
            K::WsSendBinaryToClient => fun(wsh(), fun(bytes(), task_unit())),
            // broadcast : List WebSocketServer -> String -> Task Error ()
            K::WsBroadcast => fun(list(wsh()), fun(string(), task_unit())),
            // closeClient : WebSocketServer -> Task Error ()
            K::WsCloseClient => fun(wsh(), task_unit()),

            // ── Ipe.WebSocket — outbound WebSocket client ─────────────
            // The Task-tier six take/return a raw `Int` socket id (the stdlib
            // wraps it in the `WebSocket` ADT). `connectWith` takes the nominal
            // `WebSocketCfg` record (`{ url, headers, timeout, pingInterval }`),
            // which the lowerer folds to the runtime `WsClientCfg` struct.
            //
            // connect : String -> Task Error Int
            K::WebSocketConnect => fun(string(), task(int())),
            // connectWith : WebSocketCfg -> Task Error Int
            K::WebSocketConnectWith => fun(wsclientcfg(), task(int())),
            // send : Int -> String -> Task Error ()
            K::WebSocketSend => fun(int(), fun(string(), task_unit())),
            // sendBinary : Int -> Bytes -> Task Error ()
            // Our fork's `Bytes` is a distinct primitive (`Vec<u8>`), matching the
            // runtime `web_socket_send_binary`'s `Vec<u8>` payload (the server-side
            // `sendBinaryToClient` uses the same `bytes()` scheme). Divergence from
            // the reference's stale `String` alias, recorded in the stdlib source.
            K::WebSocketSendBinary => fun(int(), fun(bytes(), task_unit())),
            // close : Int -> Task Error ()
            K::WebSocketClose => fun(int(), task_unit()),
            // closeWithCode : Int -> String -> Int -> Task Error ()
            K::WebSocketCloseWithCode => {
                fun(int(), fun(string(), fun(int(), task_unit())))
            }
            // subscribeWebSocket : Int -> String -> any -> Sub msg
            // The heterogeneous 3rd arg (a bare `msg` for onOpen, or a
            // `WebSocketMessage -> msg` / `CloseCode -> msg` / `Error -> msg`
            // handler for the other three) is modelled as bare `any` (`var(0)`) —
            // matching the stdlib's `subscribeWebSocketRaw` signature so all four
            // on* wrappers unify. `var(1)` is the Sub's msg. The backend peephole
            // splits on the literal `kind` into the four typed `sub_subscribe_ws_*`
            // runtime fns, each with its own concrete 3rd-arg contract.
            K::SubSubscribeWebSocket => {
                fun(int(), fun(string(), fun(var(0), sub(var(1)))))
            }

            // ── Ipe.Env — build-time-embedded public config ──────────
            // public : String -> Maybe String. Resolves ONLY for a
            // `[wasm] publicEnv`-allowlisted key (`env_public.rs`, a
            // per-project backend-generated file — see `project.rs`'s
            // `render_env_public_rs`); every other key is `Nothing`.
            K::EnvPublic => fun(string(), maybe(string())),

            // ── Ipe.Regex (6 kernels) ────────────────────────────────
            // Concrete, monomorphic schemes (no type vars). `compile` parses a
            // pattern String ONCE into the opaque `Regex` handle, surfacing an
            // invalid pattern as a typed `Err` (`String -> Result Error Regex`).
            // Every operation then takes the compiled `Regex`: `match` returns
            // `Bool`; `find` a `Maybe String`; `findAll`/`split` a `List
            // String`; `replace` is `Regex -> String -> String -> String`.
            // Runtime is total/pure (`ipe_runtime::regex_kernel::*`,
            // re-exported ungated).
            K::RegexCompile => fun(string(), result(error_ty(), regex())),
            K::RegexMatch => fun(regex(), fun(string(), bool_ty())),
            K::RegexFind => fun(regex(), fun(string(), maybe(string()))),
            K::RegexFindAll => fun(regex(), fun(string(), list(string()))),
            K::RegexReplace => fun(regex(), fun(string(), fun(string(), string()))),
            K::RegexSplit => fun(regex(), fun(string(), list(string()))),

            // ── Ipe.Path (6 kernels) ─────────────────────────────────
            // `Path` is opaque and validated. `fromString` (the seal) parses a
            // raw `String` into `Result Error Path` — rejecting NUL / traversal
            // escapes at construction; `toString` unwraps back to `String`. The
            // helpers `base`/`dir`/`ext` take a `Path` and return `String`;
            // `isAbsolute` takes a `Path` and returns `Bool`. Runtime total/pure
            // (`ipe_runtime::path::*`, re-exported ungated).
            K::PathFromString => fun(string(), result(error_ty(), path())),
            K::PathToString => fun(path(), string()),
            K::PathBase => fun(path(), string()),
            K::PathDir => fun(path(), string()),
            K::PathExt => fun(path(), string()),
            K::PathIsAbsolute => fun(path(), bool_ty()),

            // ── Ipe.Trace (3 kernels) ─────────────────────────────────────
            // `span : String -> Task a -> Task a` — the wrapped Task's value flows
            // through untouched; the error channel is the implicit `Error`.
            // `event : String -> Task ()`; `attr : String -> String -> Task ()`.
            K::TraceSpan => fun(string(), fun(task(var(0)), task(var(0)))),
            K::TraceEvent => fun(string(), task_unit()),
            K::TraceAttr => fun(string(), fun(string(), task_unit())),

            // ── Ipe.Compression (4 kernels) ───────────────────────────────
            // `Bytes -> Task Bytes` — the Rust runtime `compression_*` takes and
            // returns `Vec<u8>` (`Bytes` lowers to `Vec<u8>`), a documented
            // divergence from the Go backend's `String`-as-bytes shape.
            K::CompressionGzip => fun(bytes(), task(bytes())),
            K::CompressionGunzip => fun(bytes(), task(bytes())),
            K::CompressionZstdCompress => fun(bytes(), task(bytes())),
            K::CompressionZstdDecompress => fun(bytes(), task(bytes())),

            // ── Ipe.Csv (5 kernels) ───────────────────────────────────────
            // `Csv` is the closed record `{ header : List String,
            // rows : List (List String) }` (runtime `ipe_runtime::csv::CsvDoc`).
            K::CsvParse => fun(string(), result(error_ty(), csv_rec())),
            K::CsvParseWithDelimiter => {
                fun(string(), fun(string(), result(error_ty(), csv_rec())))
            }
            K::CsvEncode => fun(csv_rec(), string()),
            K::CsvEncodeWithDelimiter => fun(string(), fun(csv_rec(), string())),
            K::CsvParseStreamFromFile => fun(string(), task(list(list(string())))),

            // ── Ipe.Cache (7 kernels) ─────────────────────────────────────
            // All take the raw `Int` handle. `k`/`v` are the surface key/value
            // type variables (`var(0)`/`var(1)`); the runtime scans keys by
            // `PartialEq`. `newRaw` takes the `CacheCfg` record, `stats` returns
            // the `{ hits, misses, evictions }` record.
            K::CacheNewRaw => fun(cachecfg_rec(), task(int())),
            K::CacheGet => fun(int(), fun(var(0), task(maybe(var(1))))),
            K::CachePut => fun(int(), fun(var(0), fun(var(1), task_unit()))),
            K::CacheRemove => fun(int(), fun(var(0), task_unit())),
            K::CacheClear => fun(int(), task_unit()),
            K::CacheSize => fun(int(), task(int())),
            K::CacheStats => fun(int(), task(cache_stats_rec())),

            // ── Ipe.Email ─────────────────────────────────────────────────────────
            // send : EmailProvider -> EmailMessage -> Task Error String
            K::EmailSend => fun(
                email_provider(),
                fun(email_message_rec(), task(string())),
            ),

            // ── Ipe.Crypto typed-key newtypes ────────────────────────────────────
            // Construction boundaries — parse-don't-validate:
            //   fromString : String -> Key
            //   fromBytes  : String -> Key
            // Extraction boundary:
            //   Mac.toHex  : Mac -> String
            K::CryptoKeyFromString => fun(string(), crypto_key()),
            K::CryptoKeyFromBytes => fun(string(), crypto_key()),
            K::CryptoMacToHex => fun(crypto_mac(), string()),
            // Typed HMAC variants — Key replaces the bare String role parameter;
            // Mac replaces the bare String return:
            //   hmacSha256WithKey : Key -> String -> Mac
            //   hmacSha512WithKey : Key -> String -> Mac
            K::CryptoHmacSha256WithKey => fun(crypto_key(), fun(string(), crypto_mac())),
            K::CryptoHmacSha512WithKey => fun(crypto_key(), fun(string(), crypto_mac())),
            // Typed key-derivation — same inputs, typed Key output:
            //   aesKeyFromPasswordKey   : String -> String -> Key
            //   chachaKeyFromPasswordKey: String -> String -> Key
            K::CryptoAesKeyFromPasswordKey => fun(string(), fun(string(), crypto_key())),
            K::CryptoChachaKeyFromPasswordKey => fun(string(), fun(string(), crypto_key())),
            // Typed AEAD variants — Key replaces bare String key role:
            //   aesGcmEncryptKey  : Key -> String -> Result Error String
            //   aesGcmDecryptKey  : Key -> String -> Result Error String
            //   chacha20EncryptKey: Key -> String -> Result Error String
            //   chacha20DecryptKey: Key -> String -> Result Error String
            K::CryptoAesGcmEncryptKey => {
                fun(crypto_key(), fun(string(), result(error_ty(), string())))
            }
            K::CryptoAesGcmDecryptKey => {
                fun(crypto_key(), fun(string(), result(error_ty(), string())))
            }
            K::CryptoChacha20EncryptKey => {
                fun(crypto_key(), fun(string(), result(error_ty(), string())))
            }
            K::CryptoChacha20DecryptKey => {
                fun(crypto_key(), fun(string(), result(error_ty(), string())))
            }

            // ── Ipe.Email.EmailAddress ────────────────────────────────────────────
            // parse-don't-validate boundary — invalid addresses surface as Nothing:
            //   parse    : String -> Maybe EmailAddress
            //   toString : EmailAddress -> String
            K::EmailAddressParse => fun(string(), maybe(email_address())),
            K::EmailAddressToString => fun(email_address(), string()),
            // ── Ipe.Locale ──────────────────────────────────────────────
            K::LocaleFromTag => fun(string(), maybe(locale())),
            K::LocaleToTag => fun(locale(), string()),
            // `toUpperIn`/`toLowerIn`: `Locale -> String -> String`
            K::StringToUpperIn => fun(locale(), fun(string(), string())),
            K::StringToLowerIn => fun(locale(), fun(string(), string())),

            // ── Ipe.Url ───────────────────────────────────────────────────────────
            // parse-don't-validate boundary — an unparseable / relative URL
            // surfaces as `Err`, never a silent accept:
            //   fromString : String -> Result Error Url
            //   toString   : Url -> String
            //   scheme     : Url -> String
            //   host       : Url -> Maybe String
            //   port       : Url -> Maybe Int
            //   path       : Url -> String
            //   query      : Url -> Maybe String
            //   fragment   : Url -> Maybe String
            //   buildQuery : List (String, String) -> String  (percent-encoding)
            K::UrlFromString => fun(string(), result(error_ty(), url())),
            K::UrlToString => fun(url(), string()),
            K::UrlScheme => fun(url(), string()),
            K::UrlHost => fun(url(), maybe(string())),
            K::UrlPort => fun(url(), maybe(int())),
            K::UrlPath => fun(url(), string()),
            K::UrlQuery => fun(url(), maybe(string())),
            K::UrlFragment => fun(url(), maybe(string())),
            K::UrlBuildQuery => fun(list(tuple2(string(), string())), string()),

            // ── Ui.link ──────────────────────────────────────────────────────────
            // link : List (Attribute msg) -> { url : String, label : Element msg }
            //      -> Element msg
            K::UiLink => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    // `url : String`
                    m.insert(self.builtins.http_f_url, string());
                    // `label : Element msg`
                    m.insert(self.builtins.btn_f_label, elem_t(var(0)));
                    m
                }, RowTail::Closed);
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Ui.image ─────────────────────────────────────────────────────────
            // image : List (Attribute msg) -> { src : String, description : String }
            //       -> Element msg
            K::UiImage => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.img_f_src, string());
                    m.insert(self.builtins.img_f_description, string());
                    m
                }, RowTail::Closed);
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Border.widthEach ─────────────────────────────────────────────────
            // widthEach : { top : Int, right : Int, bottom : Int, left : Int }
            //           -> Attribute msg
            K::BorderWidthEach => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.edge_f_top, int());
                    m.insert(self.builtins.edge_f_right, int());
                    m.insert(self.builtins.edge_f_bottom, int());
                    m.insert(self.builtins.edge_f_left, int());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // ── Border.shadow ────────────────────────────────────────────────────
            // shadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int,
            //            color : Color } -> Attribute msg
            K::BorderShadow => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.shadow_f_offset_x, int());
                    m.insert(self.builtins.shadow_f_offset_y, int());
                    m.insert(self.builtins.shadow_f_blur, int());
                    m.insert(self.builtins.shadow_f_spread, int());
                    m.insert(self.builtins.shadow_f_color, color());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // ── Border.glow ──────────────────────────────────────────────────────
            // glow : Int -> Color -> Attribute msg  (convenience box-shadow with
            // 0,0 offset + 0 spread; user supplies blur radius + colour). Curried
            // 2-arg — no record, unlike `Border.shadow`.
            K::BorderGlow => fun(int(), fun(color(), attr(var(0)))),

            // ── Border.innerShadow ────────────────────────────────────────────────
            // innerShadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int,
            //                 color : Color } -> Attribute msg
            // Same record shape as Border.shadow but INSET; reuses the shadow field
            // symbols.
            K::BorderInnerShadow => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.shadow_f_offset_x, int());
                    m.insert(self.builtins.shadow_f_offset_y, int());
                    m.insert(self.builtins.shadow_f_blur, int());
                    m.insert(self.builtins.shadow_f_spread, int());
                    m.insert(self.builtins.shadow_f_color, color());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // ── Ipe.Db.Sql — SqlFragment builder ───────────────
            //
            // `Sql.column : String -> SqlFragment` — validated column/table
            // reference (dot-accepting, so `users.id` is legal).
            K::SqlColumn => fun(string(), sqlfragment()),
            // `Ipe.Db.Unsafe.unsafeFragment : String -> SqlFragment` — the
            // un-validated anti-`Sql.column` (same shape, no `valid_sql_ident`).
            K::SqlUnsafeFragment => fun(string(), sqlfragment()),
            // `Sql.param : SqlValue -> SqlFragment` — binds a single `?`.
            K::SqlParam => fun(sqlvalue(), sqlfragment()),
            // `int` / `string` / `float` / `bool` are Ipê-level type
            // narrowings of `param` (sugar — see the kernel decl doc): same
            // shape, scalar argument instead of the `SqlValue` ADT.
            K::SqlInt => fun(int(), sqlfragment()),
            K::SqlString => fun(string(), sqlfragment()),
            K::SqlFloat => fun(float(), sqlfragment()),
            K::SqlBool => fun(bool_ty(), sqlfragment()),
            // `eq/ne/gt/lt/gte/lte/and/or : SqlFragment -> SqlFragment -> SqlFragment`
            K::SqlEq
            | K::SqlNe
            | K::SqlGt
            | K::SqlLt
            | K::SqlGte
            | K::SqlLte
            | K::SqlAnd
            | K::SqlOr => fun(sqlfragment(), fun(sqlfragment(), sqlfragment())),
            // `not/isNull/isNotNull : SqlFragment -> SqlFragment`
            K::SqlNot | K::SqlIsNull | K::SqlIsNotNull => fun(sqlfragment(), sqlfragment()),
            // `inList : SqlFragment -> List SqlValue -> SqlFragment` — `[]`
            // emits `(1 = 0)` at the runtime combinator, not a type-level case.
            K::SqlInList => fun(sqlfragment(), fun(list(sqlvalue()), sqlfragment())),
            // `like : SqlFragment -> String -> SqlFragment` — the pattern is
            // always a bound param.
            K::SqlLike => fun(sqlfragment(), fun(string(), sqlfragment())),
        })
    }
}

// ===========================================================================
// Boundary Scheme Promotion — untyped top-level binding generalization.
//
// See `docs/adr/0008-untyped-binding-module-boundary-generalization.md` for the
// full design. Summary: an unannotated top-level binding is monomorphic
// *within its home module* (unchanged), but is generalized into a scheme at
// its module's boundary, so each cross-module reference instantiates it
// fresh — exactly like an annotated (typed) binding already does via
// `instantiate_tracked`, except the scheme is *discovered* post-solve rather
// than declared. `promote_untyped_boundaries` (called once, between
// `solve_attributed` and `resolve_deferred`) drives this for the whole
// linked program, in module topo order.
// ===========================================================================

/// A generalized scheme for one untyped top-level binding, discovered at its
/// home module's boundary-discharge step.
///
/// `quantified` maps each generalized `Flex` root to its synthesized name
/// (`"a"`, `"b"`, …, never `"any"`). Only plain, obligation-free `Flex` roots
/// are quantified in phase 1 — `Super`-bounded and `Rigid`-contaminated roots
/// stay shared program-wide (Divergences D2/D3 in the spec); a residual root
/// still reachable from a pending field-access / record-update / route
/// obligation is excluded too (the existing "single concrete use" gate
/// fallback stays intact for those defs).
pub struct UntypedScheme {
    /// The shared, home-module-monomorphic root every same-module reference
    /// (and, pre-discharge, the binding's own `untyped[key]` var) resolves to.
    root: VarId,
    /// Generalized root → synthesized type-variable name.
    pub quantified: BTreeMap<VarId, Symbol>,
}

/// Every untyped def's generalized scheme, keyed by `(home, name)`. Returned
/// by [`promote_untyped_boundaries`].
pub type UntypedSchemes = BTreeMap<(Vec<Symbol>, Symbol), UntypedScheme>;

const COPY_VAR_NODE_LIMIT: u32 = 4_096;

/// One step of the iterative [`copy_var`] work stack — the mirror image of
/// [`ZonkTask`]: instead of reading a settled UF node back into an owned
/// [`Ty`], it builds a *fresh* UF substructure over it.
enum CopyVarTask {
    Visit(VarId),
    BuildFun,
    BuildCon {
        module: Vec<Symbol>,
        name: Symbol,
        arity: usize,
    },
    BuildTuple {
        arity: usize,
    },
    BuildRecord {
        names: Vec<Symbol>,
    },
}

/// Instantiate a generalized untyped-def scheme at one use site.
///
/// A quantified root (per `quantified`) gets a fresh `Flex` per call — shared
/// via `fresh_map` so repeated occurrences of the same quantified var within
/// *this one* instantiation alpha-rename consistently (`fresh_map` must be
/// fresh per discharge, i.e. per cross-module reference, not shared across
/// references). Every other var — `Flex` not in `quantified`, `Super`,
/// `Rigid` — is returned as-is, unchanged: this is what makes a program with
/// no boundary-free untyped defs byte-identical to today, since nothing is
/// ever copied unless it was actually quantified. Every `Structure` node is
/// rebuilt with fresh children, including a **fresh** `EmptyRecord` sentinel
/// per closed record (mirrors `empty_record_tail`'s occurs-distinctness
/// rule) — this is a UF-level copy-walk, deliberately NOT a `Ty`-level reify
/// (`instantiate_in`), so it never needs to round-trip through a resolved
/// `Ty` (and its `AUD-13` solver-var tagging) at all.
///
/// **Iterative**, mirroring [`zonk`]: an explicit heap-allocated work stack,
/// so it never grows the native call stack regardless of how deep the
/// scheme's type is, budget-ticked per node and bounded by
/// [`COPY_VAR_NODE_LIMIT`] (stack-safety, not the DOS budget).
///
/// # Errors
/// [`Diagnostic::CompilerBug`] on a union-find invariant violation or if the
/// structure has more than [`COPY_VAR_NODE_LIMIT`] nodes;
/// [`TypeError::StepBudgetExceeded`] if the shared budget is exhausted.
#[allow(clippy::too_many_lines)] // one task-stack state machine, mirrors `zonk` — splitting would obscure the Visit/Build pairing
fn copy_var(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    var: VarId,
    quantified: &BTreeMap<VarId, Symbol>,
    fresh_map: &mut BTreeMap<VarId, VarId>,
) -> DResult<VarId> {
    let mut work: Vec<CopyVarTask> = vec![CopyVarTask::Visit(var)];
    let mut results: Vec<VarId> = Vec::new();
    let mut nodes_left = COPY_VAR_NODE_LIMIT;

    while let Some(task) = work.pop() {
        match task {
            CopyVarTask::Visit(v) => {
                budget.tick()?;
                nodes_left = nodes_left
                    .checked_sub(1)
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: "type exceeded scheme-instantiation node limit".to_owned(),
                    })?;
                let root = uf.find(v)?;
                if let Some(&fresh) = fresh_map.get(&root) {
                    results.push(fresh);
                    continue;
                }
                match uf.content(root)? {
                    Content::Flex if quantified.contains_key(&root) => {
                        let fresh = uf.fresh(Content::Flex)?;
                        fresh_map.insert(root, fresh);
                        results.push(fresh);
                    }
                    Content::Flex | Content::Rigid | Content::Super { .. } => {
                        // Not quantified: shared program-wide, no copy.
                        fresh_map.insert(root, root);
                        results.push(root);
                    }
                    Content::Structure(FlatType::Unit) => {
                        results.push(uf.fresh(Content::Structure(FlatType::Unit))?);
                    }
                    Content::Structure(FlatType::EmptyRecord) => {
                        // A fresh sentinel per copy — same rationale as
                        // `empty_record_tail`: distinct closed records must
                        // stay distinguishable to a later occurs check.
                        results.push(uf.fresh(Content::Structure(FlatType::EmptyRecord))?);
                    }
                    Content::Structure(FlatType::Fun(a, b)) => {
                        work.push(CopyVarTask::BuildFun);
                        work.push(CopyVarTask::Visit(b));
                        work.push(CopyVarTask::Visit(a));
                    }
                    Content::Structure(FlatType::Con { module, name, args }) => {
                        let arity = args.len();
                        work.push(CopyVarTask::BuildCon {
                            module,
                            name,
                            arity,
                        });
                        for a in args.into_iter().rev() {
                            work.push(CopyVarTask::Visit(a));
                        }
                    }
                    Content::Structure(FlatType::Tuple(elems)) => {
                        let arity = elems.len();
                        work.push(CopyVarTask::BuildTuple { arity });
                        for e in elems.into_iter().rev() {
                            work.push(CopyVarTask::Visit(e));
                        }
                    }
                    Content::Structure(FlatType::Record(fields, ext)) => {
                        let names: Vec<Symbol> = fields.keys().copied().collect();
                        work.push(CopyVarTask::BuildRecord { names });
                        work.push(CopyVarTask::Visit(ext));
                        for v in fields.values().copied().rev() {
                            work.push(CopyVarTask::Visit(v));
                        }
                    }
                }
            }
            CopyVarTask::BuildFun => {
                let (Some(b), Some(a)) = (results.pop(), results.pop()) else {
                    return Err(copy_var_underflow());
                };
                results.push(uf.fresh(Content::Structure(FlatType::Fun(a, b)))?);
            }
            CopyVarTask::BuildCon {
                module,
                name,
                arity,
            } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(copy_var_underflow)?;
                let args = results.split_off(split);
                results.push(uf.fresh(Content::Structure(FlatType::Con { module, name, args }))?);
            }
            CopyVarTask::BuildTuple { arity } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(copy_var_underflow)?;
                let elems = results.split_off(split);
                results.push(uf.fresh(Content::Structure(FlatType::Tuple(elems)))?);
            }
            CopyVarTask::BuildRecord { names } => {
                let Some(ext) = results.pop() else {
                    return Err(copy_var_underflow());
                };
                let split = results
                    .len()
                    .checked_sub(names.len())
                    .ok_or_else(copy_var_underflow)?;
                let vals = results.split_off(split);
                let fields: BTreeMap<Symbol, VarId> = names.into_iter().zip(vals).collect();
                results.push(uf.fresh(Content::Structure(FlatType::Record(fields, ext)))?);
            }
        }
    }

    match results.pop() {
        Some(v) if results.is_empty() => Ok(v),
        _ => Err(copy_var_underflow()),
    }
}

/// The work-stack invariant was violated (only reachable via a compiler bug in
/// `copy_var` itself, never from input).
fn copy_var_underflow() -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: STAGE,
        detail: "copy_var result stack underflow".to_owned(),
    }
}

/// Every `Flex`-content root structurally reachable from `root` (through
/// `Structure` children only — `Flex`/`Rigid`/`Super` are leaves), collected
/// as UF representatives. The traversal shape mirrors `unify::occurs`
/// exactly (iterative, explicit stack, budget-ticked per node), just
/// collecting instead of comparing against a target.
///
/// Used by `promote_untyped_boundaries` to find an untyped binding's
/// generalization *candidates* — the actual quantified set additionally
/// excludes any root still reachable from a pending deferred obligation (see
/// callers).
fn reachable_flex_roots(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    root: VarId,
) -> DResult<std::collections::BTreeSet<VarId>> {
    let mut seen: std::collections::BTreeSet<VarId> = std::collections::BTreeSet::new();
    let mut flex: std::collections::BTreeSet<VarId> = std::collections::BTreeSet::new();
    let mut stack = vec![root];
    while let Some(v) = stack.pop() {
        budget.tick()?;
        let here = uf.find(v)?;
        if !seen.insert(here) {
            continue;
        }
        match uf.content(here)? {
            Content::Flex => {
                flex.insert(here);
            }
            Content::Rigid
            | Content::Super { .. }
            | Content::Structure(FlatType::Unit | FlatType::EmptyRecord) => {}
            Content::Structure(FlatType::Fun(a, b)) => {
                stack.push(a);
                stack.push(b);
            }
            Content::Structure(FlatType::Con { args, .. }) => {
                for a in args {
                    stack.push(a);
                }
            }
            Content::Structure(FlatType::Tuple(elems)) => {
                for e in elems {
                    stack.push(e);
                }
            }
            Content::Structure(FlatType::Record(fields, ext)) => {
                for v in fields.values() {
                    stack.push(*v);
                }
                stack.push(ext);
            }
        }
    }
    Ok(flex)
}

/// Mint a fresh, source-collision-free type-variable name (`"a"`, `"b"`, …,
/// `"z"`, `"a1"`, …) for a generalized untyped-def scheme — never `"any"`
/// (AUD-13's wildcard sentinel is reserved). `next` is the caller's shared
/// naming cursor, threaded across every quantified var of every scheme in one
/// `promote_untyped_boundaries` run so names stay distinct program-wide (not
/// required for soundness — each scheme's names only need to be distinct
/// *within* that scheme — but keeps `IPE_DEBUG_UNTYPED` dumps unambiguous).
fn mint_synth_symbol(interner: &mut Interner, next: &mut u32) -> DResult<Symbol> {
    loop {
        let candidate = crate::doc::letters(*next);
        *next = next.saturating_add(1);
        if !interner.contains(&candidate) {
            return interner.intern(&candidate);
        }
    }
}

/// Boundary Scheme Promotion — discharge every cross-module untyped-binding
/// reference and generalize every untyped def at its home module's boundary.
///
/// Runs once, over the WHOLE linked program, between `solve_attributed` and
/// `resolve_deferred` (see `docs/adr/0008-untyped-binding-module-boundary-generalization.md`'s
/// algorithm section). Walks `module_order` (dependency-first topo order): for
/// each module, first discharges its own OUTGOING pending instantiations
/// (against schemes already computed for modules it depends on — always
/// present, since those modules precede it in `module_order`), then
/// generalizes its OWN untyped defs (recording their schemes for later
/// modules to discharge against).
///
/// Returns the generalized scheme for every `(home, name)` key `untyped`
/// covers (an entry with an empty `quantified` map means the def stayed
/// fully monomorphic — no boundary-free residual `Flex` root). The caller
/// folds this into `SolvedTypes::untyped_type_params` / `poly_var_map`.
///
/// # Errors
/// A cross-module reference's instantiated scheme failing to unify against
/// local call-site structure is a genuine `IPE-T0001`, blamed on the
/// referencing (`use_home`) module. A union-find invariant violation is a
/// `Diagnostic::CompilerBug` with an empty home.
pub fn promote_untyped_boundaries(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &mut Interner,
    generated: &Generated,
) -> Result<UntypedSchemes, (Diagnostic, Vec<Symbol>)> {
    macro_rules! lift {
        ($e:expr) => {
            $e.map_err(|d: Diagnostic| (d, Vec::<Symbol>::new()))?
        };
    }

    // Roots still reachable from a still-pending deferred obligation are
    // excluded from quantification — the existing "single concrete use" gate
    // fallback for these defs stays intact (test matrix item 6; D2/D3-style
    // conservative under-acceptance). Computed once, globally: every one of
    // these obligations is still pending at this point in the pipeline (this
    // pass runs BEFORE `resolve_deferred`), regardless of which module owns
    // which untyped def.
    let mut obligation_roots: std::collections::BTreeSet<VarId> = std::collections::BTreeSet::new();
    for fa in &generated.field_accesses {
        obligation_roots.insert(lift!(uf.find(fa.record)));
        // A simple getter's own result var (`fa.result`) IS the function's
        // return-type var — e.g. `getName r = r.name`. Excluding only
        // `fa.record` left `fa.result` a residual plain-`Flex` root, eligible
        // for quantification here even though `resolve_deferred` (which runs
        // AFTER this pass) is what pins it to the concrete field type. A
        // quantified-then-later-pinned var produced a Rust generic that
        // appeared in neither `params` nor `ret` — E0283 at the emitted
        // `cargo build` step. Confirmed by independent review as a real SEAL
        // violation on a 3-module cross-module field-access getter; see
        // BACKLOG.md's "Boundary Scheme Promotion" row.
        obligation_roots.insert(lift!(uf.find(fa.result)));
    }
    for ru in &generated.record_updates {
        obligation_roots.insert(lift!(uf.find(ru.record)));
        // Symmetric to `fa.result` above: each updated field's VALUE var is
        // pinned to the record's concrete field type by
        // [`crate::resolve_record_updates`], which runs AFTER this pass. At
        // this point it can still be a residual plain-`Flex` root (e.g. the
        // `n` parameter in `setName r n = { r | name = n }`), so without this
        // exclusion it would be quantified into the def's scheme and later
        // pinned — producing a stale quantified symbol that structurally
        // appears nowhere in the resolved `params`/`ret`. The lowerer's
        // `used_generics` filter independently strips such a symbol
        // (defense-in-depth, empirically verified), but the primary
        // obligation-exclusion mechanism must be complete in its own right.
        for &(_, value_var) in &ru.fields {
            obligation_roots.insert(lift!(uf.find(value_var)));
        }
    }
    for rw in &generated.route_witness_checks {
        obligation_roots.insert(lift!(uf.find(rw.builder_var)));
        obligation_roots.insert(lift!(uf.find(rw.page_var)));
    }
    for rl in &generated.routed_web_checks {
        obligation_roots.insert(lift!(uf.find(rl.model_var)));
        obligation_roots.insert(lift!(uf.find(rl.not_found_var)));
    }

    let mut schemes: UntypedSchemes = BTreeMap::new();
    // Shared naming cursor across every scheme in this run — see
    // `mint_synth_symbol`'s doc comment for why this is a convenience, not a
    // soundness requirement.
    let mut synth_next: u32 = 0;

    for home in &generated.module_order {
        // (a) Discharge this module's OUTGOING cross-module references.
        for pi in generated
            .pending_instantiations
            .iter()
            .filter(|pi| &pi.use_home == home)
        {
            let Some(scheme) = schemes.get(&pi.source) else {
                // module_order is dependency-first, and a `PendingInstantiation`
                // only exists for a key already present in `untyped` — so the
                // source module always precedes `use_home` and always has a
                // scheme by now. Unreachable except via a link-order invariant
                // break; fail closed rather than panic.
                return Err((
                    Diagnostic::CompilerBug {
                        where_: "ipe_types::promote_untyped_boundaries",
                        detail: "cross-module untyped reference discharged before its source \
                                 module was generalized"
                            .to_owned(),
                    },
                    pi.use_home.clone(),
                ));
            };
            let root = scheme.root;
            let quantified = scheme.quantified.clone();
            let mut fresh_map = BTreeMap::new();
            let inst = copy_var(uf, budget, root, &quantified, &mut fresh_map)
                .map_err(|d| (d, pi.use_home.clone()))?;
            unify(uf, budget, interner, pi.span, inst, pi.placeholder)
                .map_err(|d| (d, pi.use_home.clone()))?;
        }

        // (b) Generalize this module's own untyped defs.
        for (key, &shared) in generated.untyped.iter().filter(|(k, _)| &k.0 == home) {
            let root = lift!(uf.find(shared));
            let candidates =
                reachable_flex_roots(uf, budget, root).map_err(|d| (d, key.0.clone()))?;
            let mut quantified = BTreeMap::new();
            for r in candidates {
                if obligation_roots.contains(&r) {
                    continue;
                }
                let sym = lift!(mint_synth_symbol(interner, &mut synth_next));
                quantified.insert(r, sym);
            }
            schemes.insert(key.clone(), UntypedScheme { root, quantified });
        }
    }

    Ok(schemes)
}

/// One step of the iterative [`reify_scheme`] work stack — the interface
/// sibling of [`ZonkTask`], differing in the two places an interface must be
/// faithful where a display read-back need not be: quantified variables map
/// to CANONICAL tagged ids (deterministic, union-find-numbering-free), and an
/// open record tail is PRESERVED as `RowTail::Open` (zonk presents every
/// settled record as closed, which is fine for display but would silently
/// close a row-polymorphic exported scheme).
enum ReifyTask {
    Visit(VarId),
    BuildFun,
    BuildCon {
        module: Vec<Symbol>,
        name: Symbol,
        arity: usize,
    },
    BuildTuple {
        arity: usize,
    },
    BuildRecord {
        names: Vec<Symbol>,
        tail: RowTail,
    },
}

/// Reify one generalized untyped-binding scheme into an owned interface
/// [`Ty`], or report the scheme OPEN.
///
/// A quantified root becomes `Ty::Var(tag_solver_var(k))` where `k` is the
/// root's first-encounter index in this walk — canonical, so the same scheme
/// reifies to the same bytes regardless of union-find numbering (the
/// backdating property a typed interface exists for). A reachable residual
/// variable that is NOT quantified — a plain `Flex` sharable program-wide, a
/// `Super` obligation a later defaulting pass would conceal, a `Rigid`
/// contamination — makes the scheme OPEN (`Ok(None)`): its final type can
/// legitimately be pinned by an importer, so no per-module interface can
/// stand for it. Must run BEFORE numeric/SQL defaulting: defaulting pins
/// residual `Super` flexes to concrete types, which would disguise an open
/// scheme as closed and let a scoped solve disagree with the joint one.
///
/// # Errors
/// [`Diagnostic::CompilerBug`] on a union-find invariant violation or a
/// structure over [`ZONK_NODE_LIMIT`] nodes; [`TypeError::StepBudgetExceeded`]
/// on budget exhaustion.
#[allow(clippy::too_many_lines)] // one task-stack state machine, mirrors `zonk` — splitting would obscure the Visit/Build pairing
pub fn reify_scheme(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    scheme: &UntypedScheme,
) -> DResult<Option<Ty>> {
    let mut work: Vec<ReifyTask> = vec![ReifyTask::Visit(scheme.root)];
    let mut results: Vec<Ty> = Vec::new();
    let mut canonical: BTreeMap<VarId, u32> = BTreeMap::new();
    let mut nodes_left = ZONK_NODE_LIMIT;

    let canonical_raw = |root: VarId, canonical: &mut BTreeMap<VarId, u32>| -> u32 {
        let next = u32::try_from(canonical.len()).unwrap_or(u32::MAX);
        tag_solver_var(*canonical.entry(root).or_insert(next))
    };

    while let Some(task) = work.pop() {
        match task {
            ReifyTask::Visit(v) => {
                budget.tick()?;
                nodes_left = nodes_left
                    .checked_sub(1)
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: "type exceeded interface-reification node limit".to_owned(),
                    })?;
                let root = uf.find(v)?;
                match uf.content(root)? {
                    Content::Flex if scheme.quantified.contains_key(&root) => {
                        results.push(Ty::Var(canonical_raw(root, &mut canonical)));
                    }
                    // A residual non-quantified variable: an importer may
                    // still pin it, so the scheme is open.
                    Content::Flex | Content::Rigid | Content::Super { .. } => {
                        return Ok(None);
                    }
                    // `EmptyRecord` is only reachable on a direct call over a
                    // bare tail — records route tails through `BuildRecord`
                    // below — and falls back to `Ty::Unit` like `zonk` does.
                    Content::Structure(FlatType::Unit | FlatType::EmptyRecord) => {
                        results.push(Ty::Unit);
                    }
                    Content::Structure(FlatType::Fun(a, b)) => {
                        work.push(ReifyTask::BuildFun);
                        work.push(ReifyTask::Visit(b));
                        work.push(ReifyTask::Visit(a));
                    }
                    Content::Structure(FlatType::Con { module, name, args }) => {
                        let arity = args.len();
                        work.push(ReifyTask::BuildCon {
                            module,
                            name,
                            arity,
                        });
                        for a in args.into_iter().rev() {
                            work.push(ReifyTask::Visit(a));
                        }
                    }
                    Content::Structure(FlatType::Tuple(elems)) => {
                        let arity = elems.len();
                        work.push(ReifyTask::BuildTuple { arity });
                        for e in elems.into_iter().rev() {
                            work.push(ReifyTask::Visit(e));
                        }
                    }
                    Content::Structure(FlatType::Record(fields, ext)) => {
                        let names: Vec<Symbol> = fields.keys().copied().collect();
                        let ext_root = uf.find(ext)?;
                        let tail = match uf.content(ext_root)? {
                            Content::Structure(FlatType::EmptyRecord) => RowTail::Closed,
                            Content::Flex if scheme.quantified.contains_key(&ext_root) => {
                                RowTail::Open(canonical_raw(ext_root, &mut canonical))
                            }
                            // A residual open tail an importer could still
                            // grow — the scheme is open.
                            _ => return Ok(None),
                        };
                        work.push(ReifyTask::BuildRecord { names, tail });
                        for fv in fields.values().copied().rev() {
                            work.push(ReifyTask::Visit(fv));
                        }
                    }
                }
            }
            ReifyTask::BuildFun => {
                let (Some(b), Some(a)) = (results.pop(), results.pop()) else {
                    return Err(reify_underflow());
                };
                results.push(Ty::Fun(Box::new(a), Box::new(b)));
            }
            ReifyTask::BuildCon {
                module,
                name,
                arity,
            } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(reify_underflow)?;
                let args = results.split_off(split);
                results.push(Ty::Con { module, name, args });
            }
            ReifyTask::BuildTuple { arity } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(reify_underflow)?;
                let elems = results.split_off(split);
                results.push(Ty::Tuple(elems));
            }
            ReifyTask::BuildRecord { names, tail } => {
                let split = results
                    .len()
                    .checked_sub(names.len())
                    .ok_or_else(reify_underflow)?;
                let tys = results.split_off(split);
                let fields: BTreeMap<Symbol, Ty> = names.into_iter().zip(tys).collect();
                results.push(Ty::Record(fields, tail));
            }
        }
    }

    match results.pop() {
        Some(ty) if results.is_empty() => Ok(Some(ty)),
        _ => Err(reify_underflow()),
    }
}

/// The work-stack invariant was violated (only reachable via a compiler bug
/// in `reify_scheme` itself, never from input).
fn reify_underflow() -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: STAGE,
        detail: "reify_scheme result stack underflow".to_owned(),
    }
}

/// A single step of the iterative [`zonk`] work stack.
///
/// `Visit` reads one union-find node and pushes either a leaf result or the
/// `Build*` task plus its children's `Visit`s; the `Build*` tasks reassemble a
/// parent [`Ty`] once its children's results sit on the result stack.
enum ZonkTask {
    /// Resolve and read back one variable.
    Visit(VarId),
    /// Pop two results (`arg`, then `result`) and push a `Fun`.
    BuildFun,
    /// Pop `arity` results and push a `Con` over them.
    BuildCon {
        module: Vec<Symbol>,
        name: Symbol,
        arity: usize,
    },
    /// Pop `arity` results and push a `Tuple` over them.
    BuildTuple { arity: usize },
    /// Pop one result per field name (in `names` order) and push a `Record`. The
    /// `names` are visited in their `BTreeMap` order, so popping in reverse pairs
    /// each result with its field name.
    BuildRecord { names: Vec<Symbol> },
}

/// Read a settled union-find variable back into a resolved [`Ty`].
///
/// Called after [`crate::solve::solve`] has discharged every constraint. The
/// occurs check in unification guarantees the structure is acyclic, so the node
/// bound is only ever hit on adversarial input.
///
/// **Iterative.** The walk runs over an explicit heap-allocated work stack
/// (mirroring the iterative `find` in `unionfind.rs`), so it never grows the
/// native call stack regardless of how deep the type is. Each node visited
/// ticks the shared [`Budget`] (a DOS bound) and consumes one of
/// [`ZONK_NODE_LIMIT`] per-call nodes (a stack-safety bound on the renderer that
/// later walks the result).
///
/// # Errors
/// [`Diagnostic::CompilerBug`] on a union-find invariant violation or if the
/// structure has more than [`ZONK_NODE_LIMIT`] nodes; [`TypeError::StepBudgetExceeded`]
/// if the shared budget is exhausted.
pub fn zonk(uf: &mut UnionFind<Content>, budget: &mut Budget, var: VarId) -> DResult<Ty> {
    let mut work: Vec<ZonkTask> = vec![ZonkTask::Visit(var)];
    let mut results: Vec<Ty> = Vec::new();
    let mut nodes_left = ZONK_NODE_LIMIT;

    while let Some(task) = work.pop() {
        match task {
            ZonkTask::Visit(v) => {
                budget.tick()?;
                nodes_left = nodes_left
                    .checked_sub(1)
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: "type exceeded read-back node limit".to_owned(),
                    })?;
                let root = uf.find(v)?;
                match uf.content(root)? {
                    // A flexible, rigid, or super-typed variable that survives
                    // solving reads back as a type variable named by its
                    // representative's id. (A super-typed variable is still a
                    // variable; its obligations are read separately when
                    // generalising — see [`crate::SolvedTypes::bounds`].)
                    Content::Flex | Content::Rigid | Content::Super { .. } => {
                        // AUD-13: tag so this solver-representative id can
                        // never be mistaken for an annotation-symbol raw by
                        // `instantiate_in`'s wildcard-`"any"` check if this
                        // zonked `Ty` is ever fed back through it.
                        results.push(Ty::Var(tag_solver_var(root)));
                    }
                    Content::Structure(FlatType::Unit) => results.push(Ty::Unit),
                    Content::Structure(FlatType::Fun(a, b)) => {
                        // Push the rebuild first, then the children so that `a`
                        // is visited before `b` and lands lower on `results`.
                        work.push(ZonkTask::BuildFun);
                        work.push(ZonkTask::Visit(b));
                        work.push(ZonkTask::Visit(a));
                    }
                    Content::Structure(FlatType::Con { module, name, args }) => {
                        let arity = args.len();
                        work.push(ZonkTask::BuildCon {
                            module,
                            name,
                            arity,
                        });
                        // Reverse so args land on `results` in source order.
                        for a in args.into_iter().rev() {
                            work.push(ZonkTask::Visit(a));
                        }
                    }
                    Content::Structure(FlatType::Tuple(elems)) => {
                        let arity = elems.len();
                        work.push(ZonkTask::BuildTuple { arity });
                        // Reverse so elements land on `results` in source order.
                        for e in elems.into_iter().rev() {
                            work.push(ZonkTask::Visit(e));
                        }
                    }
                    Content::Structure(FlatType::Record(fields, _ext)) => {
                        // Capture the field names (BTreeMap order) for the
                        // rebuild, and visit each field var in reverse so the
                        // results land in the same order the names are popped.
                        // The extension var is intentionally not zonked here —
                        // `Ty::Record` does not carry a RowTail in its resolved
                        // form (the tail is a solver artefact consumed only by
                        // unify.rs and the `BuildRecord` path).  Closed records
                        // resolve to fields only; open records show as the same
                        // (tail is transparent to diagnostics for now).
                        let names: Vec<Symbol> = fields.keys().copied().collect();
                        work.push(ZonkTask::BuildRecord { names });
                        for v in fields.values().copied().rev() {
                            work.push(ZonkTask::Visit(v));
                        }
                    }
                    Content::Structure(FlatType::EmptyRecord) => {
                        // EmptyRecord is the closed-tail sentinel — it carries no
                        // children and does not produce a `Ty` of its own.
                        // It should only appear as the extension variable of a
                        // `FlatType::Record`, never as the root type of a
                        // standalone expression.  Push `Ty::Unit` as a safe
                        // fallback so the work stack stays balanced (this arm is
                        // reachable if zonk is called directly on an extension
                        // var, which does not happen in normal code, but must not
                        // panic).
                        results.push(Ty::Unit);
                    }
                }
            }
            ZonkTask::BuildFun => {
                let (Some(b), Some(a)) = (results.pop(), results.pop()) else {
                    return Err(zonk_underflow());
                };
                results.push(Ty::Fun(Box::new(a), Box::new(b)));
            }
            ZonkTask::BuildCon {
                module,
                name,
                arity,
            } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(zonk_underflow)?;
                let args = results.split_off(split);
                results.push(Ty::Con { module, name, args });
            }
            ZonkTask::BuildTuple { arity } => {
                let split = results
                    .len()
                    .checked_sub(arity)
                    .ok_or_else(zonk_underflow)?;
                let elems = results.split_off(split);
                results.push(Ty::Tuple(elems));
            }
            ZonkTask::BuildRecord { names } => {
                let split = results
                    .len()
                    .checked_sub(names.len())
                    .ok_or_else(zonk_underflow)?;
                let tys = results.split_off(split);
                // `tys` is in the same order as `names` (field var visits were
                // reversed, so the results stack restores `BTreeMap` order).
                let fields: BTreeMap<Symbol, Ty> = names.into_iter().zip(tys).collect();
                // Zonked records are always presented as closed — the RowTail
                // is a solver artefact; the resolved `Ty` simply carries the
                // settled field map without advertising openness (consistent
                // with the Haskell reference's read-back behaviour).
                results.push(Ty::Record(fields, RowTail::Closed));
            }
        }
    }

    match results.pop() {
        Some(ty) if results.is_empty() => Ok(ty),
        _ => Err(zonk_underflow()),
    }
}

/// The work-stack invariant was violated (only reachable via a compiler bug in
/// `zonk` itself, never from input).
fn zonk_underflow() -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: STAGE,
        detail: "zonk result stack underflow".to_owned(),
    }
}

// ===========================================================================
// Kernel-registry tripwires
// ===========================================================================

impl<'a> Builder<'a> {
    /// Minimal [`Builder`] for reading the pure scheme table
    /// ([`Self::stdlib_scheme`]) outside a full inference run. Only `uf`,
    /// `interner`, and `builtins` are load-bearing for that method; every
    /// other field is empty. Pre-intern any needed strings BEFORE taking the
    /// immutable borrow into `interner`.
    ///
    /// Consumers: the registry tripwire tests below and
    /// [`kernel_type_table`] (the salsa Task-9 `kernel_types()` query's
    /// single source of schemes — one code path, so the query can never
    /// drift from what inference actually uses).
    const fn for_scheme_table(
        uf: &'a mut UnionFind<Content>,
        interner: &'a Interner,
        builtins: Builtins,
    ) -> Self {
        Self {
            uf,
            interner,
            builtins,
            regions: BTreeMap::new(),
            expected: BTreeMap::new(),
            current_home: Vec::new(),
            constraints: Vec::new(),
            top_level: BTreeMap::new(),
            untyped: BTreeMap::new(),
            field_accesses: Vec::new(),
            record_updates: Vec::new(),
            routed_web_checks: Vec::new(),
            route_witness_checks: Vec::new(),
            wildcard_any_return_bodies: BTreeMap::new(),
            wildcard_any_return_bindings: BTreeSet::new(),
            wildcard_any_use_results: Vec::new(),
            ctors: BTreeMap::new(),
            typed_rigids: Vec::new(),
            scheme_apps: Vec::new(),
            super_vars: Vec::new(),
            pending_instantiations: Vec::new(),
        }
    }
}

/// Materialize the full kernel type-scheme table.
///
/// Every [`StdlibKernel`] variant paired with its inference scheme, in
/// `StdlibKernel::ALL` order, skipping variants the registry deliberately
/// never schemes (routed / unlowered buckets — those fail closed with
/// IPE-L0108 at their call sites).
///
/// This is the *lift* behind the salsa `kernel_types()` query: the table is
/// read through the SAME [`Builder::resolve_scheme`] adapter inference uses
/// (a `TyShape`-carrying kernel is interpreted; every other resolves through
/// [`Builder::stdlib_scheme`]), so the memoized table can never drift from what
/// constraint generation actually applies. The schemes are pure functions of
/// the interned builtin names — no union-find state is created or consumed.
///
/// Interning note: [`Builtins::new`] interns the builtin type/constructor
/// names (idempotent lookups when they are already interned — which is the
/// case whenever any parse/canon of stdlib-shaped source has run first).
///
/// # Errors
/// Propagates the interner-capacity diagnostic from [`Builtins::new`] (the
/// only fallible step; the scheme reads themselves are total).
pub fn kernel_type_table(interner: &mut Interner) -> Result<Vec<(StdlibKernel, Ty)>, Diagnostic> {
    let builtins = Builtins::new(interner)?;
    let mut uf: UnionFind<Content> = UnionFind::new();
    let builder = Builder::for_scheme_table(&mut uf, interner, builtins);
    Ok(StdlibKernel::ALL
        .iter()
        .filter_map(|&k| builder.resolve_scheme(SchemeKey(k)).map(|ty| (k, ty)))
        .collect())
}

/// Resolve a single [`SchemeKey`] to its concrete HM type scheme, outside a full
/// inference run.
///
/// This is the free-function entry to the scheme-by-key bridge: a consumer
/// holding a [`ipe_kernels::KernelDef`] reads `def.scheme` (a [`SchemeKey`]) and
/// resolves it here to the same `Ty` inference uses, via the single
/// [`Builder::resolve_scheme`] interpreter (which delegates to
/// [`Builder::stdlib_scheme`]). `Ok(None)` mirrors the table — the kernel has no
/// registry scheme (a routed / unlowered bucket).
///
/// # Errors
/// Propagates the interner-capacity diagnostic from [`Builtins::new`] (the only
/// fallible step; the scheme read itself is total).
pub fn resolve_scheme(key: SchemeKey, interner: &mut Interner) -> Result<Option<Ty>, Diagnostic> {
    let builtins = Builtins::new(interner)?;
    let mut uf: UnionFind<Content> = UnionFind::new();
    let builder = Builder::for_scheme_table(&mut uf, interner, builtins);
    Ok(builder.resolve_scheme(key))
}

#[cfg(test)]
mod registry_phase_c_tests {
    use super::{Builder, Builtins, Content, Diagnostic, Feature, LowerError, Ty, UnionFind};
    use ipe_diagnostics::Span;
    use ipe_intern::{Interner, Symbol};
    use ipe_kernels::{StdlibKernel, TyShape};

    /// Kernels RELOCATED into `stdlib_scheme` from the legacy `kernel_ty` table
    /// (String / List / Math plus the remaining backed families). Each carries
    /// a byte-faithful legacy oracle, so `stdlib_scheme_matches_legacy`
    /// proves the relocation changed no type. Monotone burndown anchor: GROWS
    /// per family, never shrinks, and must exactly match the RELOCATED slice of
    /// what `stdlib_scheme` returns `Some` for.
    ///
    /// `Math.min` / `Math.max` are RELOCATED here as their *base* scheme
    /// (`a -> a -> a`); the `Comparable` obligation is layered separately in
    /// `constrain_var_kernel`, so their base is parity-checked like any
    /// other relocation while the bound still fires in production.
    const RELOCATED: &[StdlibKernel] = {
        use StdlibKernel as K;
        &[
            // String (2)
            K::StringFromInt,
            K::StringFromFloat,
            // List (10)
            K::ListMap,
            K::ListFilter,
            K::ListFoldl,
            K::ListFoldr,
            K::ListLength,
            K::ListHead,
            K::ListTail,
            K::ListMember,
            K::ListRange,
            K::ListReverse,
            // Math including min/max base (38)
            K::MathPi,
            K::MathE,
            K::MathPhi,
            K::MathSqrt2,
            K::MathInf,
            K::MathNan,
            K::MathIsNaN,
            K::MathAbs,
            K::MathSqrt,
            K::MathCbrt,
            K::MathExp,
            K::MathExp2,
            K::MathLog,
            K::MathLog2,
            K::MathLog10,
            K::MathSin,
            K::MathCos,
            K::MathTan,
            K::MathAsin,
            K::MathAcos,
            K::MathAtan,
            K::MathSinh,
            K::MathCosh,
            K::MathTanh,
            K::MathAsinh,
            K::MathAcosh,
            K::MathAtanh,
            K::MathFloor,
            K::MathCeil,
            K::MathRound,
            K::MathTrunc,
            K::MathPow,
            K::MathHypot,
            K::MathAtan2,
            K::MathMod,
            K::MathRemainder,
            K::MathMin,
            K::MathMax,
            // Maybe (3)
            K::MaybeWithDefault,
            K::MaybeMap,
            K::MaybeAndThen,
            // Result (2)
            K::ResultWithDefault,
            K::ResultMap,
            // Bytes (11)
            K::BytesEmpty,
            K::BytesLength,
            K::BytesIsEmpty,
            K::BytesFromString,
            K::BytesToString,
            K::BytesFromHex,
            K::BytesToHex,
            K::BytesFromBase64,
            K::BytesToBase64,
            K::BytesAppend,
            K::BytesSlice,
            // Task (13)
            K::TaskSucceed,
            K::TaskFail,
            K::TaskMap,
            K::TaskAndThen,
            K::TaskMapError,
            K::TaskOnError,
            K::TaskFromResult,
            K::TaskAndThenResult,
            K::TaskSequence,
            K::TaskParallel,
            K::TaskRun,
            K::TaskPerform,
            K::TaskLazy,
            K::TaskRetryWith,
            K::TaskLinearBackoff,
            K::TaskExponentialBackoff,
            K::TaskWithJitter,
            K::TaskRetryOn,
            K::TaskWithRetryOn,
            K::TaskDefaultRetryPolicy,
            K::TaskWithMaxAttempts,
            K::TaskWithBaseMs,
            K::TaskWithKind,
            // Io (3)
            K::IoReadLine,
            K::IoWriteStdout,
            K::IoWriteStderr,
            // Time (5)
            K::TimeNow,
            K::TimeUnixMillis,
            K::TimeSleep,
            K::TimeTimeString,
            K::TimeIsLeapYear,
            K::TimeDaysInMonth,
            K::TimeEvery,
            // System (11)
            K::SystemArgs,
            K::SystemGetenv,
            K::SystemGetenvOr,
            K::SystemGetArg,
            K::SystemGetenvInt,
            K::SystemGetenvBool,
            K::SystemSetenv,
            K::SystemUnsetenv,
            K::SystemCwd,
            K::SystemLoadEnv,
            K::SystemExit,
            // Random (6)
            K::RandomInt,
            K::RandomFloat,
            K::RandomChoice,
            K::RandomChoiceMaybe,
            K::RandomShuffle,
            K::RandomWeighted,
            // File (15)
            K::FileReadFile,
            K::FileWriteFile,
            K::FileExists,
            K::FileRemove,
            K::FileMkdirAll,
            K::FileReadFileLimit,
            K::FileReadFileBytes,
            K::FileAppend,
            K::FileReadDir,
            K::FileIsDir,
            K::FileTempFile,
            K::FileTempDir,
            K::FileCopy,
            K::FileRename,
            K::FileDelete,
            // Http (13)
            K::HttpGet,
            K::HttpPost,
            K::HttpRequest,
            K::HttpParseQuery,
            K::HttpDefaultRequest,
            K::HttpDefaultRequestFromString,
            K::HttpWithMethod,
            K::HttpWithTimeout,
            K::HttpWithBody,
            K::HttpWithHeader,
            K::HttpWithUrl,
            K::HttpWithFollowRedirects,
            K::HttpWithMaxRedirects,
            // Cmd (3)
            K::CmdNone,
            K::CmdBatch,
            K::CmdPerform,
            // Sub (4)
            K::SubNone,
            K::SubBatch,
            K::SubEvery,
            K::SubSubscribeTopic,
            // Middleware (5)
            K::MiddlewareWithCors,
            K::MiddlewareWithLogging,
            K::MiddlewareWithBasicAuth,
            K::MiddlewareWithRateLimit,
            K::MiddlewareWithCsrf,
            // RateLimit (1)
            K::RateLimitAllow,
            // Server (23)
            K::ServerGet,
            K::ServerPost,
            K::ServerPut,
            K::ServerDelete,
            K::ServerAny,
            K::ServerApi,
            K::ServerStatic,
            K::ServerListen,
            K::ServerText,
            K::ServerJson,
            K::ServerHtml,
            K::ServerWithStatus,
            K::ServerWithHeader,
            K::ServerRedirect,
            K::ServerParam,
            K::ServerQueryParam,
            K::ServerHeader,
            K::ServerGetCookie,
            K::ServerBody,
            K::ServerPath,
            K::ServerMethod,
            K::ServerCookieNew,
            K::ServerWithCookie,
            // Db (22 — `unsafeFindWhere` removed; its
            // replacements `findWhere`/`deleteWhere` are FIRST_SCHEMED below,
            // never having existed in the legacy `kernel_ty` table)
            K::DbConnect,
            K::DbOpen,
            K::DbClose,
            // External Connection — read-only-by-type foreign-DB connect (3)
            K::DbConnOpen,
            K::DbConnClose,
            K::DbConnUnsafeExecRawOn,
            // External read path — `…On` reads over a `Connection a` (3)
            K::DbConnFindWhere,
            K::DbConnQueryDecode,
            K::DbConnGetById,
            // Ipe.Db.Dsn — parse-don't-validate descriptor (9)
            K::DsnParse,
            K::DsnBuild,
            K::DsnDriverTag,
            K::DsnHost,
            K::DsnPort,
            K::DsnDatabase,
            K::DsnUser,
            K::DsnTlsTag,
            K::DsnRedacted,
            K::DbExecRaw,
            K::DbExec,
            K::DbQuery,
            K::DbQueryDecode,
            K::DbGetString,
            K::DbGetInt,
            K::DbGetBool,
            K::DbGetField,
            K::DbInsertRow,
            K::DbGetById,
            K::DbUpdateById,
            K::DbDeleteById,
            K::DbFindOneByField,
            K::DbFindManyByField,
            K::DbFindByConditions,
            K::DbInsertFields,
            K::DbUpdateFields,
            K::DbInsertFieldsReturning,
            K::DbWithTransaction,
            K::DbMigrate,
            K::DbDefaultMigration,
            // Db.Decode (15)
            K::DbDecString,
            K::DbDecInt,
            K::DbDecFloat,
            K::DbDecBool,
            K::DbDecNullable,
            K::DbDecMap,
            K::DbDecAndThen,
            K::DbDecSucceed,
            K::DbDecFail,
            K::DbDecMap2,
            K::DbDecMap3,
            K::DbDecMap4,
            K::DbDecRequired,
            K::DbDecOptional,
            // `DbDecMoney` is FIRST_SCHEMED, not relocated — it is Ipê-new,
            // so no byte-faithful legacy `kernel_ty` oracle ever existed for it.
            // Set (10) — base scheme; set_elem obligation layered in constrain_var_kernel
            K::SetEmpty,
            K::SetSize,
            K::SetToList,
            K::SetFromList,
            K::SetMember,
            K::SetInsert,
            K::SetRemove,
            K::SetUnion,
            K::SetIntersect,
            K::SetDiff,
            K::SetIsEmpty,
            K::SetSingleton,
            K::SetFoldl,
            K::SetFoldr,
            K::SetMap,
            K::SetFilter,
            K::SetPartition,
            // Dict (14) — base scheme; dict_key obligation layered in constrain_var_kernel
            K::DictEmpty,
            K::DictIsEmpty,
            K::DictSize,
            K::DictKeys,
            K::DictValues,
            K::DictToList,
            K::DictFromList,
            K::DictGet,
            K::DictMember,
            K::DictRemove,
            K::DictUnion,
            K::DictMap,
            K::DictInsert,
            K::DictFoldl,
            K::DictSingleton,
            K::DictFoldr,
            K::DictFilter,
            K::DictPartition,
            K::DictIntersect,
            K::DictDiff,
            K::DictUpdate,
            // Ipe.Ui layout / element / event
            K::UiLayout,
            K::UiLayoutWith,
            K::UiAbove,
            K::UiBelow,
            K::UiOnLeft,
            K::UiOnRight,
            K::UiInFront,
            K::UiBehind,
            K::UiButton,
            K::UiOnClick,
            K::UiOnFocus,
            K::UiOnBlur,
            K::UiOnMouseOver,
            K::UiOnMouseOut,
            K::UiOnInput,
            K::UiOnChange,
            K::UiOnKeyDown,
            K::UiOnKeyUp,
            K::UiOnBool,
            K::UiOnSubmit,
            // Ipe.Web app-entry (3)
            K::WebApp,
            K::WebRoute,
            K::WebRenderStatic,
            // Ipe.Terminal app-entry (1)
            K::TerminalAppScreen,
            // Ipe.WebView app-entry (1)
            K::WebViewApp,
            // Ipe.Html styleNode (1 — F7; parity checked by
            // stdlib_scheme_matches_legacy).
            K::HtmlStyleNode,
        ]
    };

    /// Families that have NO legacy scheme (`kernel_ty` → `Ty::Var(u32::MAX)`)
    /// and receive their scheme directly from their runtime + `.ipe`
    /// signatures. No parity oracle exists; correctness is pinned by
    /// `first_schemed_were_holes` (the scheme closes a genuine hole) plus the
    /// ipe→cargo build fixtures. GROWS per family; never shrinks.
    ///
    /// Notable members:
    /// - Crypto AEAD (`aesGcm*`/`chacha20*`) and Jwt ENCODE
    ///   (`encodeHs256`/`encodeRs256`): registry `decl().arity` is 2 to match
    ///   the Rust runtime (the AEAD nonce is internal; encode takes secret +
    ///   claims-JSON), so the arrow-count == arity invariant holds.
    /// - Ipe.Ui `Length` builders (`px`/`fill`/`content`/`shrink`/
    ///   `fillPortion`/`vh`/`vw`/`minimum`/`maximum`), Ipe.Ui `Color` builders
    ///   (`rgb`/`rgba`/`white`/`black`/`transparent`), and the
    ///   `Ipe.Json.Encode` encoders: `Length` / `Color` lower to
    ///   `IrType::UiPlain(_)` and the JSON `Value` type to `IrType::Json`.
    /// - `Ipe.Uuid` (`v4`/`v7` as `() -> Task Error String` — entropy is
    ///   an effect; `parse` as the pure `String -> Maybe String` parser).
    /// - The `List` combinators and the `Encoding` codecs (UTF-8 text path,
    ///   Go parity).
    /// - `PubSub.publish` / `PubSub.publishNoEcho`
    ///   (`String -> a -> Task Error Int`): the runtime `pubsub_publish` /
    ///   `pubsub_publish_no_echo` exist; the emit arm emits
    ///   `pubsub_publish::<_, IpeError>(topic, payload)`. `KNOWN_UNBACKED` is
    ///   empty.
    const FIRST_SCHEMED: &[StdlibKernel] = {
        use StdlibKernel as K;
        &[
            // Http method ADT accessors (Ipê-new, no legacy oracle).
            K::HttpMethodFromString,
            K::HttpMethodToString,
            // String (33 — beyond the relocated `fromInt`/`fromFloat`)
            K::StringLength,
            K::StringIsEmpty,
            K::StringReverse,
            K::StringToUpper,
            K::StringToLower,
            K::StringCasefold,
            K::StringTrim,
            K::StringTrimStart,
            K::StringTrimEnd,
            K::StringToInt,
            K::StringToFloat,
            K::StringFromChar,
            K::StringFromList,
            K::StringConcat,
            K::StringWords,
            K::StringLines,
            K::StringToList,
            K::StringIsEmail,
            K::StringIsUrl,
            K::StringAppend,
            K::StringContains,
            K::StringStartsWith,
            K::StringEndsWith,
            K::StringEqualFold,
            K::StringJoin,
            K::StringSplit,
            K::StringRepeat,
            K::StringDropLeft,
            K::StringDropRight,
            K::StringReplace,
            K::StringSlice,
            K::StringPadLeft,
            K::StringPadRight,
            K::StringContainsIn,
            K::StringStartsWithIn,
            K::StringEndsWithIn,
            K::StringLeft,
            K::StringRight,
            K::StringCons,
            K::StringUncons,
            K::StringPad,
            K::StringIndexes,
            K::StringMap,
            K::StringFilter,
            K::StringFoldl,
            K::StringFoldr,
            K::StringAny,
            K::StringAll,
            // Char (8)
            K::CharIsAlpha,
            K::CharIsDigit,
            K::CharIsLower,
            K::CharIsUpper,
            K::CharToLower,
            K::CharToUpper,
            K::CharToCode,
            K::CharFromCode,
            K::CharIsAlphaNum,
            K::CharIsHexDigit,
            K::CharIsOctDigit,
            // Error (18 — Ipe.Error real `Error ErrorKind ErrorInfo` ADT:
            // constructors, modifiers, render, classification, inspectors)
            K::ErrorUnexpected,
            K::ErrorInvalidInput,
            K::ErrorIo,
            K::ErrorNetwork,
            K::ErrorFfi,
            K::ErrorDecode,
            K::ErrorConflict,
            K::ErrorUnavailable,
            K::ErrorTimeout,
            K::ErrorNotFound,
            K::ErrorPermissionDenied,
            K::ErrorToString,
            K::ErrorWithMessage,
            K::ErrorIsRetryable,
            K::ErrorWithDetails,
            K::ErrorKind,
            K::ErrorMessage,
            K::ErrorKindName,
            // CssSafety (4 — Ipe.Css leaf security kernels). Each is a hole
            // (`kernel_ty` has no CssSafety arm → `Ty::Var(u32::MAX)`) unless
            // schemed above; the three parsers are `String -> Maybe String`,
            // `stripStyleClose` is `String -> String`.
            K::CssSafetySafeValue,
            K::CssSafetySafePropName,
            K::CssSafetySafeSelector,
            K::CssSafetyStripStyleClose,
            // Crypto (17 — AEAD included after the arity 3→2 correction)
            K::CryptoSha256,
            K::CryptoSha512,
            K::CryptoSha1,
            K::CryptoMd5,
            K::CryptoHmacSha256,
            K::CryptoHmacSha512,
            K::CryptoRsaSha256Sign,
            K::CryptoRsaSha256Verify,
            K::CryptoConstantTimeEqual,
            K::CryptoAesKeyFromPassword,
            K::CryptoChachaKeyFromPassword,
            K::CryptoAesGcmEncrypt,
            K::CryptoAesGcmDecrypt,
            K::CryptoChacha20Encrypt,
            K::CryptoChacha20Decrypt,
            K::CryptoRandomBytes,
            K::CryptoRandomToken,
            // Jwt (4 — encode included after the arity 3→2 correction)
            K::JwtDecodeHs256,
            K::JwtDecodeRs256,
            K::JwtEncodeHs256,
            K::JwtEncodeRs256,
            // Jwt builder API (13 — D-00): `claims` / `hs256` / `rs256` /
            // `subject` / `issuer` / `audience` / `expiresAt` / `notBefore` /
            // `issuedAt` / `jwtId` / `withClaim` / `encode` / `decode`.
            // All genuine holes (no legacy `kernel_ty` arm).
            K::JwtClaims,
            K::JwtHs256,
            K::JwtRs256,
            K::JwtSubject,
            K::JwtIssuer,
            K::JwtAudience,
            K::JwtExpiresAt,
            K::JwtNotBefore,
            K::JwtIssuedAt,
            K::JwtJwtId,
            K::JwtWithClaim,
            K::JwtEncode,
            K::JwtDecode,
            // Json.Decode (17)
            K::JsonDecString,
            K::JsonDecInt,
            K::JsonDecFloat,
            K::JsonDecBool,
            K::JsonDecDecodeString,
            K::JsonDecField,
            K::JsonDecAt,
            K::JsonDecIndex,
            K::JsonDecList,
            K::JsonDecMap,
            K::JsonDecAndThen,
            K::JsonDecSucceed,
            K::JsonDecFail,
            K::JsonDecOneOf,
            K::JsonDecMap2,
            K::JsonDecMap3,
            K::JsonDecMap4,
            // Json.Decode.Pipeline (4)
            K::JsonDecPRequired,
            K::JsonDecPOptional,
            K::JsonDecPCustom,
            K::JsonDecPRequiredAt,
            // Result internal okDefault (1)
            K::ResultOkDefault,
            // Ipe.Ui Length builders (9) — result type `Length`
            K::UiPx,
            K::UiFill,
            K::UiContent,
            K::UiShrink,
            K::UiFillPortion,
            K::UiVh,
            K::UiVw,
            K::UiMinimum,
            K::UiMaximum,
            // Ipe.Ui Color builders (6) — result type `Color`
            K::UiRgb,
            K::UiRgba,
            K::UiWhite,
            K::UiBlack,
            K::UiTransparent,
            K::UiColorCss,
            // Ipe.Json.Encode (8) — `Value` positions map to `IrType::Json`
            K::JsonEncString,
            K::JsonEncInt,
            K::JsonEncFloat,
            K::JsonEncBool,
            K::JsonEncNull,
            K::JsonEncList,
            K::JsonEncObject,
            K::JsonEncEncode,
            // Ipe.Json.Decode (2) — the in-memory `Value` seam: `value` (the
            // identity `Decoder Value`) and `decodeValue` (run a decoder against
            // a `Value`). Both are Ipê-new with no legacy oracle; `Value`
            // positions map to `IrType::Json`.
            K::JsonDecValue,
            K::JsonDecDecodeValue,
            // Uuid (3): `v4`/`v7` are `() -> Task Error String`
            // (entropy is an effect, not a memoizable pure String); `parse` is
            // the pure `String -> Maybe String` parser. Each is a hole
            // (`kernel_ty` has no Uuid arm → `Ty::Var(u32::MAX)`), confirmed by
            // `first_schemed_were_holes`.
            K::UuidV4,
            K::UuidV7,
            K::UuidParse,
            // List (9): the non-HOF combinators `append`/`concat`/
            // `take`/`drop`/`zip`/`cons`/`isEmpty` plus the two HOFs
            // `concatMap`/`indexedMap`. Canon anchored every `List.x` to
            // `VarHome::Kernel`, but only 10 had a `KernelFn`+scheme — these nine
            // were holes (`kernel_ty` had no arm → `Ty::Var(u32::MAX)`) and
            // emitted IPE-L0108 at lower. Now schemed from their runtime + `.ipe`
            // signatures; confirmed holes by `first_schemed_were_holes`.
            K::ListAppend,
            K::ListConcat,
            K::ListTake,
            K::ListDrop,
            K::ListZip,
            K::ListCons,
            K::ListIsEmpty,
            K::ListConcatMap,
            K::ListIndexedMap,
            // List HOFs any/all/find (3).
            K::ListAny,
            K::ListAll,
            K::ListFind,
            // List filterMap/sortBy (2).
            K::ListFilterMap,
            K::ListSortBy,
            K::ListSort,
            K::ListSortWith,
            K::ListSingleton,
            K::ListRepeat,
            K::ListSum,
            K::ListProduct,
            K::ListMaximum,
            K::ListMinimum,
            K::ListUnique,
            K::ListIntersperse,
            K::ListPartition,
            K::ListUnzip,
            K::ListMap2,
            K::ListMap3,
            K::ListMap4,
            K::ListMap5,
            // Basics core Prelude (6 — slice).
            K::BasicsNot,
            K::BasicsIdentity,
            K::BasicsAlways,
            K::BasicsFst,
            K::BasicsSnd,
            K::BasicsModBy,
            // Log info/debug/warn/error (4 — slice).
            K::LogInfo,
            K::LogDebug,
            K::LogWarn,
            K::LogError,
            // Log *With (4 — Stringify obligation on the attr list element).
            K::LogInfoWith,
            K::LogDebugWith,
            K::LogWarnWith,
            K::LogErrorWith,
            // Io line-printers (Ipê-new — no legacy oracle).
            K::IoPrintln,
            K::IoEprintln,
            // Io echo-suppressed line read (Ipê-new — no legacy oracle).
            K::IoReadSecret,
            // Debug.log (Ipê-new — dev-only; Stringify obligation on `a`).
            K::DebugLog,
            // `Basics.clamp` — first-schemed hole; carries the `Comparable a`
            // (Ord) obligation, base scheme in `stdlib_scheme`.
            K::BasicsClamp,
            K::BasicsToString,
            // ── Basics numerics — negate/abs/sqrt/min/max ────────────
            K::BasicsNegate,
            K::BasicsAbs,
            K::BasicsSqrt,
            K::BasicsMin,
            K::BasicsMax,
            K::BasicsCompare,
            // ── end Basics numerics ──────────────────────────────────
            // Bitwise — Ipê-new (no legacy oracle); Int-only, runtime fns in
            // `bitwise.rs`.
            K::BitwiseAnd,
            K::BitwiseOr,
            K::BitwiseXor,
            K::BitwiseComplement,
            K::BitwiseShiftLeftBy,
            K::BitwiseShiftRightBy,
            K::BitwiseShiftRightZfBy,
            // Random seeded (Generator primitives) — pure/reproducible draws in
            // `random.rs` (`random_seeded_int`/`random_seeded_float`/
            // `random_seeded_choice`).
            K::RandomSeededInt,
            K::RandomSeededFloat,
            K::RandomSeededChoice,
            // Result combinators that are first-schemed holes; `withDefault` /
            // `map` are the RELOCATED pair, these two are first-schemed.
            K::ResultAndThen,
            K::ResultMapError,
            // Result / Maybe applicative combinators (mapN / andMap /
            // combine / traverse). All genuine holes (no legacy `kernel_ty`
            // arm); runtime fns in `core.rs` (`result_map2` .. `result_traverse`,
            // `maybe_map2` .. `maybe_combine`; `result_traverse` pre-existed).
            K::ResultMap2,
            K::ResultMap3,
            K::ResultMap4,
            K::ResultMap5,
            K::ResultAndMap,
            K::ResultCombine,
            K::ResultTraverse,
            K::ResultToMaybe,
            K::ResultFromMaybe,
            K::MaybeMap2,
            K::MaybeMap3,
            K::MaybeMap4,
            K::MaybeMap5,
            K::MaybeAndMap,
            K::MaybeCombine,
            K::MaybeIsJust,
            K::MaybeIsNothing,
            // Encoding (6): base64/url/hex text codecs. Encoders
            // `String -> String`, decoders `String -> Result Error String`.
            // Each is a `Ty::Var(u32::MAX)` hole (`kernel_ty` has no Encoding
            // arm), confirmed by `first_schemed_were_holes`. The runtime text
            // path is UTF-8 (Go parity); byte round-tripping lives in
            // `Ipe.Bytes`.
            K::EncodingBase64Encode,
            K::EncodingBase64Decode,
            K::EncodingUrlEncode,
            K::EncodingUrlDecode,
            K::EncodingHexEncode,
            K::EncodingHexDecode,
            // Ipe.Html / Ipe.Ui / Ipe.Web rendering family (42).
            // All genuine `Ty::Var(u32::MAX)` holes (legacy `kernel_ty` has no
            // Html/Ui/Background/Border/Font arm). Verified vs runtime + lower
            // `callee_arity` in docs/adr/0020-html-ui-live-kernel-arity-tripwire.md.
            // `WebAppRouted` is EXCLUDED here — it is `REACHABLE_BUT_UNLOWERED`.
            K::HtmlRender,
            K::HtmlEscapeText,
            K::HtmlEscapeAttr,
            K::HtmlAttrToString,
            K::UiNone,
            K::UiText,
            K::UiHtml,
            K::UiCells,
            // The container / tagged-element primitives (first-schemed — no
            // legacy). The layout / flow builders are pure Ipê over them.
            K::UiNode,
            K::UiTaggedNode,
            K::UiSpacing,
            K::UiPadding,
            K::UiPaddingXY,
            K::UiWidth,
            K::UiHeight,
            K::UiCenterX,
            K::UiCenterY,
            K::UiAlignLeft,
            K::UiAlignRight,
            K::UiAlignTop,
            K::UiAlignBottom,
            K::UiPointer,
            K::UiClip,
            K::UiScrollbars,
            K::UiGridColumns,
            K::BackgroundColor,
            K::BackgroundImage,
            K::BorderWidth,
            K::BorderRounded,
            K::BorderColor,
            K::FontSize,
            K::FontColor,
            K::FontFamily,
            K::FontBold,
            K::FontItalic,
            K::HtmlTextNode,
            K::HtmlRawNode,
            K::HtmlNode,
            // Ipe.Html.Attributes retained primitives (first-schemed — no legacy).
            K::HtmlAttribute,
            K::HtmlBoolAttribute,
            K::HtmlNoAttr,
            // Ipe.Html.Events builders (first-schemed — no legacy).
            K::HtmlOnClick,
            K::HtmlOnFocus,
            K::HtmlOnBlur,
            K::HtmlOnMouseOver,
            K::HtmlOnMouseOut,
            K::HtmlOnSubmit,
            K::HtmlOnInput,
            K::HtmlOnChange,
            K::HtmlOnKeyDown,
            K::HtmlOnKeyUp,
            K::HtmlOnBool,
            // `Html.Unsafe.unsafeScript` — Ipê-new inline-`<script>` escape hatch,
            // no legacy oracle, so it is FIRST_SCHEMED (schemed in `stdlib_scheme`).
            K::HtmlScriptNode,
            // `CssSafety.sanitizeRawBody` — Ipê-new raw/keyframes-body gate over
            // the audited `css_safety` policy (`css_unescape` + whitespace-strip),
            // no legacy oracle, so it is FIRST_SCHEMED (schemed in `stdlib_scheme`).
            K::CssSafetySanitizeRawBody,
            // NB: HtmlStyleNode is NOT here — it is RELOCATED (`Html.styleNode`
            // is schemed in the legacy `kernel_ty` table, F7), so its parity is
            // checked by `stdlib_scheme_matches_legacy`.
            // ── Tier 1: extended Ipe.Ui / Font / Background / Border builders ──
            K::UiSquare,
            K::UiWidescreen,
            K::UiCinemascope,
            K::UiAspectRatio,
            K::UiAspectRatioWH,
            K::UiHtmlAttribute,
            K::UiName,
            K::UiStyle,
            K::UiTransitionRaw,
            K::UiGridTracksRaw,
            K::UiAnimateRaw,
            // Breakpoint
            K::UiBreakpoint,
            // `Ui.mediaQuery` — routes through the `style_inject::build_mq`
            // consumer.
            K::UiMediaQuery,
            K::UiMobile,
            K::UiTablet,
            K::UiDesktop,
            K::UiDarkMode,
            K::UiLightMode,
            K::UiReducedMotion,
            K::BackgroundHoverColor,
            K::BackgroundFocusColor,
            K::BackgroundActiveColor,
            K::BackgroundDisabledColor,
            K::BorderSolid,
            K::BorderDashed,
            K::BorderDotted,
            K::BorderHoverColor,
            K::BorderFocusColor,
            K::BorderActiveColor,
            K::BorderHoverWidth,
            K::BorderHoverRounded,
            K::FontWeight,
            K::FontSemiBold,
            K::FontRegular,
            K::FontLight,
            K::FontExtraBold,
            K::FontBlack,
            K::FontUnderline,
            K::FontNoDecoration,
            K::FontLineThrough,
            K::FontLetterSpacing,
            K::FontWordSpacing,
            K::FontAlignLeft,
            K::FontAlignRight,
            K::FontAlignCenter,
            K::FontCenter,
            K::FontJustify,
            K::FontSansSerif,
            K::FontSerif,
            K::FontMonospace,
            K::FontHoverColor,
            K::FontFocusColor,
            K::FontActiveColor,
            K::FontDisabledColor,
            K::FontHoverSize,
            // Ipe.Terminal line-oriented app-entry.
            K::TerminalAppLines,
            // ── Ipe.Auth (9 kernels) — schemed + lowered, moved from REACHABLE_BUT_UNLOWERED ──
            K::AuthHashPassword,
            K::AuthHashPasswordCost,
            K::AuthVerifyPassword,
            K::AuthPasswordStrength,
            K::AuthSignToken,
            K::AuthVerifyToken,
            K::AuthRegister,
            K::AuthLogin,
            K::AuthSetRole,
            // ── Ipe.Http.Server.Stream (4 kernels) ─────────────────────────
            K::StreamStream,
            K::StreamEmit,
            K::StreamFinish,
            K::StreamWithContentType,
            // ── Ipe.Http.Stream (4 kernels) ───────────────────────────
            K::HttpStreamOpen,
            K::HttpStreamForEachChunk,
            K::HttpStreamClose,
            K::HttpStreamChunks,
            // ── Ipe.Http.Server.WebSocket (12 kernels) ─────────────────────
            K::WsDefaultCfg,
            K::WsWithOnConnect,
            K::WsWithOnMessage,
            K::WsWithOnClose,
            K::WsWithOnError,
            K::WsWithMaxMessageBytes,
            K::WsWithOriginPatterns,
            K::WsUpgrade,
            K::WsSendToClient,
            K::WsSendBinaryToClient,
            K::WsBroadcast,
            K::WsCloseClient,
            // ── Ipe.WebSocket — outbound WebSocket client (7 kernels) ──
            K::WebSocketConnect,
            K::WebSocketConnectWith,
            K::WebSocketSend,
            K::WebSocketSendBinary,
            K::WebSocketClose,
            K::WebSocketCloseWithCode,
            K::SubSubscribeWebSocket,
            // ── Ipe.Process — subprocess execution (no shell) ──
            K::ProcessRun,
            // ── Ipe.Env — build-time-embedded public config ──
            K::EnvPublic,
            // ── Ipe.Ui.Region — all 8 landmark/live-region attrs ──
            K::RegionMainContent,
            K::RegionNavigation,
            K::RegionFooter,
            K::RegionAside,
            K::RegionHeading,
            K::RegionLabel,
            K::RegionAnnounce,
            K::RegionAnnounceUrgently,
            // ── Ui.describe + desc* batch ──
            K::UiDescribe,
            K::UiDescNone,
            K::UiDescParagraph,
            K::UiDescMain,
            K::UiDescNavigation,
            K::UiDescContentInfo,
            K::UiDescComplementary,
            K::UiDescLivePolite,
            K::UiDescLiveAssertive,
            K::UiDescHeading,
            K::UiDescLabel,
            // ── Ipe.Ui.Input ───────────────────────────────────────────
            K::InputLabelAbove,
            K::InputLabelBelow,
            K::InputLabelLeft,
            K::InputLabelRight,
            K::InputLabelHidden,
            K::InputPlaceholder,
            K::InputText,
            K::InputMultiline,
            K::InputEmail,
            K::InputUsername,
            K::InputSearch,
            K::InputCurrentPassword,
            K::InputNewPassword,
            K::InputCheckbox,
            K::InputSlider,
            K::InputOption,
            K::InputRadio,
            K::InputRadioRow,
            // ── Ipe.Ui.Lazy ────────────────────────────────────────────
            K::LazyLazy,
            K::LazyLazy2,
            K::LazyLazy3,
            K::LazyLazy4,
            K::LazyLazy5,
            // ── TEA pub/sub: Cmd.publish / Cmd.publishNoEcho ──────────────
            // Genuine holes — no legacy `kernel_ty` arm. `"publish"` /
            // `"publishNoEcho"` are registered in canon QUALIFIERS ("Cmd"
            // entry) and flow through lower + emit.
            K::CmdPublish,
            K::CmdPublishNoEcho,
            // ── PubSub.topic — Ipê-new typed-topic constructor (`String -> Topic
            // a`); no legacy `kernel_ty` arm. Erases to the name String at emit. ─
            K::PubSubTopic,
            // ── Ui.link + Border.widthEach ────────────────────────────────────
            // No legacy `kernel_ty` entry — pure holes.
            K::UiLink,
            K::BorderWidthEach,
            K::BorderShadow,
            K::BorderGlow,
            K::BorderInnerShadow,
            // ── 20 Ipe.Ui / Ipe.Html / Background kernels — the
            // exhaustiveness gate list. No legacy `kernel_ty` entry — pure
            // holes.
            K::UiImage,
            K::UiPaddingEach,
            K::UiClipX,
            K::UiClipY,
            K::UiScrollbarX,
            K::UiScrollbarY,
            K::UiOnFile,
            K::HtmlToString,
            K::HtmlVoidNode,
            K::HtmlDoctype,
            K::HtmlTitleNode,
            K::BackgroundLinearGradient,
            K::UiOnPseudo,
            K::UiHover,
            K::UiFocus,
            K::UiFocusVisible,
            K::UiActive,
            K::UiDisabled,
            // ── Ipe.Ui.Keyed (column + row) ──────────────────────────────────
            K::KeyedColumn,
            K::KeyedRow,
            // ── Ipe.Decimal (40 kernels) ──────────────────────────────────────
            K::DecZero,
            K::DecOne,
            K::DecOneHundred,
            K::DecFromString,
            K::DecFromInt,
            K::DecFromFloat,
            K::DecFromMinor,
            K::DecToString,
            K::DecToStringFixed,
            K::DecToFloat,
            K::DecToInt,
            K::DecToMinor,
            K::DecAdd,
            K::DecSub,
            K::DecMul,
            K::DecDiv,
            K::DecMod,
            K::DecNeg,
            K::DecAbs,
            K::DecFloor,
            K::DecCeil,
            K::DecRound,
            K::DecRoundHalfUp,
            K::DecTruncate,
            K::DecCompare,
            K::DecEq,
            K::DecNeq,
            K::DecLt,
            K::DecLte,
            K::DecGt,
            K::DecGte,
            K::DecMin,
            K::DecMax,
            K::DecIsZero,
            K::DecIsPositive,
            K::DecIsNegative,
            K::DecPercentOf,
            K::DecAddPercent,
            K::DecSubPercent,
            K::DecFormatWith,
            // ── Ipe.Money (11) ─────────────────────────────────────────────────
            K::MoneyMinorUnits,
            K::MoneySymbol,
            K::MoneyCurrencyName,
            K::MoneyIsKnownCurrency,
            K::MoneyFormat,
            K::MoneyFormatWithCode,
            K::MoneyAllocate,
            K::MoneySetRate,
            K::MoneyGetRate,
            K::MoneyHasRate,
            K::MoneyClearRates,
            // ── Ipe.Db.Sql — SqlFragment builder (20) ──────────────
            K::SqlColumn,
            // Ipe.Db.Unsafe.unsafeFragment — the un-validated anti-`Sql.column`.
            K::SqlUnsafeFragment,
            K::SqlParam,
            K::SqlInt,
            K::SqlString,
            K::SqlFloat,
            K::SqlBool,
            K::SqlEq,
            K::SqlNe,
            K::SqlGt,
            K::SqlLt,
            K::SqlGte,
            K::SqlLte,
            K::SqlAnd,
            K::SqlOr,
            K::SqlNot,
            K::SqlIsNull,
            K::SqlIsNotNull,
            K::SqlInList,
            K::SqlLike,
            K::DbFindWhere,
            K::DbDeleteWhere,
            K::DbUpdateWhere,
            // `Db.Decode.money` and `Db.Decode.bytes` — Ipê-NEW kernels (the
            // ancestor has no DbDec money/bytes routes), so they close genuine
            // holes rather than relocating legacy `kernel_ty` schemes. Their
            // DbDec siblings are RELOCATED; these are deliberately not.
            K::DbDecMoney,
            K::DbDecBytes,
            // ── Ipe.Secret (4) ─────────────────────────────
            K::SecretFromString,
            K::SecretReveal,
            // `Secret.use : Secret -> (String -> a) -> a` — Ipê-new scoped
            // consume (no legacy oracle); the polymorphic higher-order arm.
            K::SecretUse,
            K::SecretRedacted,
            // ── Ipe.Regex (6) ─────────────────────────────────────
            K::RegexCompile,
            K::RegexMatch,
            K::RegexFind,
            K::RegexFindAll,
            K::RegexReplace,
            K::RegexSplit,
            // ── Ipe.Path (6) ──────────────────────────────────────
            K::PathFromString,
            K::PathToString,
            K::PathBase,
            K::PathDir,
            K::PathExt,
            K::PathIsAbsolute,
            // ── Ipe.Trace (3) ──────────────────────────────────────────
            K::TraceSpan,
            K::TraceEvent,
            K::TraceAttr,
            // ── Ipe.Compression (4) ────────────────────────────────────
            K::CompressionGzip,
            K::CompressionGunzip,
            K::CompressionZstdCompress,
            K::CompressionZstdDecompress,
            // ── Ipe.Csv (5) ────────────────────────────────────────────
            K::CsvParse,
            K::CsvParseWithDelimiter,
            K::CsvEncode,
            K::CsvEncodeWithDelimiter,
            K::CsvParseStreamFromFile,
            // ── Ipe.Cache (7) ──────────────────────────────────────────
            K::CacheNewRaw,
            K::CacheGet,
            K::CachePut,
            K::CacheRemove,
            K::CacheClear,
            K::CacheSize,
            K::CacheStats,
            // ── Ipe.Config (16) ────────────────────────────────────
            K::ConfigString,
            K::ConfigInt,
            K::ConfigFloat,
            K::ConfigBool,
            K::ConfigNullable,
            K::ConfigField,
            K::ConfigAt,
            K::ConfigList,
            K::ConfigSucceed,
            K::ConfigFail,
            K::ConfigMap,
            K::ConfigAndThen,
            K::ConfigMap2,
            K::ConfigMap3,
            K::ConfigMap4,
            K::ConfigMap5,
            K::ConfigMap6,
            K::ConfigMap7,
            K::ConfigMap8,
            K::ConfigOneOf,
            K::ConfigIndex,
            K::ConfigKeyValuePairs,
            K::ConfigMaybe,
            K::ConfigDict,
            K::ConfigDecodeToml,
            K::ConfigDecodeYaml,
            K::ConfigDecodeJson,
            K::ConfigLoadFromFile,
            // ── Ipe.Email (1) ──────────────────────────────────────────
            K::EmailSend,
            // ── Ipe.Crypto typed-key newtypes (11) ─────────────────────
            K::CryptoKeyFromString,
            K::CryptoKeyFromBytes,
            K::CryptoMacToHex,
            K::CryptoHmacSha256WithKey,
            K::CryptoHmacSha512WithKey,
            K::CryptoAesKeyFromPasswordKey,
            K::CryptoChachaKeyFromPasswordKey,
            K::CryptoAesGcmEncryptKey,
            K::CryptoAesGcmDecryptKey,
            K::CryptoChacha20EncryptKey,
            K::CryptoChacha20DecryptKey,
            // ── Ipe.Email.EmailAddress (2) ──────────────────────────────
            K::EmailAddressParse,
            K::EmailAddressToString,
            // ── Ipe.Url (9) ─────────────────────────────────────────
            K::UrlFromString,
            K::UrlToString,
            K::UrlScheme,
            K::UrlHost,
            K::UrlPort,
            K::UrlPath,
            K::UrlQuery,
            K::UrlFragment,
            K::UrlBuildQuery,
            // ── Ipe.Locale (4) ──────────────────────────────────────────
            K::LocaleFromTag,
            K::LocaleToTag,
            K::StringToUpperIn,
            K::StringToLowerIn,
            // ── Ipe.PubSub (2) ─────────────────────────────────────
            // Runtime exists, emit arm present (`pubsub_publish::<_, IpeError>`),
            // scheme `String -> a -> Task Error Int`.
            K::PubSubPublish,
            K::PubSubPublishNoEcho,
            // ── TEA: Cmd.map / Sub.map (2) ─────────────────────────
            // Ipê-new (no legacy oracle): `(a -> msg) -> Cmd a -> Cmd msg` and
            // the `Sub` twin. Runtime `cmd_map` / `sub_map`, emit arm in
            // `emit_tea_call`.
            K::CmdMap,
            K::SubMap,
            // ── Task combinators map2..5 + attempt (Ipê-new) ───────
            // `map2..5` combine independent tasks; `attempt` bridges a Task into
            // a Cmd (emit arm in `emit_tea_call`, runtime `cmd_perform`).
            K::TaskMap2,
            K::TaskMap3,
            K::TaskMap4,
            K::TaskMap5,
            K::TaskAttempt,
        ]
    };

    /// REACHABLE-BUT-UNLOWERED kernels: they HAVE a runtime fn AND a canon
    /// qualifier (so a user program can name them — distinct from
    /// `KNOWN_UNBACKED`, which has no runtime fn), but their LOWERING is not yet
    /// implemented, so `stdlib_scheme` intentionally leaves them un-schemed and a
    /// caller fails closed. `Web.appRouted` lowering is `Feature::RoutedWebApp`
    /// unsupported (`lower.rs`); its type is a closed config record, not a simple
    /// curried `Ty`. When routed-live lowering lands it moves to `FIRST_SCHEMED`
    /// with the dedicated `Ty::Record` arm (design table Option A). Excluded from
    /// `stdlib_scheme_total_over_reachable` until then.
    const REACHABLE_BUT_UNLOWERED: &[StdlibKernel] = {
        use StdlibKernel as K;
        &[K::WebAppRouted]
    };

    /// KNOWN-UNBACKED kernels: present in `StdlibKernel::ALL` (so they carry a
    /// registry index) but deliberately NEVER schemed. Currently **empty** —
    /// `PubSub.publish`/`PubSub.publishNoEcho` (the only
    /// previous occupants) were promoted to `FIRST_SCHEMED` once their runtime
    /// functions and emit arm were confirmed present. The bucket exists
    /// structurally so the `known_unbacked_never_schemed` gate still compiles
    /// (it iterates the slice, which is now a vacuous pass) and future
    /// deliberately-unschemed kernels have a named home. Do NOT scheme a kernel
    /// into `FIRST_SCHEMED` before its runtime function and emit arm exist —
    /// that forges an exit-0 path to an unbacked kernel (SEAL violation).
    /// Enforced by `known_unbacked_never_schemed`.
    const KNOWN_UNBACKED: &[StdlibKernel] = {
        #[allow(unused_imports)]
        use StdlibKernel as K;
        &[]
    };

    /// KNOWN-UNBACKED kernels are in `ALL`, are disjoint from the migrated
    /// sets, and `stdlib_scheme` returns `None` for them. Pins the deliberate
    /// unbacked exclusion so a future accidental scheme (an exit-0 path to a
    /// non-existent runtime fn) fails loudly here.
    #[test]
    fn known_unbacked_never_schemed() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        for &k in KNOWN_UNBACKED {
            assert!(
                StdlibKernel::ALL.contains(&k),
                "{k:?} must be in ALL to carry a registry index",
            );
            assert!(
                !RELOCATED.contains(&k) && !FIRST_SCHEMED.contains(&k),
                "{k:?} is KNOWN-UNBACKED and must not be in RELOCATED/FIRST_SCHEMED",
            );
            assert!(
                builder.stdlib_scheme(k).is_none(),
                "{k:?} is KNOWN-UNBACKED (no runtime fn, qualifier not in \
                 qual_vars) and must NOT be schemed — a scheme forges an exit-0 \
                 path to an unbacked kernel.",
            );
        }

        // REACHABLE_BUT_UNLOWERED is a bounded escape hatch, not a dumping
        // ground: each entry must be in ALL, return `None` (un-schemed, fails
        // closed for callers), and be disjoint from the other three buckets.
        for &k in REACHABLE_BUT_UNLOWERED {
            assert!(
                StdlibKernel::ALL.contains(&k),
                "{k:?} must be in ALL to carry a registry index",
            );
            assert!(
                builder.stdlib_scheme(k).is_none(),
                "{k:?} is REACHABLE_BUT_UNLOWERED and must NOT be schemed until \
                 its lowering lands (a caller must fail closed, not type-check).",
            );
            assert!(
                !RELOCATED.contains(&k)
                    && !FIRST_SCHEMED.contains(&k)
                    && !KNOWN_UNBACKED.contains(&k),
                "{k:?} is REACHABLE_BUT_UNLOWERED and must be disjoint from the \
                 other classification buckets.",
            );
        }
    }

    /// Build a scheme-test `Builder` plus the pre-interned `(qualifier, name)`
    /// symbol for every `StdlibKernel::ALL` variant, in lockstep order.
    ///
    /// Returns the interner + uf by value so the caller owns them for the
    /// `Builder` borrow (the closure-free layout keeps the borrow-checker happy
    /// without `unsafe`).
    fn make_builder(interner: &mut Interner) -> Builtins {
        Builtins::new(interner).expect("Builtins::new must not fail in tests")
    }

    // `kernel_ty` is deleted, so a two-source `stdlib_scheme_matches_legacy`
    // parity check is structurally impossible. `migrated_set_burndown` pins the
    // exact Some set (RELOCATED ∪ FIRST_SCHEMED ⟺ Some), which subsumes both
    // "every RELOCATED kernel is Some" and "every Some scheme is classified
    // RELOCATED or FIRST_SCHEMED". The `first_schemed_were_holes` test below
    // holds the classify guard (RELOCATED ∩ FIRST_SCHEMED = ∅), and the golden
    // suite exercises each RELOCATED scheme's emit.

    /// A was-a-hole oracle (`FIRST_SCHEMED` kernel had NO legacy scheme,
    /// `kernel_ty` → the un-typed sentinel) is not checkable — the legacy
    /// table is deleted. What stays checkable is that `FIRST_SCHEMED` and
    /// `RELOCATED` are DISJOINT (a scheme is a hole XOR a relocation, never
    /// both) and that every `FIRST_SCHEMED` kernel is actually schemed.
    /// Disjointness is NOT implied by `migrated_set_burndown` (an overlapping
    /// kernel still satisfies the union membership), so this is a genuine
    /// independent guard.
    #[test]
    fn first_schemed_were_holes() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_table(&mut uf, &interner, builtins);
        for &k in FIRST_SCHEMED {
            assert!(
                !RELOCATED.contains(&k),
                "FIRST_SCHEMED {k:?} is ALSO in RELOCATED — a kernel is a hole \
                 XOR a relocation, never both; classify it into exactly one \
                 bucket.",
            );
            assert!(
                builder.resolve_scheme(k.def().scheme).is_some(),
                "FIRST_SCHEMED {k:?} does not resolve to a scheme — a \
                 first-schemed kernel must actually be schemed (via its table arm \
                 or, once migrated, its structural `TyShape`).",
            );
        }
    }

    /// The interned `RetryPolicy` field symbols must resolve to exactly the
    /// shared `RETRY_POLICY_FIELDS` set — that const is the single source of
    /// truth the lowering gate matches against, so any drift between the two
    /// (a renamed field, an added/removed field) is a build error here rather
    /// than a silent gate mismatch (an over- or under-broad exemption).
    #[test]
    fn retry_policy_field_symbols_match_ssot() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut field_names: Vec<&str> = [
            builtins.retry_f_base_ms,
            builtins.retry_f_jitter,
            builtins.retry_f_kind,
            builtins.retry_f_max_attempts,
            builtins.retry_f_should_retry,
        ]
        .into_iter()
        .filter_map(|s| interner.resolve(s))
        .collect();
        field_names.sort_unstable();
        let mut expected: Vec<&str> = crate::RETRY_POLICY_FIELDS.to_vec();
        expected.sort_unstable();
        assert_eq!(
            field_names, expected,
            "the interned RetryPolicy field symbols drifted from \
             RETRY_POLICY_FIELDS; update the shared const and the lowering gate \
             together.",
        );
    }

    /// The field-name → required-type mapping encoded in `is_retry_policy_record`
    /// (in `ipe_lower`) must stay in sync with the kernel scheme field types.
    /// This test pins the interned type-name strings the scheme uses for each
    /// field so a future rename of `Int`/`Bool` or a field-type change in the
    /// kernel scheme that is not reflected in the lowering predicate becomes a
    /// red test rather than a silent SEAL hole.
    ///
    /// The lowering predicate maps:
    ///   `baseMs`, `kind`, `maxAttempts` → `Int`
    ///   `jitter`                        → `Bool`
    ///   `shouldRetry`                   → kernel arrow (`e -> Bool`)
    #[test]
    fn retry_policy_field_type_mapping_matches_kernel_scheme() {
        let mut interner = Interner::new();
        make_builder(&mut interner);
        // Verify the built-in type names the predicate checks are still correct.
        // If `Int` or `Bool` are renamed, these assertions fail before the SEAL
        // gap appears in emitted code.
        let int_sym = interner.intern("Int").expect("intern Int");
        assert_eq!(
            interner.resolve(int_sym).unwrap(),
            "Int",
            "built-in Int name changed; update is_retry_policy_record in ipe_lower"
        );
        let bool_sym = interner.intern("Bool").expect("intern Bool");
        assert_eq!(
            interner.resolve(bool_sym).unwrap(),
            "Bool",
            "built-in Bool name changed; update is_retry_policy_record in ipe_lower"
        );
        let error_sym = interner.intern("Error").expect("intern Error");
        assert_eq!(
            interner.resolve(error_sym).unwrap(),
            "Error",
            "built-in Error name changed; update is_kernel_shouldretry_ty in ipe_lower"
        );
    }

    /// Condition 4 — monotone burndown. Scheme resolution returns `Some` for
    /// EXACTLY `RELOCATED ∪ FIRST_SCHEMED` and `None` for every other variant.
    /// Pins the migrated set so an accidental over- or under-migration is caught.
    ///
    /// Resolution is read through [`Builder::resolve_scheme`], NOT
    /// [`Builder::stdlib_scheme`] directly: a kernel migrated to a structural
    /// `TyShape` has NO table arm (it resolves by interpreting its shape), so
    /// reading the table alone would see `None` and falsely report it
    /// un-migrated. `resolve_scheme` unions both routes — the same adapter
    /// inference and [`kernel_type_table`] use — so the burndown tracks the true
    /// schemed set regardless of which route a family takes.
    #[test]
    fn migrated_set_burndown() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        for &k in StdlibKernel::ALL {
            let migrated = builder.resolve_scheme(k.def().scheme).is_some();
            let expected = RELOCATED.contains(&k) || FIRST_SCHEMED.contains(&k);
            assert_eq!(
                migrated, expected,
                "resolve_scheme({k:?}).is_some() = {migrated} but \
                 RELOCATED∪FIRST_SCHEMED membership = {expected}",
            );
        }
    }

    /// The number of leading `->` arrows on a scheme's curried spine — its
    /// Ipê-level argument count.
    ///
    /// Walks ONLY the result (right) branch of each top-level [`Ty::Fun`],
    /// stopping at the first non-`Fun` node. A function that sits in an
    /// *argument* position (a higher-order kernel's callback, e.g. the
    /// `(Char -> Char)` in `String.map : (Char -> Char) -> String -> String`) is
    /// NOT descended into: it is one argument, not two, so `String.map` counts
    /// two arrows, matching its arity of 2. A kernel whose *result* is itself a
    /// function would count that trailing arrow too — which is the point: such a
    /// kernel's declared arity must include it, or the two disagree and the
    /// coherence test fires.
    fn scheme_arrow_count(ty: &Ty) -> u8 {
        let mut n: u8 = 0;
        let mut cur = ty;
        while let Ty::Fun(_, result) = cur {
            n = n.saturating_add(1);
            cur = result;
        }
        n
    }

    /// Kernels whose *result value is itself a function*, so their scheme's
    /// curried spine carries exactly ONE arrow more than `def().arity`.
    ///
    /// A `Middleware.with*` kernel is a handler transformer: applied to its
    /// declared arguments (a config plus, for most, nothing else) it yields a
    /// `Handler` value — and a `Handler` is `Req -> Task Resp`, itself a
    /// one-arrow function type. So `withLogging : Handler -> Handler` has
    /// arity 1 (it is applied to one argument, the wrapped handler) but a
    /// two-arrow scheme `(Req -> Task Resp) -> (Req -> Task Resp)`: the trailing
    /// arrow belongs to the RETURNED handler value, not to an argument position.
    /// The runtime confirms it — `middleware_with_logging(h) -> ServerHandler`
    /// takes one argument and returns a handler closure.
    ///
    /// This is the ONE legitimate non-1:1 arrow-vs-arity class; it is listed
    /// explicitly (with this reason) rather than excluded silently, so a NEW
    /// returns-a-function kernel that forgot to account for its trailing arrow
    /// still trips the coherence test until it is classified here on purpose.
    const RETURNS_HANDLER: &[StdlibKernel] = &[
        StdlibKernel::MiddlewareWithCors,
        StdlibKernel::MiddlewareWithLogging,
        StdlibKernel::MiddlewareWithBasicAuth,
        StdlibKernel::MiddlewareWithRateLimit,
        StdlibKernel::MiddlewareWithCsrf,
    ];

    /// The arity ↔ scheme coherence tripwire (the declared-but-mis-schemed
    /// drift catcher).
    ///
    /// For every schemed kernel in [`StdlibKernel::ALL`], resolve its scheme
    /// THROUGH the [`SchemeKey`] bridge — `def().scheme` -> [`resolve_scheme`] —
    /// and assert the scheme's leading-arrow count equals `def().arity`, plus one
    /// for the [`RETURNS_HANDLER`] class whose result value is itself a function.
    /// This is the extension of ADR 0009's
    /// `callee_arity`-derives-from-`decl().arity` rule to the *scheme*: a kernel
    /// whose declared arity disagrees with the arrow count of its type is a
    /// coherence failure here, caught pre-cargo, not a silent hole deep in the
    /// emitter — the drift class where a declared member had no coherent scheme
    /// and shipped as an exit-0-then-cargo-fail.
    ///
    /// Routing through [`Builder::resolve_scheme`] (not `stdlib_scheme` directly)
    /// is deliberate: it exercises the scheme-by-key bridge, proving `def().scheme`
    /// is resolvable to the exact same `Ty` the table produces.
    ///
    /// The relationship is 1:1 for every kernel except [`RETURNS_HANDLER`]:
    /// a curried `arg0 -> … -> result` scheme has `arity` leading arrows, and a
    /// nullary value kernel (e.g. `Jwt.claims : Claims`) has arity 0 and a
    /// non-`Fun` scheme (0 arrows). The returns-a-function class carries exactly
    /// one extra trailing arrow (the returned handler's own `Req -> Task Resp`),
    /// encoded above with its reason.
    #[test]
    fn scheme_arrow_count_matches_arity() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        let mut mismatches: Vec<(StdlibKernel, u8, u8)> = Vec::new();
        for &k in StdlibKernel::ALL {
            let def = k.def();
            // Only schemed kernels are checked; the un-schemed (routed /
            // unlowered) buckets are gated by `stdlib_scheme_total_over_reachable`
            // and fail closed at their call sites, so there is no scheme to weigh
            // against arity here.
            if let Some(scheme) = builder.resolve_scheme(def.scheme) {
                let arrows = scheme_arrow_count(&scheme);
                // The returns-a-function class carries one arrow for its returned
                // handler value on top of its argument arrows; every other kernel
                // is strictly arrows == arity.
                let extra = u8::from(RETURNS_HANDLER.contains(&k));
                let expected = def.arity.saturating_add(extra);
                if arrows != expected {
                    mismatches.push((k, arrows, expected));
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "arity <-> scheme coherence broken — these kernels' scheme \
             arrow-count disagrees with the expected count (def().arity, plus \
             one for the returns-a-function class): {mismatches:?} \
             (kernel, scheme_arrows, expected)",
        );
    }

    /// The load-bearing byte-identity guarantee: for every kernel that carries a
    /// structural [`TyShape`], interpreting its shape yields a `Ty`
    /// BYTE-IDENTICAL to the type its (now-removed) `stdlib_scheme` arm produced.
    /// This is belt-and-braces beyond the golden suite — it pins the interpreter
    /// directly against an INDEPENDENT reference, so a shape or interpreter that
    /// disagrees with it is caught here, pre-cargo, rather than as a golden-diff.
    ///
    /// # Where the oracle lives
    ///
    /// The reference each shape is checked against depends on the kernel's class,
    /// and both references are INDEPENDENT of the shape and its interpreter:
    ///
    /// - A **primitive monomorphic** shape-migrated family has NO `stdlib_scheme`
    ///   arm (its scheme lives once, on the descriptor), so there is no table `Ty`
    ///   to compare against. Its reference is [`expected_primitive_scheme`] below:
    ///   a per-kernel hand-built `Ty` authored from the published signature over
    ///   the primitive constructors.
    /// - A family whose scheme `expected_primitive_scheme` cannot express — the
    ///   `List` / `Maybe` / `Result` / `Set` / `Dict` combinators, the `Basics`
    ///   arrow-only arms, the `Bytes` decoders, and the tuple-shaped slice
    ///   (`zip`/`unzip`/`partition`, `fst`/`snd`, `toList`/`fromList`, the
    ///   `Random` seeded generators) — KEEPS its `stdlib_scheme` arm (that
    ///   retained hand-built arm — over `let var = Ty::Var`, the
    ///   `list`/`maybe`/`result`/`set`/`dict`/`order` closures, and the `tuple2`
    ///   builder — is the byte-identity witness). Its reference is that arm,
    ///   `stdlib_scheme(k)`, which `expected_primitive_scheme` cannot express.
    ///
    /// Selecting the reference by "does the kernel still have a table arm" keeps
    /// each shaped kernel checked against a genuine second source, so a wrong
    /// shape or a wrong interpreter arm makes the `assert_eq!` fire here.
    #[test]
    fn interpreted_shape_matches_legacy() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        let mut migrated = 0usize;
        for &k in StdlibKernel::ALL {
            let Some(shape) = k.def().shape else { continue };
            migrated += 1;
            let interpreted = builder.interpret_shape(shape);
            // Monomorphic families dropped their arm → the primitive oracle is
            // their only reference. Polymorphic `List` families kept their arm →
            // it is the reference (the primitive oracle has no `Ty` for them).
            let expected =
                expected_primitive_scheme(&builder, k).or_else(|| builder.stdlib_scheme(k));
            assert!(
                expected.is_some(),
                "kernel {k:?} carries a TyShape but neither the primitive oracle \
                 `expected_primitive_scheme` nor the retained `stdlib_scheme` arm \
                 provides a reference `Ty` for it — add one so byte-identity \
                 stays proven",
            );
            assert_eq!(
                Some(interpreted),
                expected,
                "interpreted TyShape for {k:?} is NOT byte-identical to its \
                 reference Ty — the structural encoding disagrees with the \
                 hand-authored signature",
            );
            // Field ORDER tripwire. `interpret_shape` builds the record via a
            // `BTreeMap`, which re-sorts by resolved symbol — so a reordered
            // declared field slice yields the SAME `Ty` and the byte-identity
            // `assert_eq!` above cannot catch it. Assert every record shape's
            // declared fields are in strictly-ascending resolved-symbol order
            // (matching the `BTreeMap` iteration order): a field reorder OR a
            // duplicate field now fails here.
            assert_record_fields_ordered(&builder, shape, k);
        }
        // Guard against a silently-empty sweep. The migrated set spans the
        // primitive-monomorphic kernels, the core `List` combinators, the
        // arrow-only / tuple-shaped / arrow-scalar polymorphic slices, and the
        // effect / scalar-opaque families now expressible with the `Unit` node
        // and the opaque/parametric `Con` tags: the `Task` / `Cmd` / `Sub` /
        // `PubSub` combinators, the `() -> …` and `… -> Task ()` effect kernels
        // (`Io` / `File` / `System` / `Time` / `Random` / `Process` / `Log` /
        // `Uuid` v4·v7 / `Trace`), the shared `Decoder a` families
        // (`Json.Decode` / `Db.Decode` / `Config`), the `JsonEnc` encoders, the
        // `Error` / `ErrorKind` / `ErrorDetails` ADT surface, the scalar-opaque
        // families (`Secret` / `Regex` / `Path` / `Url` / `Locale` / `Decimal`
        // via `Db.Decode.money` / `Crypto` typed-key / `EmailAddress` / `Sql`
        // fragment builders / `Jwt` builder / `Auth` / `Compression` /
        // `Encoding` decoders / `HttpMethod` / `Env`), the opaque-`Db`-handle
        // operations, the opaque `StreamWriter` / `StreamId` / `WsServer(Cfg)` /
        // `ServerRoute` / `ServerCookie` / `ServerRequest` handle kernels, and
        // the raw-`Int`-handle `WebSocket` client.
        //
        // The `Ui` / `Html` / style builder families — layout / element / event
        // / attribute builders, the `Html` node and `Html.Attributes` /
        // `Html.Events` builders, the `Font` / `Border` / `Background` / `Region`
        // attribute builders, the `Length` / `Color` / `Description` /
        // `PseudoClass` value builders, the non-record `Input` constructors
        // (`label*` / `labelHidden` / `placeholder` / `option`), `Ui.Keyed`,
        // `Ui.Lazy`, `Ui.breakpoint` / `mediaQuery` / `onPseudo`, and
        // `Server.listen` — via the `Attribute` / `Element` / `Html` / `Length` /
        // `Color` / `Description` / `PseudoClass` / `Label` / `Placeholder` /
        // `RadioOption` `Con` tags. The `Ipe.Html.Attribute` cons are
        // module-qualified (the `HtmlAttribute` tag carries the `Html` module
        // path — see `builtin_con_module`), byte-identical to the
        // `stdlib_scheme` `html_attr` builder.
        //
        // The closed-record / open-row families via the `Record` node: the
        // app-entry cfg records (`Web.app` open-row, `WebView.app` /
        // `Terminal.appScreen` open-row / `Terminal.appLines`), `HttpRequest` /
        // `HttpResponse` / server `Response`, `Migration`, `Csv` / `CacheCfg` /
        // `CacheStats` / `WebSocketCfg` / `EmailMessage` (+ nested attachment),
        // `RetryPolicy` (incl. the `Error`-channel `retryWith`), the
        // record-carrying `Input` (`text` / `multiline` / `checkbox` / `slider` /
        // `radio` / `radioRow`), `Border` (`widthEach` / `shadow` /
        // `innerShadow`), `Ui.button` / `Ui.layoutWith` / `Ui.paddingEach` /
        // `Ui.link` / `Ui.image`, and the record-producing `Server` route-handler
        // kernels — each byte-identical to its retained `stdlib_scheme` arm.
        assert!(
            migrated >= 863,
            "expected at least the primitive + core-List + arrow-only + \
             tuple-shaped + arrow-scalar polymorphic kernels plus the migrated \
             effect / scalar-opaque / Ui / Html / style builder families, the \
             closed-record / open-row families, and the arrow-over-record \
             server kernels (`Server.withCookie`, the `Middleware` wrappers, \
             `Stream.stream`, `HttpStream.open`, `Ws.upgrade`) — 863 total — to \
             carry a TyShape, found only {migrated}",
        );
    }

    /// Assert every [`TyShape::Record`] reachable from `shape` declares its
    /// fields in strictly-ascending resolved-symbol order — the order a
    /// `BTreeMap` iterates, so the declared slice mirrors the materialised
    /// `Ty::Record`'s key order. A reordered slice, or a duplicated field name,
    /// fails here even though `interpret_shape`'s `BTreeMap` re-sort hides both
    /// from the byte-identity `assert_eq!`.
    fn assert_record_fields_ordered(builder: &Builder, shape: &TyShape, k: StdlibKernel) {
        match shape {
            TyShape::Fun(a, b) => {
                assert_record_fields_ordered(builder, a, k);
                assert_record_fields_ordered(builder, b, k);
            }
            TyShape::Con(_, args) | TyShape::Tuple(args) => {
                for a in *args {
                    assert_record_fields_ordered(builder, a, k);
                }
            }
            TyShape::Record { fields, .. } => {
                let mut prev: Option<Symbol> = None;
                for (name, field) in *fields {
                    let sym = builder.field_symbol(*name);
                    if let Some(p) = prev {
                        assert!(
                            p < sym,
                            "record TyShape for {k:?} declares field {name:?} \
                             out of ascending resolved-symbol order (or a \
                             duplicate) — declare record fields sorted by \
                             resolved symbol so the slice mirrors the BTreeMap",
                        );
                    }
                    prev = Some(sym);
                    assert_record_fields_ordered(builder, field, k);
                }
            }
            TyShape::Unit | TyShape::Var(_) => {}
        }
    }

    /// Independent byte-identity oracle for the shape-migrated primitive
    /// families: the exact `Ty` each kernel's removed `stdlib_scheme` arm built,
    /// re-authored here from the kernel's published signature over the six
    /// primitive constructors. Returns `None` for a kernel that carries no
    /// primitive shape (so a future non-primitive migration is flagged loudly by
    /// [`interpreted_shape_matches_legacy`] rather than silently unproven).
    ///
    /// Deliberately built with LOCAL closures (not by calling `stdlib_scheme`,
    /// which carries no arm for a shape-migrated family) so it is a second,
    /// independent source — the whole point of an oracle.
    #[allow(clippy::too_many_lines)] // declarative reference table — mirrors the removed arms
    #[allow(clippy::match_same_arms)] // family-grouped; coincidentally-equal signatures across families stay separate for readability
    fn expected_primitive_scheme(builder: &Builder, k: StdlibKernel) -> Option<Ty> {
        use StdlibKernel as K;
        let b = &builder.builtins;
        let int = || Ty::Con {
            module: Vec::new(),
            name: b.int,
            args: Vec::new(),
        };
        let float = || Ty::Con {
            module: Vec::new(),
            name: b.float,
            args: Vec::new(),
        };
        let bool_ty = || Ty::Con {
            module: Vec::new(),
            name: b.bool,
            args: Vec::new(),
        };
        let string = || Ty::Con {
            module: Vec::new(),
            name: b.string,
            args: Vec::new(),
        };
        let char = || Ty::Con {
            module: Vec::new(),
            name: b.char,
            args: Vec::new(),
        };
        let bytes = || Ty::Con {
            module: Vec::new(),
            name: b.bytes,
            args: Vec::new(),
        };
        let fun = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b));
        Some(match k {
            // ── Bitwise / Math.abs. ──
            K::BitwiseAnd
            | K::BitwiseOr
            | K::BitwiseXor
            | K::BitwiseShiftLeftBy
            | K::BitwiseShiftRightBy
            | K::BitwiseShiftRightZfBy => fun(int(), fun(int(), int())),
            K::BitwiseComplement | K::MathAbs => fun(int(), int()),

            // ── Math (monomorphic arms). ──
            K::MathPi | K::MathE | K::MathPhi | K::MathSqrt2 | K::MathInf | K::MathNan => float(),
            K::MathIsNaN => fun(float(), bool_ty()),
            K::MathSqrt
            | K::MathCbrt
            | K::MathExp
            | K::MathExp2
            | K::MathLog
            | K::MathLog2
            | K::MathLog10
            | K::MathSin
            | K::MathCos
            | K::MathTan
            | K::MathAsin
            | K::MathAcos
            | K::MathAtan
            | K::MathSinh
            | K::MathCosh
            | K::MathTanh
            | K::MathAsinh
            | K::MathAcosh
            | K::MathAtanh
            | K::BasicsSqrt => fun(float(), float()),
            K::MathFloor | K::MathCeil | K::MathRound | K::MathTrunc => fun(float(), int()),
            K::MathPow | K::MathHypot | K::MathAtan2 | K::MathMod | K::MathRemainder => {
                fun(float(), fun(float(), float()))
            }

            // ── Basics. ──
            K::BasicsNot => fun(bool_ty(), bool_ty()),

            // ── String / Money / Time primitive shapes. ──
            K::StringFromInt | K::TimeTimeString => fun(int(), string()),
            K::StringFromFloat => fun(float(), string()),
            // `Money.minorUnits : String -> Int` (the ISO-code-taking kernel).
            K::StringLength | K::MoneyMinorUnits => fun(string(), int()),
            K::StringIsEmpty | K::StringIsEmail | K::StringIsUrl | K::MoneyIsKnownCurrency => {
                fun(string(), bool_ty())
            }
            K::StringReverse
            | K::StringToUpper
            | K::StringToLower
            | K::StringCasefold
            | K::StringTrim
            | K::StringTrimStart
            | K::StringTrimEnd
            | K::CryptoSha256
            | K::CryptoSha512
            | K::CryptoSha1
            | K::CryptoMd5
            | K::EncodingBase64Encode
            | K::EncodingUrlEncode
            | K::EncodingHexEncode
            | K::HtmlEscapeText
            | K::HtmlEscapeAttr
            | K::CssSafetyStripStyleClose
            | K::MoneySymbol
            | K::MoneyCurrencyName => fun(string(), string()),
            K::StringFromChar | K::CharToLower | K::CharToUpper => fun(char(), string()),
            K::StringAppend
            | K::SystemGetenvOr
            | K::CryptoHmacSha256
            | K::CryptoHmacSha512
            | K::CryptoAesKeyFromPassword
            | K::CryptoChachaKeyFromPassword => fun(string(), fun(string(), string())),
            K::StringContains
            | K::StringStartsWith
            | K::StringEndsWith
            | K::StringEqualFold
            | K::StringContainsIn
            | K::StringStartsWithIn
            | K::StringEndsWithIn
            | K::CryptoConstantTimeEqual
            | K::MoneyHasRate => fun(string(), fun(string(), bool_ty())),
            K::StringReplace => fun(string(), fun(string(), fun(string(), string()))),
            K::CryptoRsaSha256Verify => fun(string(), fun(string(), fun(string(), bool_ty()))),
            K::StringRepeat
            | K::StringDropLeft
            | K::StringDropRight
            | K::StringLeft
            | K::StringRight => fun(int(), fun(string(), string())),
            K::StringSlice => fun(int(), fun(int(), fun(string(), string()))),
            K::StringPadLeft | K::StringPadRight | K::StringPad => {
                fun(int(), fun(char(), fun(string(), string())))
            }
            K::StringCons => fun(char(), fun(string(), string())),
            K::StringMap => fun(fun(char(), char()), fun(string(), string())),
            K::StringFilter => fun(fun(char(), bool_ty()), fun(string(), string())),
            K::StringAny | K::StringAll => fun(fun(char(), bool_ty()), fun(string(), bool_ty())),

            // ── Char. ──
            K::CharIsAlpha
            | K::CharIsDigit
            | K::CharIsLower
            | K::CharIsUpper
            | K::CharIsAlphaNum
            | K::CharIsHexDigit
            | K::CharIsOctDigit => fun(char(), bool_ty()),
            K::CharToCode => fun(char(), int()),
            K::CharFromCode => fun(int(), char()),

            // ── Bytes. ──
            K::BytesEmpty => bytes(),
            K::BytesLength => fun(bytes(), int()),
            K::BytesIsEmpty => fun(bytes(), bool_ty()),
            K::BytesFromString => fun(string(), bytes()),
            K::BytesToHex | K::BytesToBase64 => fun(bytes(), string()),
            K::BytesAppend => fun(bytes(), fun(bytes(), bytes())),
            K::BytesSlice => fun(int(), fun(int(), fun(bytes(), bytes()))),

            // ── Time calendar helpers. ──
            K::TimeIsLeapYear => fun(int(), bool_ty()),
            K::TimeDaysInMonth => fun(int(), fun(int(), int())),

            // ── RateLimit / string constants. ──
            K::RateLimitAllow => fun(string(), fun(string(), fun(int(), fun(int(), bool_ty())))),
            K::FontSansSerif
            | K::FontSerif
            | K::FontMonospace
            | K::UiMobile
            | K::UiTablet
            | K::UiDesktop
            | K::UiDarkMode
            | K::UiLightMode
            | K::UiReducedMotion => string(),

            _ => return None,
        })
    }

    /// Totality gate. Scheme resolution is TOTAL over the reachable set:
    /// every `StdlibKernel` except the explicit `KNOWN_UNBACKED` exclusions has a
    /// concrete scheme. This is the load-bearing precondition for deleting the
    /// `Ty::Var(u32::MAX)` fallback — only sound if no reachable kernel is
    /// silently riding it. If this fails, it prints the un-schemed variants;
    /// they must be schemed (or classified `KNOWN_UNBACKED`).
    ///
    /// Read through [`Builder::resolve_scheme`], not [`Builder::stdlib_scheme`]:
    /// a shape-migrated kernel has no table arm and is schemed by interpreting
    /// its `TyShape`, so the totality check must union both routes exactly as
    /// inference does.
    #[test]
    fn stdlib_scheme_total_over_reachable() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        let unschemed: Vec<StdlibKernel> = StdlibKernel::ALL
            .iter()
            .copied()
            .filter(|k| {
                !KNOWN_UNBACKED.contains(k)
                    && !REACHABLE_BUT_UNLOWERED.contains(k)
                    && builder.resolve_scheme(k.def().scheme).is_none()
            })
            .collect();
        assert!(
            unschemed.is_empty(),
            "stdlib_scheme is NOT total over the reachable set — these variants \
             are neither schemed nor KNOWN_UNBACKED, so the un-typed sentinel \
             fallback cannot be deleted yet: {unschemed:?}",
        );
    }

    /// SEAL. The banned F1 exit-0 sentinel (`Ty::Var` at the
    /// reserved max id) is GONE from the code: `kernel_ty` and its
    /// `_ => <sentinel>` fallthrough are deleted, and `constrain_var_kernel`
    /// fails closed with IPE-L0108 on a registry miss. This test freezes that by
    /// scanning this very source file: no NON-COMMENT line may contain the
    /// sentinel token, so any reintroduction (a new fallback, a resurrected
    /// legacy arm) is a compile-time-adjacent test failure. Comment/doc lines
    /// are excluded — they legitimately narrate the retired sentinel's history.
    /// The needle is built via `concat!` so this test's own source does not
    /// contain the contiguous banned token and thus never self-matches.
    #[test]
    fn no_ty_var_max_sentinel() {
        let src = include_str!("constrain.rs");
        let needle = concat!("Ty::Var(u32::", "MAX)");
        for (idx, line) in src.lines().enumerate() {
            // Strip the comment tail: everything from the first `//` onward.
            // `///` / `//!` doc lines and inline `// …` trailers thus drop out,
            // leaving only executable code / string literals to inspect.
            let code = line.split("//").next().unwrap_or(line);
            assert!(
                !code.contains(needle),
                "F1 sentinel token reintroduced in CODE at constrain.rs:{} — \
                 the exit-0 un-typed-kernel fallback must stay deleted (Task 1c \
                 seal). Offending line: {line:?}",
                idx + 1,
            );
        }
    }

    /// Condition 2 — the fail-closed path is REACHABLE. When the registry does
    /// not type a kernel, `kernel_scheme_or_unsupported` raises the
    /// IPE-L0108-shaped `Err` (loud), NOT a silent `Ty::Var`. Also checks
    /// registry-first precedence and single-source resolution.
    ///
    /// The legacy string table is DELETED and
    /// `constrain_var_kernel` passes `None` for the legacy slot, so a registry
    /// miss (`None` id, or a `REACHABLE_BUT_UNLOWERED` bucket) reaches this exact
    /// `Err` live in the constrain path — the seal that removed the exit-0 hole.
    #[test]
    fn both_miss_is_fail_closed() {
        let span = Span::DUMMY;
        let a = Ty::Var(0);
        let b = Ty::Var(1);

        // BOTH miss → fail-closed IPE-L0108.
        let err = Builder::kernel_scheme_or_unsupported(None, None, span)
            .expect_err("both-miss must fail closed, not type as Ty::Var");
        assert!(
            matches!(
                err,
                Diagnostic::Lower {
                    msg: LowerError::Unsupported(Feature::Kernels),
                    ..
                }
            ),
            "expected IPE-L0108 (Feature::Kernels), got {err:?}",
        );

        // Registry present → used.
        assert_eq!(
            Builder::kernel_scheme_or_unsupported(Some(a.clone()), None, span),
            Ok(a.clone()),
        );
        // Only legacy present → used.
        assert_eq!(
            Builder::kernel_scheme_or_unsupported(None, Some(b.clone()), span),
            Ok(b.clone()),
        );
        // Both present → registry wins (parse-once precedence).
        assert_eq!(
            Builder::kernel_scheme_or_unsupported(Some(a.clone()), Some(b), span),
            Ok(a),
        );
    }

    /// The [`Builder::hof_result_slot_for`] table
    /// cannot drift from the scheme shapes in [`Builder::stdlib_scheme`]: for
    /// every table entry, the slot's raw var must be exactly the FINAL RESULT
    /// of the kernel's callback arrow (the arrow the runtime kernel fully
    /// applies). A drifted slot would tie the obligation to the WRONG scheme
    /// variable — silently unsound (the hazard var escapes unchecked while an
    /// innocent var gets over-constrained) — which is precisely the failure
    /// class this item was reverted for four times.
    #[test]
    fn hof_result_slots_match_scheme_shapes() {
        fn arrow_final(mut t: &Ty) -> &Ty {
            while let Ty::Fun(_, r) = t {
                t = r;
            }
            t
        }

        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        let mut covered = 0;
        for &k in StdlibKernel::ALL {
            let Some(slot) = Builder::hof_result_slot_for(k) else {
                continue;
            };
            covered += 1;
            let scheme = builder.stdlib_scheme(k);
            assert!(
                scheme.is_some(),
                "{k:?} carries a hof_kernel_result obligation and must be schemed",
            );
            let Some(scheme) = scheme else { continue };

            // Locate the callback arrow: for the map family it is the
            // scheme's FIRST parameter; for `andMap` it is the unique arrow
            // inside the SECOND parameter's `Con` payload
            // (`Con (a -> b)` in `Maybe (a -> b)` / `Result e (a -> b)`).
            let cb: Option<&Ty> = match k {
                StdlibKernel::MaybeAndMap | StdlibKernel::ResultAndMap => {
                    if let Ty::Fun(_, rest) = &scheme
                        && let Ty::Fun(second, _) = rest.as_ref()
                        && let Ty::Con { args, .. } = second.as_ref()
                    {
                        args.iter().find(|a| matches!(a, Ty::Fun(_, _)))
                    } else {
                        None
                    }
                }
                _ => {
                    if let Ty::Fun(first, _) = &scheme {
                        Some(first.as_ref())
                    } else {
                        None
                    }
                }
            };
            assert!(
                matches!(cb, Some(Ty::Fun(_, _))),
                "{k:?}: could not locate the callback arrow in its scheme — \
                 the scheme shape changed; re-derive hof_result_slot_for",
            );
            let Some(cb) = cb else { continue };
            assert_eq!(
                arrow_final(cb),
                &Ty::Var(slot),
                "{k:?}: hof_result_slot_for says raw var {slot} but the \
                 callback arrow's final result is a different type — the \
                 obligation would bind the WRONG variable",
            );
        }
        // Freeze the covered set's size so silently dropping a kernel from
        // the table (obligation removed → hazard reopened) fails loudly.
        assert_eq!(
            covered, 13,
            "hof_result_slot_for must cover exactly the 13 Maybe/Result \
             higher-order kernels (map ×2, map2..5 ×8, mapError ×1, andMap \
             ×2); adding/removing a member must update this pin AND the \
             fixtures",
        );
    }
}

#[cfg(test)]
mod aud13_solver_var_tag_tests {
    use super::{Builder, Builtins, Content, Interner, Ty, UnionFind};
    use crate::ty::tag_solver_var;
    use std::collections::BTreeMap;

    fn make_builder(interner: &mut Interner) -> Builtins {
        Builtins::new(interner).expect("Builtins::new must not fail in tests")
    }

    /// AUD-13 regression: `instantiate_in`'s wildcard-`"any"` check must not
    /// misfire on a solver-representative id that happens to numerically
    /// equal the interned raw of the string `"any"`. Constructs the exact
    /// collision by reusing `any`'s own raw, tagged as solver-space —
    /// `zonk` (see `constrain.rs`'s `Content::Flex | Rigid | Super` arm)
    /// tags every surviving `VarId` this way before it can ever reach
    /// `instantiate_in` again.
    #[test]
    fn tagged_solver_var_sharing_any_raw_is_not_treated_as_wildcard_any() {
        let mut interner = Interner::new();
        let any_sym = interner
            .intern("any")
            .expect("interning \"any\" must not fail");
        let any_raw = any_sym.as_raw();

        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let mut builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        // Tagged: the SAME raw as `any`'s interned symbol, but marked
        // solver-space. Two references through one `vars` map must resolve
        // to the SAME variable (ordinary shared-var behavior) — if the tag
        // were ignored, the wildcard-`any` path would instead mint a FRESH
        // flex var per occurrence.
        let tagged = Ty::Var(tag_solver_var(any_raw));
        let mut vars = BTreeMap::new();
        let first = builder
            .instantiate_in(&tagged, &mut vars, false)
            .expect("instantiate_in must not fail");
        let second = builder
            .instantiate_in(&tagged, &mut vars, false)
            .expect("instantiate_in must not fail");
        assert_eq!(
            first, second,
            "a tagged solver-var raw sharing any's numeric value must still \
             share ONE variable across occurrences, proving it was NOT \
             routed through the wildcard-any fresh-per-occurrence path",
        );
    }

    /// Control: the SAME raw value, untagged, is genuine annotation-space
    /// `"any"` and must keep its documented wildcard semantics — each
    /// occurrence gets an independent fresh flex variable.
    #[test]
    fn untagged_any_raw_still_gets_wildcard_semantics() {
        let mut interner = Interner::new();
        let any_sym = interner
            .intern("any")
            .expect("interning \"any\" must not fail");
        let any_raw = any_sym.as_raw();

        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let mut builder = Builder::for_scheme_table(&mut uf, &interner, builtins);

        let untagged = Ty::Var(any_raw);
        let mut vars = BTreeMap::new();
        let first = builder
            .instantiate_in(&untagged, &mut vars, false)
            .expect("instantiate_in must not fail");
        let second = builder
            .instantiate_in(&untagged, &mut vars, false)
            .expect("instantiate_in must not fail");
        assert_ne!(
            first, second,
            "untagged \"any\" must keep independent-fresh-var-per-occurrence \
             wildcard semantics",
        );
    }
}
