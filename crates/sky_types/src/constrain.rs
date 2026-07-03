//! Constraint generation, ported from the M0-relevant arms of
//! `Sky.Type.Constrain.Expression` (derivative of elm/compiler's
//! `Type.Constrain.Expression`, BSD-3-Clause).
//!
//! Walks the canonical module, minting a union-find variable for each
//! sub-expression region and emitting equality [`Constraint`]s that the solver
//! discharges. The arms modelled are exactly those the M0 golden program
//! exercises: integer literals, `VarLocal` / `VarTopLevel` / `VarKernel` /
//! `VarCtor` references, function application (`Call`), `case`, and the binary
//! operators `+` / `-`.
//!
//! This module also owns the two bridges between the resolved [`Ty`] level and
//! the solver level: [`Builder::instantiate`] (a [`Ty`] → fresh union-find
//! structure) and [`Builder::zonk`] (a settled union-find variable → [`Ty`]).

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, Feature, LowerError, Span, TypeError};
use sky_intern::{Interner, Symbol};
use sky_kernels::StdlibKernel;

use crate::doc::{VarNamer, canon_type_to_doc, ty_to_doc};
use crate::solve::{Budget, Constraint};
use crate::ty::{Content, FlatType, Ty, TyBounds, from_canon};
use crate::unionfind::{UnionFind, VarId};

/// `where_` tag for any `CompilerBug` raised during constraint generation.
const STAGE: &str = "sky_types::constrain";

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
/// never appears in M0 source, so the builder interns them up front to
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
    /// `Sky.Core.Dict` type constructor symbol.
    dict: Symbol,
    /// `Sky.Core.Set` type constructor symbol.
    set: Symbol,
    /// `Sky.Core.Bytes` type constructor symbol.
    /// Divergence from Sky: Bytes is a distinct primitive in Sky-Rust (Vec<u8>),
    /// not a String alias as in the Go reference.
    bytes: Symbol,
    /// The interned `Error` symbol, used to validate the error channel in
    /// `Task Error a` annotations (normalised to unary `Task a`) and to pin the
    /// handler parameter type in `mapError` / `onError` so a bare lambda `\e ->
    /// ...` infers `e : Error` without leaving a free variable.
    error: Symbol,
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
    /// `"url"` — `HttpRequest` only.
    http_f_url: Symbol,
    /// `"timeout"` — `HttpRequest` only.
    http_f_timeout: Symbol,
    /// `"followRedirects"` — `HttpRequest` only (camelCase Sky field name).
    http_f_follow_redirects: Symbol,
    /// `"maxRedirects"` — `HttpRequest` only (camelCase Sky field name).
    http_f_max_redirects: Symbol,
    // ── Db type symbols (M5b-db) ─────────────────────────────────────────────
    /// `"Db"` — the opaque database connection pool type constructor.
    db: Symbol,
    /// `"SqlValue"` — the sum type for typed SQL parameter values.
    sqlvalue: Symbol,
    /// `"SqlField"` — the sum type for PATCH-style field-set / field-omit SQL params.
    sqlfield: Symbol,
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
    // ── Shared row-decoder type (M5b-db + M4h JSON) ───────────────────────────
    /// `"Decoder"` — the opaque decoder type constructor shared by `Sky.Core.Json.Decode`
    /// and `Std.Db.Decode`. Represented in the IR as `IrType::Decoder(Box<IrType>)`.
    decoder: Symbol,
    // ── TEA Cmd / Sub type constructor symbols (M5c) ─────────────────────────
    /// `"Cmd"` — the opaque command type constructor `Cmd msg`.
    /// Represented in the IR as `IrType::Cmd(Box<IrType>)`.
    cmd: Symbol,
    /// `"Sub"` — the opaque subscription type constructor `Sub msg`.
    /// Represented in the IR as `IrType::Sub(Box<IrType>)`.
    sub: Symbol,
    // ── Sky.Http.Server opaque type constructor symbols (M6) ──────────────────
    /// `"Request"` — the opaque server request type.
    server_request: Symbol,
    /// `"Response"` — the opaque server response type.
    server_response: Symbol,
    /// `"Route"` — the opaque server route type.
    server_route: Symbol,
    /// `"Cookie"` — the opaque server cookie type.
    server_cookie: Symbol,
    // ── M7: Std.Ui / Std.Html parametric type constructor symbols ─────────────
    /// `"Attribute"` — Std.Ui attribute type constructor `Attribute msg`.
    ///
    /// Used to build Ui kernel type schemes so the HM solver constrains
    /// `List (Attribute msg)` arguments (e.g. `layout [] child`) to a concrete
    /// element type rather than leaving them as free variables.  Without these
    /// entries the empty-attrs list `[]` keeps `List (Ty::Var)` as its region
    /// type, `list_elem_ir` returns `IrType::Json`, and `emit_list` emits the
    /// bare `Vec::new()` that Rust rejects with E0283 when M cannot be inferred
    /// from elsewhere in the expression.
    attribute: Symbol,
    /// `"Element"` — Std.Ui element type constructor `Element msg`.
    element: Symbol,
    /// `"Html"` — Html type constructor `Html msg` (shared by Std.Html and
    /// Std.Ui render entry points).
    html_con: Symbol,
    /// `"Length"` — Std.Ui nullary length type produced by `Ui.px` / `Ui.fill`
    /// / `Ui.minimum` / …. Lowered to `IrType::UiPlain(UiPlain::Length)` via the
    /// `"Length"` arm in `sky_lower::ir_type_from_ty`.
    length: Symbol,
    /// `"Color"` — Std.Ui nullary colour type produced by `Ui.rgb` / `Ui.rgba`
    /// / `Ui.white` / …. Lowered to `IrType::UiPlain(UiPlain::Color)`.
    color: Symbol,
    /// `"Value"` — the opaque JSON value type (`Value = any` in Sky) produced /
    /// consumed by the `JsonEnc.*` encoders. Lowered to `IrType::Json`
    /// (`serde_json::Value`, re-exported as `JsonVal`) via the `"Value"` arm in
    /// `sky_lower::ir_type_from_ty`. A distinct interned symbol so the `JsonEnc`
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
    // ── Std.Live / Sky.Live opaque type constructor symbols (Phase-1b) ─────────
    /// `"LiveReq"` — opaque request threaded through `Live.app`'s `init`.
    live_req: Symbol,
    /// `"LiveRoute"` — opaque route descriptor returned by `Live.route`.
    live_route_con: Symbol,
    // ── Live cfg record field name symbols ───────────────────────────────────────
    /// `"init"` — the init field of the `Live.app` config record.
    live_f_init: Symbol,
    /// `"update"` — the update field of the `Live.app` config record.
    live_f_update: Symbol,
    /// `"view"` — the view field of the `Live.app` config record.
    live_f_view: Symbol,
    /// `"subscriptions"` — the subscriptions field of the `Live.app` config record.
    live_f_subscriptions: Symbol,
    /// `"routes"` — the routes field of the `Live.appRouted` config record.
    /// Reserved for a future split scheme between `app` and `appRouted`.
    #[allow(dead_code)]
    live_f_routes: Symbol,
    /// `"notFound"` — the notFound field of the `Live.appRouted` config record.
    /// Reserved for a future split scheme between `app` and `appRouted`.
    #[allow(dead_code)]
    live_f_not_found: Symbol,
    // ── Tui cfg record field name symbols (Phase-1c) ─────────────────────────────
    /// `"onKey"` — the onKey field of the `Tui.app` / `Tui.program` config record.
    /// Flat `String -> String -> Msg` — byte-matches the runtime bound
    /// `FOnKey: Fn(String, String) -> Msg`.
    tui_f_on_key: Symbol,
    // ── Webview cfg record field name symbols (Phase-1d) ─────────────────────────
    /// `"window"` — the window field of the `Webview.app` config record.
    /// Typed as a closed record `{ title : String, size : (Int, Int) }`.
    webview_f_window: Symbol,
    /// `"title"` — the title field inside the Webview window config record.
    webview_f_title: Symbol,
    /// `"size"` — the size field inside the Webview window config record.
    /// Typed as `(Int, Int)` — width × height in logical pixels.
    webview_f_size: Symbol,
}

impl Builtins {
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
            tv_a: interner.intern("a")?,
            tv_e: interner.intern("e")?,
            // Http field names (camelCase, as they appear in Sky source).
            http_f_body: interner.intern("body")?,
            http_f_headers: interner.intern("headers")?,
            http_f_status: interner.intern("status")?,
            http_f_method: interner.intern("method")?,
            http_f_url: interner.intern("url")?,
            http_f_timeout: interner.intern("timeout")?,
            http_f_follow_redirects: interner.intern("followRedirects")?,
            http_f_max_redirects: interner.intern("maxRedirects")?,
            // Db symbols (M5b-db).
            db: interner.intern("Db")?,
            sqlvalue: interner.intern("SqlValue")?,
            sqlfield: interner.intern("SqlField")?,
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
            // TEA Cmd / Sub type constructors (M5c).
            cmd: interner.intern("Cmd")?,
            sub: interner.intern("Sub")?,
            // Sky.Http.Server opaque types (M6).
            server_request: interner.intern("Request")?,
            server_response: interner.intern("Response")?,
            server_route: interner.intern("Route")?,
            server_cookie: interner.intern("Cookie")?,
            // M7: Std.Ui / Std.Html parametric type constructor symbols.
            attribute: interner.intern("Attribute")?,
            element: interner.intern("Element")?,
            html_con: interner.intern("Html")?,
            length: interner.intern("Length")?,
            color: interner.intern("Color")?,
            json_value: interner.intern("Value")?,
            lw_wrapper_attrs: interner.intern("wrapperAttrs")?,
            lw_root_attrs: interner.intern("rootAttrs")?,
            // Phase-1b: Std.Live / Sky.Live opaque types + cfg field names.
            live_req: interner.intern("LiveReq")?,
            live_route_con: interner.intern("LiveRoute")?,
            live_f_init: interner.intern("init")?,
            live_f_update: interner.intern("update")?,
            live_f_view: interner.intern("view")?,
            live_f_subscriptions: interner.intern("subscriptions")?,
            live_f_routes: interner.intern("routes")?,
            live_f_not_found: interner.intern("notFound")?,
            // Phase-1c: Tui cfg field names.
            tui_f_on_key: interner.intern("onKey")?,
            // Phase-1d: Webview cfg field names.
            webview_f_window: interner.intern("window")?,
            webview_f_title: interner.intern("title")?,
            webview_f_size: interner.intern("size")?,
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
            // ── SqlValue constructors (M5b-db) ─────────────────────────────────
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
            // Minimal wiring: Sky users write `SqlDecimal "1234.56"` rather than
            // a native Decimal value (native Decimal IrType deferred to M6).
            (
                self.sql_decimal,
                CtorScheme {
                    arg_tys: vec![string_ty.clone()],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // SqlMoney wraps a String in "ISO_CODE AMOUNT" format (TEXT).
            // Minimal wiring matching Go's sqlMoneyToString / db_decode_money.
            // Sky users write `SqlMoney "USD 1234.56"`.
            (
                self.sql_money,
                CtorScheme {
                    arg_tys: vec![string_ty],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // SqlNull wraps another SqlValue as a type-level witness; the inner
            // value is discarded by `into_sql_param()` → SqlParam::Null.
            (
                self.sql_null,
                CtorScheme {
                    arg_tys: vec![sqlvalue_ty.clone()],
                    result: sqlvalue_ty.clone(),
                },
            ),
            // ── SqlField constructors (M5b-db) ─────────────────────────────────
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
    /// Resolved type per source region (filled with vars, read back post-solve).
    regions: BTreeMap<Span, VarId>,
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
    top_level: BTreeMap<(Vec<Symbol>, Symbol), Ty>,
    /// Body region-var of each untyped top-level binding, read back for `env`.
    ///
    /// Keyed by `(home_module_path, bare_name)` for the same reason as
    /// [`Self::top_level`].
    untyped: BTreeMap<(Vec<Symbol>, Symbol), VarId>,
    /// Deferred record field-access obligations, resolved after the main solve.
    field_accesses: Vec<FieldAccess>,
    /// Deferred record-update obligations, resolved after the main solve.
    record_updates: Vec<RecordUpdate>,
    /// The type scheme of every data constructor declared in this module, keyed
    /// by constructor name. A constructor is a (possibly generic) function
    /// `field0 -> … -> fieldN -> T vars`; each use site instantiates the scheme
    /// fresh, exactly as a polymorphic top-level binding does.
    ctors: BTreeMap<Symbol, CtorScheme>,
    /// One entry per typed binding: its name and the rigid (skolem) variable each
    /// of its annotation type variables instantiated to while its body was
    /// checked. Read post-solve to recover each variable's super-type obligations
    /// (the bounds the body imposed) for generalisation.
    typed_rigids: Vec<(Symbol, BTreeMap<Symbol, VarId>)>,
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
    /// compare, with SKY-T0014 rather than emitting code `cargo` rejects).
    super_vars: Vec<(VarId, TyBounds, Span)>,
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
}

/// The output of constraint generation, consumed by the solver + read-back.
pub struct Generated {
    pub regions: BTreeMap<Span, VarId>,
    pub constraints: Vec<Constraint>,
    pub top_level: BTreeMap<(Vec<Symbol>, Symbol), Ty>,
    pub untyped: BTreeMap<(Vec<Symbol>, Symbol), VarId>,
    pub field_accesses: Vec<FieldAccess>,
    pub record_updates: Vec<RecordUpdate>,
    pub typed_rigids: Vec<(Symbol, BTreeMap<Symbol, VarId>)>,
    pub scheme_apps: Vec<SchemeApp>,
    pub super_vars: Vec<(VarId, TyBounds, Span)>,
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
        let builtins = Builtins::new(interner)?;
        let mut builder = Self {
            uf,
            interner,
            builtins,
            regions: BTreeMap::new(),
            constraints: Vec::new(),
            top_level: BTreeMap::new(), // (home, name) → Ty
            untyped: BTreeMap::new(),   // (home, name) → VarId
            field_accesses: Vec::new(),
            record_updates: Vec::new(),
            ctors: BTreeMap::new(),
            typed_rigids: Vec::new(),
            scheme_apps: Vec::new(),
            super_vars: Vec::new(),
        };

        // Register the Prelude-built-in constructor schemes (`True` / `False` /
        // `Just` / `Nothing` / `Ok` / `Err`) first, so a reference or pattern
        // instantiates `Maybe a` / `Result e a` / `Bool` fresh per use site. A
        // user `type` cannot shadow these names (the canon §3.2 gate rejects it),
        // so the module-union loop below never collides with them.
        for (name, scheme) in builder.builtins.ctor_schemes() {
            builder.ctors.insert(name, scheme);
        }

        // Register every data constructor's scheme up front, so a `VarCtor`
        // reference or a constructor pattern can instantiate it fresh. A
        // constructor `C : field0 -> … -> T vars`; the result type applies the
        // union to its declared type variables (as `Ty::Var`s), and the field
        // types carry those same variables, so one shared instantiation map
        // alpha-renames a generic constructor per use site.
        for union in &module.unions {
            // Use the union's own `home` (its original defining module path)
            // rather than `module.name`. After `link::link` merges N canonical
            // modules into one, every union retains its source-module path in
            // `home`; `module.name` would always be the entry module's name
            // (e.g. `["Main"]`), causing cross-module constructor result types
            // (`Main.Color`) to diverge from cross-module type annotations
            // (`Helper.Color`) and fail unification (SKY-T0001).
            let result = Ty::Con {
                module: union.home.clone(),
                name: union.name,
                args: union.vars.iter().map(|v| Ty::Var(v.as_raw())).collect(),
            };
            for ctor in &union.ctors {
                let arg_tys = ctor.args.iter().map(from_canon).collect();
                builder.ctors.insert(
                    ctor.name,
                    CtorScheme {
                        arg_tys,
                        result: result.clone(),
                    },
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
        //   which the M2a solver does not yet model — so an untyped polymorphic
        //   binding is monomorphic at its use sites. Sound, not yet complete;
        //   write an annotation to get full polymorphism.)
        for def in &module.defs {
            // Key by (home_module_path, bare_name) so same-named defs from
            // different source modules never overwrite each other after
            // `link::link` merges them into a single flat def list.
            let home_key = def.home().to_vec();
            match def {
                canon::Def::Typed { name, ty, .. } => {
                    let normalized = builder.normalize_annotation_ty(from_canon(ty), name.span)?;
                    builder.top_level.insert((home_key, name.value), normalized);
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

        Ok(Generated {
            regions: builder.regions,
            constraints: builder.constraints,
            top_level: builder.top_level,
            untyped: builder.untyped,
            field_accesses: builder.field_accesses,
            record_updates: builder.record_updates,
            typed_rigids: builder.typed_rigids,
            scheme_apps: builder.scheme_apps,
            super_vars: builder.super_vars,
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

    /// Mint a fresh super-typed flexible variable carrying `bounds` — a value
    /// the body has constrained to a Sky super-type (numeric / ordered /
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
    /// returned [`VarId`] is the result type's variable. Mirrors the M1-core
    /// subset of `Sky.Type.Constrain.Expression.binopTypes`.
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
                // function instantiation fails the post-solve gate (SKY-T0014).
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
                // `++` is `String -> String -> String`: both operands and the
                // result are pinned to `String`. A non-String operand (a
                // would-be `List`) fails to unify with `String` and surfaces as
                // a type error rather than reaching the backend.
                let ls = self.string_var()?;
                self.eq(lhs.span, lv, ls);
                let rs = self.string_var()?;
                self.eq(rhs.span, rv, rs);
                self.string_var()
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
        self.eq(tail.span, tail_var, list);
        Ok(list)
    }

    fn eq(&mut self, span: Span, lhs: VarId, rhs: VarId) {
        self.constraints.push(Constraint { span, lhs, rhs });
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
            Ty::Record(fields) => {
                let mut field_vars = BTreeMap::new();
                for (name, field_ty) in fields {
                    let v = self.instantiate_in(field_ty, vars, rigid)?;
                    field_vars.insert(*name, v);
                }
                self.structure(FlatType::Record(field_vars))
            }
            Ty::Var(id) => {
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

    fn constrain_def(&mut self, def: &canon::Def) -> DResult<()> {
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
                let mut rigid_vars = BTreeMap::new();
                let mut local = BTreeMap::new();
                let mut cursor = ty;
                for pat in patterns {
                    let (arg_ty, rest) = match cursor {
                        canon::Type::Lambda(a, b) => (a.as_ref(), b.as_ref()),
                        // The binding writes more parameter patterns than its
                        // annotation has arrows (`f a b = …` with `f : Int`).
                        // Parse-don't-validate: surface a user-facing
                        // SKY-T0004 with the binding span + the written
                        // signature, not a CompilerBug.
                        _ => return Err(self.too_many_parameters(name, ty)),
                    };
                    let arg = self.normalize_annotation_ty(from_canon(arg_ty), name.span)?;
                    let arg_var = self.instantiate_rigid(&arg, &mut rigid_vars)?;
                    self.constrain_pattern(&mut local, pat, arg_var)?;
                    cursor = rest;
                }
                let ret_ty = self.normalize_annotation_ty(from_canon(cursor), name.span)?;
                let ret_var = self.instantiate_rigid(&ret_ty, &mut rigid_vars)?;
                let body_var = self.constrain_expr(&local, body)?;
                self.eq(body.span, body_var, ret_var);
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
                self.typed_rigids.push((name.value, var_rigids));
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

    /// Build the SKY-T0004 diagnostic for a binding with more parameter
    /// patterns than its annotation has arrows. Resolving the name / rendering
    /// the signature can itself only fail on a forged symbol, in which case
    /// that internal bug is surfaced instead.
    fn too_many_parameters(
        &self,
        name: &sky_diagnostics::Located<Symbol>,
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

    /// Reduce a 2-arg `Task Error a` annotation type to the internal unary
    /// `Task a`, validating that the error channel is the `Error` type, and
    /// recursively normalise nested occurrences in any composite type.
    ///
    /// Sky mandates `Task Error a` as the canonical user-facing form, but the
    /// type-checker's internal model is unary `Task a` — the error channel is
    /// always `Error` and therefore implicit in the IR.  This bridge is applied
    /// to every result of [`from_canon`] so user annotations unify with the
    /// kernel-built unary forms.
    ///
    /// # Errors
    ///
    /// Returns `SKY-T0001` when the error channel is not `Error` (e.g.
    /// `Task String a` or `Task Int a`).  Returns a `CompilerBug` when a
    /// `Task` annotation has a number of type arguments other than 1 or 2
    /// (canonicalisation rules out arity-0 or arity-3+ applications).
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
                                // Render both sides for a clear SKY-T0001 diagnostic.
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
                        n => Err(Diagnostic::CompilerBug {
                            where_: STAGE,
                            detail: format!(
                                "Task annotation with {n} type argument(s); expected 1 or 2"
                            ),
                        }),
                    }
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
            Ty::Record(fields) => {
                let fields = fields
                    .into_iter()
                    .map(|(k, v)| self.normalize_annotation_ty(v, span).map(|v| (k, v)))
                    .collect::<DResult<_>>()?;
                Ok(Ty::Record(fields))
            }
            // Leaf types: pass through unchanged.
            other @ (Ty::Var(_) | Ty::Unit) => Ok(other),
        }
    }

    /// Check whether `ty` is the built-in `Error` type — a nullary type
    /// constructor named `"Error"`.  The module path is intentionally ignored so
    /// both bare `Error` and fully-qualified `Sky.Core.Error.Error` are accepted.
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
            self.scheme_apps.push(SchemeApp { name, vars, span });
            Ok(var)
        } else if let Some(v) = self.untyped.get(&key).copied() {
            Ok(v)
        } else {
            Err(Diagnostic::CompilerBug {
                where_: "sky_types::constrain_var_top_level",
                detail: format!(
                    "unknown top-level binding (symbol {}); \
                     post-link every name must be in top_level or untyped",
                    name.as_raw()
                ),
            })
        }
    }

    /// The Sky `comparable`-key obligation a kernel's element/key variable
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
    ///   every Set / Dict kernel) carries the Sky `comparable`-key obligation
    ///   ([`Self::key_obligation_for`]). The base scheme (now in
    ///   [`Self::stdlib_scheme`]) is instantiated, then variable 0 is tied to a
    ///   fresh super-typed variable carrying that obligation, so a
    ///   non-comparable element / key (record, ADT, function) fails closed
    ///   instead of emitting an unbounded `set_insert::<T>` / `dict_insert::<T>`
    ///   call `cargo` rejects, and a generic `a -> Set a` lifts `Ord` (Set) /
    ///   `Hash + Eq + Ord` (Dict) onto its annotation skolem (see `bounds_for`).
    ///   This is also more conservative than Sky's runtime, which keys a Set /
    ///   Dict on a stringified value.
    fn constrain_var_kernel(
        &mut self,
        id: Option<StdlibKernel>,
        module: Symbol,
        name: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        // ── Obligation pre-checks (Phase D: re-keyed off the resolved `id`,
        //    not a re-inspected module string). They live OUTSIDE the scheme
        //    tables and must fire BEFORE the registry/legacy delegation, so the
        //    bounded super-var reaches the caller instead of the bare base
        //    scheme now sitting in `stdlib_scheme`. ──
        if let Some(k) = id {
            // `Math.min` / `Math.max`: `Comparable a => a -> a -> a`. The bounded
            // super-var (reused across BOTH arrow argument positions AND the
            // result) is what rejects `Math.min f g` / `Math.min recA recB`
            // (M4c gate, `golden_m4c_math_gate`). This is a DIRECT-build bounded
            // scheme, NOT `stdlib_scheme` + a tie, because min/max's base scheme
            // has three independent `var(0)`s and the gate needs all three tied
            // to one bounded var.
            if matches!(k, StdlibKernel::MathMin | StdlibKernel::MathMax) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let inner = self.structure(FlatType::Fun(s, s))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // Dict / Set element-key `comparable` obligation (M4d). The base
            // scheme is relocated into `stdlib_scheme` (Phase D); we instantiate
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
                return Ok(var);
            }
        }
        // ── Parse-once dual lookup (Phase C) ──
        //
        // Migrated families (String / List / Math-minus-min/max) resolve via
        // the `StdlibKernel` id — never touching the legacy `Ty::Var(u32::MAX)`
        // fallback. An un-migrated `Some(k)` (e.g. `String.toUpper`) or a
        // `None` id (FFI `Rust.*`) falls through to the legacy string table,
        // which still carries that fallback until Phase E. A miss on BOTH is
        // fail-closed with SKY-L0108 (loud) rather than silently typed as a
        // free variable that `cargo` later rejects — the exit-0-then-cargo-fail
        // hole. (`legacy_kernel_ty` is total in Phase C, so the Err is dormant
        // in production and covered directly by `both_miss_is_fail_closed`;
        // Phase E flips the legacy fallback to `None` and the Err goes live.)
        let registry = id.and_then(|k| self.stdlib_scheme(k));
        let legacy = self.legacy_kernel_ty(module, name);
        let ty = Self::kernel_scheme_or_unsupported(registry, legacy, span)?;
        self.instantiate(&ty)
    }

    /// Combine the parse-once registry scheme (`id` path) with the legacy
    /// string-table scheme, failing closed with SKY-L0108 (`Feature::Kernels`,
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

    /// Legacy string-keyed kernel-type lookup, wrapped as `Option<Ty>` for the
    /// dual-lookup composition. Phase C keeps this **total**: it returns
    /// `Some(self.kernel_ty(..))`, which still carries the historical
    /// `Ty::Var(u32::MAX)` fallback for un-migrated kernels
    /// (`String.toUpper`, `Ui.text`, `Html.render`, …) so they do not regress.
    /// Phase E replaces the body with a sentinel-detecting variant that returns
    /// `None` for un-typed kernels, at which point
    /// [`Self::kernel_scheme_or_unsupported`] fails them closed.
    #[allow(clippy::unnecessary_wraps)] // Phase C: total; Phase E returns None for un-typed kernels
    fn legacy_kernel_ty(&self, module: Symbol, name: Symbol) -> Option<Ty> {
        Some(self.kernel_ty(module, name))
    }

    fn constrain_expr(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        e: &canon::Expr,
    ) -> DResult<VarId> {
        let span = e.span;
        let var = match &e.value {
            canon::Expr_::Int(_) => self.int_var()?,
            canon::Expr_::Float(_) => self.float_var()?,
            canon::Expr_::Str(_) => self.string_var()?,
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
                // Phase C: the pre-resolved `id` selects the parse-once
                // registry scheme (`stdlib_scheme`) for migrated families,
                // falling back to the legacy symbol-keyed table otherwise.
                self.constrain_var_kernel(*id, *module, *name, span)?
            }
            canon::Expr_::VarCtor {
                home,
                type_name,
                name,
                ..
            } => self.constrain_var_ctor(home, *type_name, *name)?,
            canon::Expr_::Call(callee, args) => {
                let callee_var = self.constrain_expr(local, callee)?;
                let mut arg_vars = Vec::with_capacity(args.len());
                for a in args {
                    arg_vars.push(self.constrain_expr(local, a)?);
                }
                let ret = self.flex()?;
                // Fold a right-associative arrow: a0 -> a1 -> … -> ret.
                let mut expected = ret;
                for av in arg_vars.into_iter().rev() {
                    expected = self.structure(FlatType::Fun(av, expected))?;
                }
                self.eq(callee.span, callee_var, expected);
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
                // `in` body. The whole `let`'s type is the body's type. (M1 does
                // not generalise let-bound names — no let-polymorphism.)
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
                // type. Mirrors `Sky.Type.Constrain.Expression.constrainIf`.
                let result = self.flex()?;
                for (cond, body) in branches {
                    let cond_var = self.constrain_expr(local, cond)?;
                    let want_bool = self.bool_var()?;
                    self.eq(cond.span, cond_var, want_bool);
                    let body_var = self.constrain_expr(local, body)?;
                    self.eq(body.span, body_var, result);
                }
                let else_var = self.constrain_expr(local, else_expr)?;
                self.eq(else_expr.span, else_var, result);
                result
            }
            canon::Expr_::Tuple(elems) => {
                // A tuple's type is the product of its elements' types, each
                // constrained independently. Mirrors
                // `Sky.Type.Constrain.Expression`'s tuple arm.
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
        self.regions.insert(span, var);
        Ok(var)
    }

    /// Constrain a lambda `\p0 p1 ... -> body`. Each parameter gets a fresh
    /// flexible variable bound in the body's scope; the body is constrained
    /// there. The lambda's type is the right-nested arrow `p0 -> p1 -> … -> body`,
    /// so a surrounding `Call` unifies its callee against exactly this shape.
    /// Mirrors `Sky.Type.Constrain.Expression`'s lambda arm.
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
        self.structure(FlatType::Record(field_vars))
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
                    // `Node`) is a user error, surfaced as SKY-T0013 rather than
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
                    // sub-pattern's typing too; the lowerer is what restricts M3a
                    // payloads to variables / wildcards.
                    let (arg_vars, result_var) = self.instantiate_ctor(&scheme)?;
                    self.eq(pat.span, result_var, scrut_var);
                    for (sub, av) in args.iter().zip(arg_vars) {
                        self.constrain_pattern(local, sub, av)?;
                    }
                } else {
                    // A constructor with no registered scheme (imported, outside the
                    // single-module subset): fall back to the bare enum type, sound
                    // for the nullary case.
                    let ctor = self.con_var(home.clone(), *type_name, Vec::new())?;
                    self.eq(pat.span, ctor, scrut_var);
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
                    });
                    local.insert(f.value, result);
                }
                Ok(())
            }
            // A literal pattern pins the scrutinee to the literal's type. It
            // binds no names. A mismatch (`case n of "x" -> …` with `n : Int`)
            // surfaces as the ordinary SKY-T0001 type mismatch.
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
        }
    }

    /// Build the SKY-T0013 diagnostic for a constructor pattern that binds the
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

    /// Parse-once type scheme for a **migrated** stdlib kernel, keyed by the
    /// pre-resolved [`StdlibKernel`] id carried on the `VarKernel` node.
    /// `None` = the kernel is not yet migrated into the registry, so the caller
    /// ([`Self::constrain_var_kernel`]) falls back to the legacy symbol-keyed
    /// [`Self::kernel_ty`] table.
    ///
    /// Phase C migrates **String → List → Math**, EXCLUDING `Math.min` /
    /// `Math.max` (they keep their dedicated `Comparable`-obligation path in
    /// `constrain_var_kernel`, so the M4c gate does not reopen). Every arm here
    /// is a byte-faithful copy of the corresponding `kernel_ty` arm; the
    /// structural `Ty`-equality is pinned per-kernel by the
    /// `stdlib_scheme_matches_legacy` parity tripwire, and the exact migrated
    /// set is pinned by `migrated_set_burndown`.
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
        // ── Phase D relocation closures (mirror `kernel_ty`'s preamble so the
        //    relocated arms produce structurally identical `Ty` values; the
        //    `stdlib_scheme_matches_legacy` tripwire proves the equality). ──
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
        let error_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.error,
            args: Vec::new(),
        };
        let tuple2 = |a: Ty, b: Ty| Ty::Tuple(vec![a, b]);
        // `task(a)` — `Task a` (the error channel is the implicit `SkyError`).
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
        let req = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_request,
            args: Vec::new(),
        };
        let resp = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_response,
            args: Vec::new(),
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
        let attr = |m: Ty| Ty::Con {
            module: Vec::new(),
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
        // Nullary Std.Ui plain types (`Length` / `Color`) — lowered to
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
        // `value()` — the opaque `Value = any` JSON node produced/consumed by the
        // `JsonEnc.*` encoders. Lowered to `IrType::Json` (`JsonVal`).
        let value = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.json_value,
            args: Vec::new(),
        };
        let live_req = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.live_req,
            args: Vec::new(),
        };
        let live_route = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.live_route_con,
            args: Vec::new(),
        };
        // `HttpResponse = { body : String, headers : Dict String String, status : Int }`
        let http_response = || {
            let mut resp_fields = BTreeMap::new();
            resp_fields.insert(self.builtins.http_f_body, string());
            resp_fields.insert(self.builtins.http_f_headers, dict(string(), string()));
            resp_fields.insert(self.builtins.http_f_status, int());
            Ty::Record(resp_fields)
        };
        // `HttpRequest = { body, followRedirects, headers, maxRedirects, method, timeout, url }`
        let http_request = || {
            let mut req_fields = BTreeMap::new();
            req_fields.insert(self.builtins.http_f_body, string());
            req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty());
            req_fields.insert(
                self.builtins.http_f_headers,
                list(tuple2(string(), string())),
            );
            req_fields.insert(self.builtins.http_f_max_redirects, int());
            req_fields.insert(self.builtins.http_f_method, string());
            req_fields.insert(self.builtins.http_f_timeout, int());
            req_fields.insert(self.builtins.http_f_url, string());
            Ty::Record(req_fields)
        };
        Some(match k {
            // ── String ──
            K::StringFromInt => fun(int(), string()),
            K::StringFromFloat => fun(float(), string()),

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

            // ── Math (min / max stay on the obligation path — NOT migrated) ──
            // Constants — bare Float values (arity 0).
            K::MathPi | K::MathE | K::MathPhi | K::MathSqrt2 | K::MathInf | K::MathNan => float(),
            // abs : Int -> Int.
            K::MathAbs => fun(int(), int()),
            // Arity-1 Float -> Float.
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
            | K::MathAtanh => fun(float(), float()),
            // Arity-1 Float -> Int (rounding functions).
            K::MathFloor | K::MathCeil | K::MathRound | K::MathTrunc => fun(float(), int()),
            // Arity-2 Float -> Float -> Float.
            K::MathPow | K::MathHypot | K::MathAtan2 | K::MathMod | K::MathRemainder => {
                fun(float(), fun(float(), float()))
            }
            // Math.min / max — BASE scheme only (the `Comparable a` obligation is
            // layered on top in `constrain_var_kernel`, keyed off the id). The
            // parity tripwire checks this base against `kernel_ty("Math","min")`;
            // production never reaches this arm for min/max (the obligation
            // pre-check early-returns the bounded scheme).
            K::MathMin | K::MathMax => fun(var(0), fun(var(0), var(0))),

            // ── Log ──
            K::LogPrintln => fun(string(), task_unit()),

            // ── Maybe ──
            K::MaybeWithDefault => fun(var(0), fun(maybe(var(0)), var(0))),
            K::MaybeMap => fun(fun(var(0), var(1)), fun(maybe(var(0)), maybe(var(1)))),
            K::MaybeAndThen => fun(
                fun(var(0), maybe(var(1))),
                fun(maybe(var(0)), maybe(var(1))),
            ),

            // ── Result ──
            K::ResultWithDefault => fun(var(0), fun(result(var(1), var(0)), var(0))),
            K::ResultMap => fun(
                fun(var(0), var(1)),
                fun(result(var(2), var(0)), result(var(2), var(1))),
            ),

            // ── Bytes ──
            K::BytesEmpty => bytes(),
            K::BytesLength => fun(bytes(), int()),
            K::BytesIsEmpty => fun(bytes(), bool_ty()),
            K::BytesFromString => fun(string(), bytes()),
            K::BytesToString => fun(bytes(), maybe(string())),
            K::BytesFromHex | K::BytesFromBase64 => fun(string(), maybe(bytes())),
            K::BytesToHex | K::BytesToBase64 => fun(bytes(), string()),
            K::BytesAppend => fun(bytes(), fun(bytes(), bytes())),
            K::BytesSlice => fun(int(), fun(int(), fun(bytes(), bytes()))),

            // ── Task ──
            K::TaskSucceed => fun(var(0), task(var(0))),
            K::TaskFail => fun(var(1), task(var(0))),
            K::TaskMap => fun(fun(var(0), var(1)), fun(task(var(0)), task(var(1)))),
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
            K::TaskRun => fun(task(var(0)), result(var(1), var(0))),

            // ── Io / File / System: String -> Task () ──
            K::IoWriteStdout
            | K::IoWriteStderr
            | K::FileRemove
            | K::FileMkdirAll
            | K::FileDelete
            | K::SystemUnsetenv => fun(string(), task_unit()),
            // () -> Task String
            K::IoReadLine | K::SystemCwd => fun(Ty::Unit, task(string())),

            // ── Time ──
            K::TimeNow | K::TimeUnixMillis => fun(Ty::Unit, task(int())),
            K::TimeSleep => fun(int(), task_unit()),
            K::TimeEvery => fun(int(), fun(var(0), sub(var(0)))),

            // ── System ──
            K::SystemGetenv | K::FileReadFile | K::FileTempFile | K::FileTempDir => {
                fun(string(), task(string()))
            }
            K::SystemGetenvOr => fun(string(), fun(string(), string())),
            K::SystemArgs => fun(Ty::Unit, task(list(string()))),
            K::SystemLoadEnv => fun(Ty::Unit, task_unit()),
            K::SystemSetenv | K::FileWriteFile | K::FileAppend | K::FileCopy | K::FileRename => {
                fun(string(), fun(string(), task_unit()))
            }
            K::SystemGetArg => fun(int(), task(maybe(string()))),
            K::SystemGetenvInt => fun(string(), task(int())),
            K::SystemGetenvBool | K::FileExists | K::FileIsDir => fun(string(), task(bool_ty())),
            K::SystemExit => fun(int(), var(0)),

            // ── Random ──
            K::RandomInt => fun(int(), fun(int(), task(int()))),
            K::RandomFloat => fun(float(), fun(float(), task(float()))),
            K::RandomChoice => fun(list(var(0)), task(var(0))),

            // ── File (remaining) ──
            K::FileReadDir => fun(string(), task(list(string()))),
            K::FileReadFileLimit => fun(string(), fun(int(), task(string()))),
            K::FileReadFileBytes => fun(string(), task(list(int()))),

            // ── Http ──
            K::HttpGet => fun(string(), task(http_response())),
            K::HttpPost => fun(string(), fun(string(), task(http_response()))),
            K::HttpRequest => fun(http_request(), task(http_response())),
            K::HttpParseQuery => fun(string(), dict(string(), string())),
            K::HttpDefaultRequest => fun(string(), http_request()),
            K::HttpWithMethod => fun(string(), fun(http_request(), http_request())),
            K::HttpWithTimeout => fun(int(), fun(http_request(), http_request())),
            K::HttpWithBody => fun(string(), fun(http_request(), http_request())),
            K::HttpWithHeader => fun(string(), fun(string(), fun(http_request(), http_request()))),

            // ── Cmd ──
            K::CmdNone => cmd(var(0)),
            K::CmdBatch => fun(list(cmd(var(0))), cmd(var(0))),
            K::CmdPerform => fun(
                task(var(0)),
                fun(fun(result(error_ty(), var(0)), var(1)), cmd(var(1))),
            ),

            // ── Sub ──
            K::SubNone => sub(var(0)),
            K::SubBatch => fun(list(sub(var(0))), sub(var(0))),
            K::SubEvery => fun(int(), fun(var(0), sub(var(0)))),

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

            // ── RateLimit ──
            K::RateLimitAllow => fun(string(), fun(string(), fun(int(), fun(int(), bool_ty())))),

            // ── Db ──
            K::DbConnect => fun(Ty::Unit, task(db())),
            K::DbOpen => fun(string(), fun(string(), task(db()))),
            K::DbClose => fun(db(), task_unit()),
            K::DbExecRaw => fun(db(), fun(string(), task(int()))),
            K::DbExec => fun(db(), fun(string(), fun(list(sqlvalue()), task(int())))),
            K::DbQuery => fun(
                db(),
                fun(
                    string(),
                    fun(list(sqlvalue()), task(list(dict(string(), string())))),
                ),
            ),
            K::DbQueryDecode => fun(
                db(),
                fun(
                    string(),
                    fun(list(sqlvalue()), fun(dec(var(0)), task(list(var(0))))),
                ),
            ),
            K::DbGetString | K::DbGetField => {
                fun(string(), fun(dict(string(), string()), string()))
            }
            K::DbGetInt => fun(string(), fun(dict(string(), string()), int())),
            K::DbGetBool => fun(string(), fun(dict(string(), string()), bool_ty())),
            K::DbInsertRow => fun(
                db(),
                fun(string(), fun(list(tuple2(string(), string())), task(int()))),
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
                    fun(string(), fun(list(tuple2(string(), string())), task(int()))),
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
            K::DbUnsafeFindWhere => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(list(string()), task(list(dict(string(), string())))),
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
            K::DbMigrate => fun(
                db(),
                fun(list(tuple2(string(), string())), task(list(string()))),
            ),

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

            // ── Std.Ui layout / element / event (already schemed in kernel_ty) ──
            K::UiLayout => fun(list(attr(var(0))), fun(elem_t(var(0)), html_t(var(0)))),
            K::UiLayoutWith => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.lw_wrapper_attrs, list(attr(var(0))));
                    m.insert(self.builtins.lw_root_attrs, list(attr(var(0))));
                    m
                });
                fun(cfg_rec, fun(elem_t(var(0)), html_t(var(0))))
            }
            K::UiEl => fun(list(attr(var(0))), fun(elem_t(var(0)), elem_t(var(0)))),
            K::UiColumn | K::UiRow | K::UiWrappedRow | K::UiGrid => fun(
                list(attr(var(0))),
                fun(list(elem_t(var(0))), elem_t(var(0))),
            ),
            K::UiOnClick | K::UiOnFocus | K::UiOnBlur | K::UiOnMouseOver | K::UiOnMouseOut => {
                fun(var(0), attr(var(0)))
            }
            K::UiOnInput | K::UiOnChange | K::UiOnKeyDown | K::UiOnKeyUp => {
                fun(fun(string(), var(0)), attr(var(0)))
            }
            K::UiOnBool => fun(fun(bool_ty(), var(0)), attr(var(0))),

            // ── Std.Live app-entry (already schemed in kernel_ty) ──
            K::LiveApp => {
                let init_ret = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.live_f_init, fun(live_req(), init_ret.clone()));
                    m.insert(
                        self.builtins.live_f_update,
                        fun(var(1), fun(var(0), init_ret)),
                    );
                    m.insert(self.builtins.live_f_view, fun(var(0), html_t(var(1))));
                    m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                    m
                });
                fun(cfg_rec, task_unit())
            }
            K::LiveRoute => fun(string(), fun(fun(list(string()), var(0)), live_route())),
            K::LiveRenderStatic => fun(fun(var(0), html_t(var(1))), fun(var(0), task_unit())),

            // ── Std.Tui app-entry (already schemed in kernel_ty) ──
            K::TuiApp => {
                let tup = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                    m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                    m.insert(self.builtins.live_f_view, fun(var(0), elem_t(var(1))));
                    m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                    m.insert(
                        self.builtins.tui_f_on_key,
                        fun(string(), fun(string(), var(1))),
                    );
                    m
                });
                fun(cfg_rec, task_unit())
            }
            K::TuiProgram => {
                let tup = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                    m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                    m.insert(self.builtins.live_f_view, fun(var(0), string()));
                    m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                    m.insert(
                        self.builtins.tui_f_on_key,
                        fun(string(), fun(string(), var(1))),
                    );
                    m
                });
                fun(cfg_rec, task_unit())
            }

            // ── Std.Webview app-entry (already schemed in kernel_ty) ──
            K::WebviewApp => {
                let tup = tuple2(var(0), cmd(var(1)));
                let window_ty = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.webview_f_title, string());
                    m.insert(self.builtins.webview_f_size, tuple2(int(), int()));
                    m
                });
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                    m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                    m.insert(self.builtins.live_f_view, fun(var(0), html_t(var(1))));
                    m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                    m.insert(self.builtins.webview_f_window, window_ty);
                    m
                });
                fun(cfg_rec, task_unit())
            }

            // ══ FIRST-SCHEMED families (Phase D8–D13) ══
            // These had NO legacy scheme (`kernel_ty` → `Ty::Var(u32::MAX)`
            // hole); they receive their FIRST correct scheme here, authored from
            // the runtime signature + `.sky` HM signature. No parity oracle
            // exists (legacy was a hole), so correctness is pinned by
            // `first_schemed_were_holes` (each WAS a hole) plus skyc→cargo build
            // fixtures. Every arrow-count equals `decl().arity` — the invariant
            // `eta_expand_partial` relies on when peeling `arity` arrows off the
            // inferred callee type.

            // ── String (33 — the kernels beyond `fromInt`/`fromFloat`) ──
            K::StringLength => fun(string(), int()),
            K::StringIsEmpty | K::StringIsEmail | K::StringIsUrl => fun(string(), bool_ty()),
            K::StringReverse
            | K::StringToUpper
            | K::StringToLower
            | K::StringCasefold
            | K::StringTrim
            | K::StringTrimStart
            | K::StringTrimEnd => fun(string(), string()),
            K::StringToInt => fun(string(), maybe(int())),
            K::StringToFloat => fun(string(), maybe(float())),
            K::StringFromChar => fun(char(), string()),
            K::StringFromList => fun(list(char()), string()),
            K::StringConcat => fun(list(string()), string()),
            K::StringWords | K::StringLines => fun(string(), list(string())),
            K::StringToList => fun(string(), list(char())),
            K::StringAppend => fun(string(), fun(string(), string())),
            K::StringContains | K::StringStartsWith | K::StringEndsWith | K::StringEqualFold => {
                fun(string(), fun(string(), bool_ty()))
            }
            K::StringJoin => fun(string(), fun(list(string()), string())),
            K::StringSplit => fun(string(), fun(string(), list(string()))),
            K::StringRepeat | K::StringDropLeft | K::StringDropRight => {
                fun(int(), fun(string(), string()))
            }
            K::StringReplace => fun(string(), fun(string(), fun(string(), string()))),
            K::StringSlice => fun(int(), fun(int(), fun(string(), string()))),
            K::StringPadLeft | K::StringPadRight => {
                fun(int(), fun(char(), fun(string(), string())))
            }

            // ── Char (8) — `Char -> …`; `toLower`/`toUpper` return a 1-rune
            //    String (runtime `char_to_lower : char -> String`). ──
            K::CharIsAlpha | K::CharIsDigit | K::CharIsLower | K::CharIsUpper => {
                fun(char(), bool_ty())
            }
            K::CharToLower | K::CharToUpper => fun(char(), string()),
            K::CharToCode => fun(char(), int()),
            K::CharFromCode => fun(int(), char()),

            // ── Crypto (15) — AEAD (`aesGcm*`/`chacha20*`) now schemed: the
            //    registry `decl().arity` was corrected 3→2 to match the Rust
            //    runtime (`sky_aes_gcm_encrypt(key, plaintext)` — a fresh random
            //    nonce is prepended internally, so no third arg). Both take
            //    `key -> plaintext/ciphertext -> Result Error String`. ──
            K::CryptoSha256 | K::CryptoSha512 | K::CryptoSha1 | K::CryptoMd5 => {
                fun(string(), string())
            }
            K::CryptoHmacSha256
            | K::CryptoHmacSha512
            | K::CryptoAesKeyFromPassword
            | K::CryptoChachaKeyFromPassword => fun(string(), fun(string(), string())),
            K::CryptoRsaSha256Sign
            | K::CryptoAesGcmEncrypt
            | K::CryptoAesGcmDecrypt
            | K::CryptoChacha20Encrypt
            | K::CryptoChacha20Decrypt => {
                fun(string(), fun(string(), result(error_ty(), string())))
            }
            K::CryptoRsaSha256Verify => fun(string(), fun(string(), fun(string(), bool_ty()))),
            K::CryptoConstantTimeEqual => fun(string(), fun(string(), bool_ty())),
            K::CryptoRandomBytes | K::CryptoRandomToken => fun(int(), task(string())),

            // ── Jwt (4) — `secret -> token/claims -> Result Error String`.
            //    Decode returns the decoded claims JSON as a String; encode
            //    (`sky_jwt_encode_hs256(secret, claims_json)`) takes the secret/
            //    key and a claims-JSON String and returns the signed token — the
            //    registry `decl().arity` was corrected 3→2 to match. ──
            K::JwtDecodeHs256 | K::JwtDecodeRs256 | K::JwtEncodeHs256 | K::JwtEncodeRs256 => {
                fun(string(), fun(string(), result(error_ty(), string())))
            }

            // ── Json.Decode (17) — mirrors the already-relocated `Db.Decode`
            //    shapes (function-first `map`/`andThen`; `dec(a)` is the opaque
            //    `Decoder a`). Primitives are arity-0 bare decoders. ──
            K::JsonDecString => dec(string()),
            K::JsonDecInt => dec(int()),
            K::JsonDecFloat => dec(float()),
            K::JsonDecBool => dec(bool_ty()),
            K::JsonDecDecodeString => fun(dec(var(0)), fun(string(), result(error_ty(), var(0)))),
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

            // ── Result (internal) — `okDefault : a -> Result e a`, the Ok-wrap
            //    used during lowering (runtime `ok_res(a) -> Result e a`). ──
            K::ResultOkDefault => fun(var(0), result(var(1), var(0))),

            // ── Std.Ui Length builders (result type `Length`) — runtime
            //    `ui_px_(i64) -> Length`, `ui_fill_() -> Length`, etc. `Length`
            //    lowers to `IrType::UiPlain(UiPlain::Length)`. Arrow-count ==
            //    `decl().arity` for every arm. ──
            K::UiPx | K::UiFillPortion | K::UiVh | K::UiVw => fun(int(), length()),
            K::UiFill | K::UiContent | K::UiShrink => length(),
            K::UiMinimum | K::UiMaximum => fun(int(), fun(length(), length())),

            // ── Std.Ui Color builders (result type `Color`) — runtime
            //    `ui_rgb_(i64,i64,i64) -> Color`, `ui_rgba_(i64,i64,i64,f64) ->
            //    Color`, `ui_white_() -> Color`, etc. `Color` lowers to
            //    `IrType::UiPlain(UiPlain::Color)`. ──
            K::UiRgb => fun(int(), fun(int(), fun(int(), color()))),
            K::UiRgba => fun(int(), fun(int(), fun(int(), fun(float(), color())))),
            K::UiWhite | K::UiBlack | K::UiTransparent => color(),

            // ── Sky.Core.Json.Encode (8) — the `JsonEnc.*` encoders. `Value =
            //    any` maps to `IrType::Json` (`JsonVal`) via the `"Value"` arm in
            //    `sky_lower::ir_type_from_ty`. Runtime: `json_enc_string(String)
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

            // Not-yet-migrated / EXCLUDED. `PubSub` (`publish`/`publishNoEcho`)
            // is a KNOWN-UNBACKED exclusion — see `KNOWN_UNBACKED`: no runtime
            // fn and its qualifier is absent from canon `qual_vars`, so it is
            // unreachable and must NOT be schemed (a scheme would forge an
            // exit-0 path to an unbacked kernel). Uuid (#54) and Encoding (#55)
            // remain deferred and fall back to the legacy symbol-keyed table.
            _ => return None,
        })
    }

    /// The type of a kernel function. The wired set is `String.fromInt :
    /// Int -> String`, `String.fromFloat : Float -> String`, and `Log.println :
    /// String -> Task ()`; any other kernel is treated as fully polymorphic so
    /// it never spuriously fails inference for the supported subset.
    #[allow(clippy::too_many_lines)] // declarative kernel-type table — extracting helpers would obscure the data
    fn kernel_ty(&self, module: Symbol, name: Symbol) -> Ty {
        let int = Ty::Con {
            module: Vec::new(),
            name: self.builtins.int,
            args: Vec::new(),
        };
        let float = Ty::Con {
            module: Vec::new(),
            name: self.builtins.float,
            args: Vec::new(),
        };
        let string = Ty::Con {
            module: Vec::new(),
            name: self.builtins.string,
            args: Vec::new(),
        };
        let task_unit = Ty::Con {
            module: Vec::new(),
            name: self.builtins.task,
            args: vec![Ty::Unit],
        };
        let bool_ty = Ty::Con {
            module: Vec::new(),
            name: self.builtins.bool,
            args: Vec::new(),
        };
        // The polymorphic kernel schemes below use `Ty::Var(n)` for their type
        // variables. `instantiate` mints one fresh flexible variable per distinct
        // raw id, sharing it across every occurrence within ONE scheme — so the
        // ids only need to be distinct within a single arm (they are local to that
        // arm's instantiation), exactly like a constructor scheme's variables.
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
        let result = |e: Ty, a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.result,
            args: vec![e, a],
        };
        let dict = |k: Ty, v: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.dict,
            args: vec![k, v],
        };
        let set = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.set,
            args: vec![a],
        };
        // `bytes` is a zero-argument constructor: `Bytes`.
        let bytes = Ty::Con {
            module: Vec::new(),
            name: self.builtins.bytes,
            args: Vec::new(),
        };
        let tuple2 = |a: Ty, b: Ty| Ty::Tuple(vec![a, b]);
        // `dec(inner)` — `Decoder inner` — the opaque row-decoder type shared by
        // JSON decode and Db.Decode. Lowered to `IrType::Decoder(Box<IrType>)`.
        let dec = |inner: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.decoder,
            args: vec![inner],
        };
        match (self.interner.resolve(module), self.interner.resolve(name)) {
            (Some("String"), Some("fromInt")) => Ty::Fun(Box::new(int), Box::new(string)),
            (Some("String"), Some("fromFloat")) => Ty::Fun(Box::new(float), Box::new(string)),
            (Some("Log"), Some("println")) => Ty::Fun(Box::new(string), Box::new(task_unit)),

            // ── Sky.Core.List (kernel-anchored combinators) ──
            // map : (a -> b) -> List a -> List b
            (Some("List"), Some("map")) => {
                fun(fun(var(0), var(1)), fun(list(var(0)), list(var(1))))
            }
            // filter : (a -> Bool) -> List a -> List a
            (Some("List"), Some("filter")) => {
                fun(fun(var(0), bool_ty), fun(list(var(0)), list(var(0))))
            }
            // foldl / foldr : (a -> b -> b) -> b -> List a -> b
            (Some("List"), Some("foldl" | "foldr")) => fun(
                fun(var(0), fun(var(1), var(1))),
                fun(var(1), fun(list(var(0)), var(1))),
            ),
            // length : List a -> Int
            (Some("List"), Some("length")) => fun(list(var(0)), int),
            // head : List a -> Maybe a
            (Some("List"), Some("head")) => fun(list(var(0)), maybe(var(0))),
            // tail : List a -> Maybe (List a)
            (Some("List"), Some("tail")) => fun(list(var(0)), maybe(list(var(0)))),
            // member : a -> List a -> Bool
            (Some("List"), Some("member")) => fun(var(0), fun(list(var(0)), bool_ty)),
            // range : Int -> Int -> List Int
            (Some("List"), Some("range")) => fun(int.clone(), fun(int.clone(), list(int))),
            // reverse : List a -> List a
            (Some("List"), Some("reverse")) => fun(list(var(0)), list(var(0))),

            // ── Sky.Core.Maybe ──
            // withDefault : a -> Maybe a -> a
            (Some("Maybe"), Some("withDefault")) => fun(var(0), fun(maybe(var(0)), var(0))),
            // map : (a -> b) -> Maybe a -> Maybe b
            (Some("Maybe"), Some("map")) => {
                fun(fun(var(0), var(1)), fun(maybe(var(0)), maybe(var(1))))
            }
            // andThen : (a -> Maybe b) -> Maybe a -> Maybe b
            (Some("Maybe"), Some("andThen")) => fun(
                fun(var(0), maybe(var(1))),
                fun(maybe(var(0)), maybe(var(1))),
            ),

            // ── Sky.Core.Result ── (e = the error type variable)
            // withDefault : a -> Result e a -> a
            (Some("Result"), Some("withDefault")) => {
                fun(var(0), fun(result(var(1), var(0)), var(0)))
            }
            // map : (a -> b) -> Result e a -> Result e b
            (Some("Result"), Some("map")) => fun(
                fun(var(0), var(1)),
                fun(result(var(2), var(0)), result(var(2), var(1))),
            ),

            // ── Sky.Core.Math ──
            // NOTE: `Math.min` / `Math.max` do NOT use this arm — they are handled
            // on a dedicated path in the `VarKernel` walk that mints the shared
            // variable with the ORDERING obligation (`Comparable a => a -> a -> a`,
            // Elm Basics-conformant). This bare `var(0)` table entry would emit an
            // UNBOUNDED variable, which lowers to a `math_min<T>(…)` call that
            // `cargo` rejects (the runtime helper requires `T: PartialOrd`); the
            // bounded path fails closed at type-check on non-comparable arguments
            // instead. Kept only as a safety net should the dedicated path ever be
            // bypassed; it is unreachable in normal lowering. The no-truncation /
            // type-preserving behaviour (Divergence from Sky, PR #136 — Sky
            // routes through AsInt; we follow Elm's polymorphic comparable;
            // rationale: Elm-conformance) is a property of the runtime compare
            // the bounded variable lowers to.
            (Some("Math"), Some("min" | "max")) => fun(var(0), fun(var(0), var(0))),
            // Constants — bare Float values (arity 0).
            (Some("Math"), Some("pi" | "e" | "phi" | "sqrt2" | "inf" | "nan")) => float,
            // abs : Int -> Int.
            (Some("Math"), Some("abs")) => fun(int.clone(), int),
            // Arity-1 Float -> Float.
            (
                Some("Math"),
                Some(
                    "sqrt" | "cbrt" | "exp" | "exp2" | "log" | "log2" | "log10" | "sin" | "cos"
                    | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "asinh"
                    | "acosh" | "atanh",
                ),
            ) => fun(float.clone(), float),
            // Arity-1 Float -> Int (rounding functions).
            (Some("Math"), Some("floor" | "ceil" | "round" | "trunc")) => fun(float, int),
            // Arity-2 Float -> Float -> Float.
            (Some("Math"), Some("pow" | "hypot" | "atan2" | "mod" | "remainder")) => {
                fun(float.clone(), fun(float.clone(), float))
            }

            // ── Sky.Core.Dict (M4d) ──
            // NOTE: the key variable `var(0)` in every Dict arm below is written
            // bare here, but the `VarKernel` walk does NOT take this scheme as-is
            // for `Dict` kernels: it instantiates the scheme and then ties raw
            // scheme-variable 0 (the key) to a fresh super-typed variable
            // carrying the Sky `comparable`-key obligation (`TyBounds::dict_key`,
            // → Rust `Hash + Eq + Ord`). So a non-comparable key fails closed at
            // type-check, and a generic key lifts the bound onto the annotation
            // skolem rather than emitting an unbounded `dict_*::<T>` call `cargo`
            // rejects. The bare scheme is the SHAPE; the obligation is attached
            // on the dedicated path (see `key_obligation` + the `VarKernel` arm).
            // empty : Dict k v  — arity-0 polymorphic value.
            (Some("Dict"), Some("empty")) => dict(var(0), var(1)),
            // isEmpty : Dict k v -> Bool
            (Some("Dict"), Some("isEmpty")) => fun(dict(var(0), var(1)), bool_ty),
            // size : Dict k v -> Int
            (Some("Dict"), Some("size")) => fun(dict(var(0), var(1)), int),
            // insert : k -> v -> Dict k v -> Dict k v
            (Some("Dict"), Some("insert")) => fun(
                var(0),
                fun(var(1), fun(dict(var(0), var(1)), dict(var(0), var(1)))),
            ),
            // get : k -> Dict k v -> Maybe v
            (Some("Dict"), Some("get")) => fun(var(0), fun(dict(var(0), var(1)), maybe(var(1)))),
            // remove : k -> Dict k v -> Dict k v
            (Some("Dict"), Some("remove")) => {
                fun(var(0), fun(dict(var(0), var(1)), dict(var(0), var(1))))
            }
            // member : k -> Dict k v -> Bool
            (Some("Dict"), Some("member")) => fun(var(0), fun(dict(var(0), var(1)), bool_ty)),
            // keys : Dict k v -> List k
            (Some("Dict"), Some("keys")) => fun(dict(var(0), var(1)), list(var(0))),
            // values : Dict k v -> List v
            (Some("Dict"), Some("values")) => fun(dict(var(0), var(1)), list(var(1))),
            // toList : Dict k v -> List (k, v)
            (Some("Dict"), Some("toList")) => {
                fun(dict(var(0), var(1)), list(tuple2(var(0), var(1))))
            }
            // fromList : List (k, v) -> Dict k v
            (Some("Dict"), Some("fromList")) => {
                fun(list(tuple2(var(0), var(1))), dict(var(0), var(1)))
            }
            // map : (k -> a -> b) -> Dict k a -> Dict k b
            (Some("Dict"), Some("map")) => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dict(var(0), var(1)), dict(var(0), var(2))),
            ),
            // foldl : (k -> v -> b -> b) -> b -> Dict k v -> b
            (Some("Dict"), Some("foldl")) => fun(
                fun(var(0), fun(var(1), fun(var(2), var(2)))),
                fun(var(2), fun(dict(var(0), var(1)), var(2))),
            ),
            // union : Dict k v -> Dict k v -> Dict k v  (left-biased)
            (Some("Dict"), Some("union")) => fun(
                dict(var(0), var(1)),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),

            // ── Sky.Core.Set (M4d) ──
            // NOTE: the element variable `var(0)` in every Set arm is written
            // bare here; the `VarKernel` walk ties raw scheme-variable 0 (the
            // element) to a fresh super-typed variable carrying the Sky
            // `comparable`-key obligation (`TyBounds::set_elem`, → Rust `Ord`),
            // exactly as the Dict arms above. See `key_obligation`.
            // empty : Set a  — arity-0 polymorphic value.
            (Some("Set"), Some("empty")) => set(var(0)),
            // size : Set a -> Int
            (Some("Set"), Some("size")) => fun(set(var(0)), int),
            // insert : a -> Set a -> Set a
            // remove : a -> Set a -> Set a
            (Some("Set"), Some("insert" | "remove")) => fun(var(0), fun(set(var(0)), set(var(0)))),
            // member : a -> Set a -> Bool
            (Some("Set"), Some("member")) => fun(var(0), fun(set(var(0)), bool_ty)),
            // toList : Set a -> List a
            (Some("Set"), Some("toList")) => fun(set(var(0)), list(var(0))),
            // fromList : List a -> Set a
            (Some("Set"), Some("fromList")) => fun(list(var(0)), set(var(0))),
            // union : Set a -> Set a -> Set a
            // intersect : Set a -> Set a -> Set a
            // diff : Set a -> Set a -> Set a
            (Some("Set"), Some("union" | "intersect" | "diff")) => {
                fun(set(var(0)), fun(set(var(0)), set(var(0))))
            }

            // ── Sky.Core.Bytes (M4e) ─────────────────────────────────────
            // Divergence from Sky: Bytes is Vec<u8> not a String alias;
            // conversions are explicit and toString returns Maybe String.
            //
            // empty : Bytes  — arity-0 value.
            (Some("Bytes"), Some("empty")) => bytes,
            // length : Bytes -> Int
            (Some("Bytes"), Some("length")) => fun(bytes, int),
            // isEmpty : Bytes -> Bool
            (Some("Bytes"), Some("isEmpty")) => fun(bytes, bool_ty),
            // fromString : String -> Bytes
            (Some("Bytes"), Some("fromString")) => fun(string, bytes),
            // toString : Bytes -> Maybe String
            (Some("Bytes"), Some("toString")) => fun(bytes, maybe(string)),
            // fromHex | fromBase64 : String -> Maybe Bytes
            (Some("Bytes"), Some("fromHex" | "fromBase64")) => fun(string, maybe(bytes)),
            // toHex | toBase64 : Bytes -> String
            (Some("Bytes"), Some("toHex" | "toBase64")) => fun(bytes, string),
            // append : Bytes -> Bytes -> Bytes
            (Some("Bytes"), Some("append")) => fun(bytes.clone(), fun(bytes.clone(), bytes)),
            // slice : Int -> Int -> Bytes -> Bytes
            (Some("Bytes"), Some("slice")) => fun(int.clone(), fun(int, fun(bytes.clone(), bytes))),

            // ── Sky.Core.Task (M5a) ──────────────────────────────────────────
            // A helper closure to build `Task a` with one success-type argument.
            // The HM error type is always the implicit `SkyError` alias, so only
            // the success type is carried in the IR.
            //
            // succeed : a -> Task a
            (Some("Task"), Some("succeed")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                fun(var(0), task_a)
            }
            // fail : e -> Task a   (e is unconstrained — the error channel)
            (Some("Task"), Some("fail")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                fun(var(1), task_a)
            }
            // map : (a -> b) -> Task a -> Task b
            (Some("Task"), Some("map")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let task_b = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(1)],
                };
                fun(fun(var(0), var(1)), fun(task_a, task_b))
            }
            // andThen : (a -> Task b) -> Task a -> Task b
            (Some("Task"), Some("andThen")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let task_b_inner = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(1)],
                };
                let task_b_outer = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(1)],
                };
                fun(fun(var(0), task_b_inner), fun(task_a, task_b_outer))
            }
            // mapError : (Error -> Error) -> Task a -> Task a
            // Pin the handler's parameter and return to the fixed `Error` type so
            // `\e -> ...` infers `e : Error` without an unconstrained free variable
            // (which would stay polymorphic → SKY-L0102).
            (Some("Task"), Some("mapError")) => {
                let error_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.error,
                    args: Vec::new(),
                };
                let task_a_in = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let task_a_out = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                fun(fun(error_ty.clone(), error_ty), fun(task_a_in, task_a_out))
            }
            // onError : (Error -> Task a) -> Task a -> Task a
            // Pin the handler's parameter to the fixed `Error` type so `\e -> ...`
            // infers `e : Error` without an unconstrained free variable (SKY-L0102).
            (Some("Task"), Some("onError")) => {
                let error_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.error,
                    args: Vec::new(),
                };
                let task_a_f = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let task_a_in = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let task_a_out = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                fun(fun(error_ty, task_a_f), fun(task_a_in, task_a_out))
            }
            // fromResult : Result e a -> Task a
            (Some("Task"), Some("fromResult")) => {
                let res = result(var(0), var(1));
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(1)],
                };
                fun(res, task_a)
            }
            // andThenResult : (a -> Result e b) -> Task a -> Task b
            (Some("Task"), Some("andThenResult")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let task_b = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(2)],
                };
                fun(fun(var(0), result(var(1), var(2))), fun(task_a, task_b))
            }
            // sequence : List (Task a) -> Task (List a)
            (Some("Task"), Some("sequence" | "parallel")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let task_list_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![list(var(0))],
                };
                fun(list(task_a), task_list_a)
            }
            // run : Task a -> Result e a
            (Some("Task"), Some("run")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                fun(task_a, result(var(1), var(0)))
            }

            // ── Sky.Core.Io / File: String -> Task () ────────────────────────
            // Io.writeStdout, Io.writeStderr, File.remove, File.mkdirAll,
            // File.delete, File.unsetenv — all share String -> Task () shape.
            (Some("Io"), Some("writeStdout" | "writeStderr"))
            | (Some("File"), Some("remove" | "mkdirAll" | "delete"))
            | (Some("System"), Some("unsetenv")) => fun(string, task_unit),

            // ── () -> Task String: Io.readLine, System.cwd ───────────────────
            (Some("Io"), Some("readLine")) | (Some("System"), Some("cwd")) => {
                let task_string = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![string],
                };
                fun(Ty::Unit, task_string)
            }

            // ── Sky.Core.Time (M5a) ──────────────────────────────────────────
            // now : () -> Task Int
            // unixMillis : () -> Task Int
            (Some("Time"), Some("now" | "unixMillis")) => {
                let task_int = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![int],
                };
                fun(Ty::Unit, task_int)
            }
            // sleep : Int -> Task ()
            (Some("Time"), Some("sleep")) => fun(int, task_unit),

            // ── Sky.Core.System (M5a) ────────────────────────────────────────
            // getenv : String -> Task String
            // File.readFile : String -> Task String
            // File.tempFile, File.tempDir : String -> Task String
            (Some("System"), Some("getenv"))
            | (Some("File"), Some("readFile" | "tempFile" | "tempDir")) => {
                let task_string = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![string.clone()],
                };
                fun(string, task_string)
            }
            // getenvOr : String -> String -> String   (pure — has fallback)
            (Some("System"), Some("getenvOr")) => fun(string.clone(), fun(string.clone(), string)),
            // args : () -> Task (List String)
            (Some("System"), Some("args")) => {
                let task_list_string = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![list(string)],
                };
                fun(Ty::Unit, task_list_string)
            }
            // loadEnv : () -> Task ()
            (Some("System"), Some("loadEnv")) => fun(Ty::Unit, task_unit),
            // setenv : String -> String -> Task ()
            // File.writeFile, File.append, File.copy, File.rename: same shape
            (Some("System"), Some("setenv"))
            | (Some("File"), Some("writeFile" | "append" | "copy" | "rename")) => {
                fun(string.clone(), fun(string, task_unit))
            }
            // getArg : Int -> Task (Maybe String)
            (Some("System"), Some("getArg")) => {
                let task_maybe_string = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![maybe(string)],
                };
                fun(int, task_maybe_string)
            }
            // getenvInt : String -> Task Int
            (Some("System"), Some("getenvInt")) => {
                let task_int = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![int],
                };
                fun(string, task_int)
            }
            // getenvBool : String -> Task Bool
            // File.exists, File.isDir : String -> Task Bool
            (Some("System"), Some("getenvBool")) | (Some("File"), Some("exists" | "isDir")) => {
                let task_bool = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![bool_ty],
                };
                fun(string, task_bool)
            }
            // exit : Int -> a   (diverging — polymorphic return)
            (Some("System"), Some("exit")) => fun(int, var(0)),

            // ── Sky.Core.Random (M5a) ────────────────────────────────────────
            // int : Int -> Int -> Task Int
            (Some("Random"), Some("int")) => {
                let task_int = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![int.clone()],
                };
                fun(int.clone(), fun(int, task_int))
            }
            // float : Float -> Float -> Task Float
            (Some("Random"), Some("float")) => {
                let task_float = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![float.clone()],
                };
                fun(float.clone(), fun(float, task_float))
            }
            // choice : List a -> Task a
            (Some("Random"), Some("choice")) => {
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                fun(list(var(0)), task_a)
            }

            // ── Sky.Core.File (M5a) ──────────────────────────────────────────
            // readDir : String -> Task (List String)
            (Some("File"), Some("readDir")) => {
                let task_list_string = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![list(string.clone())],
                };
                fun(string, task_list_string)
            }
            // readFileLimit : String -> Int -> Task String
            (Some("File"), Some("readFileLimit")) => {
                let task_string = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![string.clone()],
                };
                fun(string, fun(int, task_string))
            }
            // readFileBytes : String -> Task (List Int)
            (Some("File"), Some("readFileBytes")) => {
                let task_bytes = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![list(int)],
                };
                fun(string, task_bytes)
            }

            // ── Sky.Core.Http (M5b) ──────────────────────────────────────────
            //
            // `HttpResponse = { body : String, headers : Dict String String, status : Int }`
            // `HttpRequest  = { body : String, followRedirects : Bool, headers : List (String, String),
            //                   maxRedirects : Int, method : String, timeout : Int, url : String }`
            //
            // Both record types are returned as `Ty::Record(BTreeMap<Symbol, Ty>)`
            // so the `collect_record_types` prepass in the lowerer can register the
            // synthesised struct shapes for `HttpResponse` and `HttpRequest`.
            // The field name symbols are pre-interned in `Builtins::new()` because
            // `kernel_ty` has `&self` — the interner is immutable here.
            (Some("Http"), Some("get")) => {
                // get : String -> Task HttpResponse
                let mut resp_fields = BTreeMap::new();
                resp_fields.insert(self.builtins.http_f_body, string.clone());
                resp_fields.insert(
                    self.builtins.http_f_headers,
                    dict(string.clone(), string.clone()),
                );
                // `int` is not used after this point in the arm → move, no clone.
                resp_fields.insert(self.builtins.http_f_status, int);
                let http_response = Ty::Record(resp_fields);
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![http_response],
                };
                // `string` last use → move.
                fun(string, task_resp)
            }
            (Some("Http"), Some("post")) => {
                // post : String -> String -> Task HttpResponse
                let mut resp_fields = BTreeMap::new();
                resp_fields.insert(self.builtins.http_f_body, string.clone());
                resp_fields.insert(
                    self.builtins.http_f_headers,
                    dict(string.clone(), string.clone()),
                );
                // `int` last use → move.
                resp_fields.insert(self.builtins.http_f_status, int);
                let http_response = Ty::Record(resp_fields);
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![http_response],
                };
                fun(string.clone(), fun(string, task_resp))
            }
            (Some("Http"), Some("request")) => {
                // request : HttpRequest -> Task HttpResponse
                let mut req_fields = BTreeMap::new();
                req_fields.insert(self.builtins.http_f_body, string.clone());
                // `bool_ty` not used after this point in the arm → move.
                req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty);
                req_fields.insert(
                    self.builtins.http_f_headers,
                    list(tuple2(string.clone(), string.clone())),
                );
                req_fields.insert(self.builtins.http_f_max_redirects, int.clone());
                req_fields.insert(self.builtins.http_f_method, string.clone());
                req_fields.insert(self.builtins.http_f_timeout, int.clone());
                req_fields.insert(self.builtins.http_f_url, string.clone());
                let http_request = Ty::Record(req_fields);
                let mut resp_fields = BTreeMap::new();
                resp_fields.insert(self.builtins.http_f_body, string.clone());
                // Second `string` in dict is the last use of `string` in this arm → move.
                resp_fields.insert(self.builtins.http_f_headers, dict(string.clone(), string));
                // `int` last use → move.
                resp_fields.insert(self.builtins.http_f_status, int);
                let http_response = Ty::Record(resp_fields);
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![http_response],
                };
                fun(http_request, task_resp)
            }
            (Some("Http"), Some("parseQuery")) => {
                // parseQuery : String -> Dict String String  (pure)
                fun(string.clone(), dict(string.clone(), string))
            }
            (Some("Http"), Some("defaultRequest")) => {
                // defaultRequest : String -> HttpRequest  (pure builder)
                let mut req_fields = BTreeMap::new();
                req_fields.insert(self.builtins.http_f_body, string.clone());
                // `bool_ty` not used after this point → move.
                req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty);
                req_fields.insert(
                    self.builtins.http_f_headers,
                    list(tuple2(string.clone(), string.clone())),
                );
                req_fields.insert(self.builtins.http_f_max_redirects, int.clone());
                req_fields.insert(self.builtins.http_f_method, string.clone());
                // `int` last use → move.
                req_fields.insert(self.builtins.http_f_timeout, int);
                req_fields.insert(self.builtins.http_f_url, string.clone());
                let http_request = Ty::Record(req_fields);
                // `string` last use → move.
                fun(string, http_request)
            }
            (Some("Http"), Some("withMethod")) => {
                // withMethod : String -> HttpRequest -> HttpRequest  (pure builder)
                let mut req_fields = BTreeMap::new();
                req_fields.insert(self.builtins.http_f_body, string.clone());
                // `bool_ty` not used after this point → move.
                req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty);
                req_fields.insert(
                    self.builtins.http_f_headers,
                    list(tuple2(string.clone(), string.clone())),
                );
                req_fields.insert(self.builtins.http_f_max_redirects, int.clone());
                req_fields.insert(self.builtins.http_f_method, string.clone());
                // `int` last use → move.
                req_fields.insert(self.builtins.http_f_timeout, int);
                // `string` last use in req_fields (fun(string, …) is the outer arg) → move.
                req_fields.insert(self.builtins.http_f_url, string.clone());
                let http_request_a = Ty::Record(req_fields.clone());
                let http_request_b = Ty::Record(req_fields);
                // `string` last use → move.
                fun(string, fun(http_request_a, http_request_b))
            }
            (Some("Http"), Some("withTimeout")) => {
                // withTimeout : Int -> HttpRequest -> HttpRequest  (pure builder)
                let mut req_fields = BTreeMap::new();
                req_fields.insert(self.builtins.http_f_body, string.clone());
                // `bool_ty` not used after this point → move.
                req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty);
                req_fields.insert(
                    self.builtins.http_f_headers,
                    list(tuple2(string.clone(), string.clone())),
                );
                req_fields.insert(self.builtins.http_f_max_redirects, int.clone());
                req_fields.insert(self.builtins.http_f_method, string.clone());
                // `int` at timeout is NOT the last use — `int` is also the outer `fun`
                // arg at `fun(int, …)`, so the clone here is needed.
                req_fields.insert(self.builtins.http_f_timeout, int.clone());
                // `string` last use → move.
                req_fields.insert(self.builtins.http_f_url, string);
                let http_request_a = Ty::Record(req_fields.clone());
                let http_request_b = Ty::Record(req_fields);
                // `int` last use → move.
                fun(int, fun(http_request_a, http_request_b))
            }
            (Some("Http"), Some("withBody")) => {
                // withBody : String -> HttpRequest -> HttpRequest  (pure builder)
                let mut req_fields = BTreeMap::new();
                req_fields.insert(self.builtins.http_f_body, string.clone());
                // `bool_ty` not used after this point → move.
                req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty);
                req_fields.insert(
                    self.builtins.http_f_headers,
                    list(tuple2(string.clone(), string.clone())),
                );
                req_fields.insert(self.builtins.http_f_max_redirects, int.clone());
                req_fields.insert(self.builtins.http_f_method, string.clone());
                // `int` last use → move.
                req_fields.insert(self.builtins.http_f_timeout, int);
                req_fields.insert(self.builtins.http_f_url, string.clone());
                let http_request_a = Ty::Record(req_fields.clone());
                let http_request_b = Ty::Record(req_fields);
                // `string` last use → move.
                fun(string, fun(http_request_a, http_request_b))
            }
            (Some("Http"), Some("withHeader")) => {
                // withHeader : String -> String -> HttpRequest -> HttpRequest  (pure builder)
                let mut req_fields = BTreeMap::new();
                req_fields.insert(self.builtins.http_f_body, string.clone());
                // `bool_ty` not used after this point → move.
                req_fields.insert(self.builtins.http_f_follow_redirects, bool_ty);
                req_fields.insert(
                    self.builtins.http_f_headers,
                    list(tuple2(string.clone(), string.clone())),
                );
                req_fields.insert(self.builtins.http_f_max_redirects, int.clone());
                req_fields.insert(self.builtins.http_f_method, string.clone());
                // `int` last use → move.
                req_fields.insert(self.builtins.http_f_timeout, int);
                req_fields.insert(self.builtins.http_f_url, string.clone());
                let http_request_a = Ty::Record(req_fields.clone());
                let http_request_b = Ty::Record(req_fields);
                // `string` used twice in fun(string.clone(), fun(string, …)) → clone first.
                fun(
                    string.clone(),
                    fun(string, fun(http_request_a, http_request_b)),
                )
            }

            // ── Db kernels (M5b-db) ─────────────────────────────────────────────
            // Helper type constructors for this section.
            //
            // `db_ty` : the opaque Db connection pool handle.
            // `sv_ty` : `SqlValue` — the typed SQL parameter sum.
            // `sf_ty` : `SqlField` — the PATCH-parameter sum (SetField / OmitField).
            // `row_ty`: `Dict String String` — an untyped query result row.
            // `decoder_ty(a)` : `Decoder a` — reuses the shared runtime Decoder type.
            //
            // Raw type-variable ids in this section are local to each arm (they are
            // only used within that arm's `fun(…)` chain, then discarded). A
            // distinct set of ids per arm avoids accidental sharing between different
            // kernel schemes during `instantiate`.

            // Db.connect : () -> Task Error Db
            (Some("Db"), Some("connect")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                fun(
                    Ty::Unit,
                    Ty::Con {
                        module: Vec::new(),
                        name: self.builtins.task,
                        args: vec![db_ty],
                    },
                )
            }

            // Db.open : String -> String -> Task Error Db
            (Some("Db"), Some("open")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                fun(
                    string.clone(),
                    fun(
                        string,
                        Ty::Con {
                            module: Vec::new(),
                            name: self.builtins.task,
                            args: vec![db_ty],
                        },
                    ),
                )
            }

            // Db.close : Db -> Task Error ()
            (Some("Db"), Some("close")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                fun(db_ty, task_unit)
            }

            // Db.execRaw : Db -> String -> Task Error Int
            (Some("Db"), Some("execRaw")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string,
                        Ty::Con {
                            module: Vec::new(),
                            name: self.builtins.task,
                            args: vec![int],
                        },
                    ),
                )
            }

            // Db.exec : Db -> String -> List SqlValue -> Task Error Int
            (Some("Db"), Some("exec")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let sv_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sqlvalue,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string,
                        fun(
                            list(sv_ty),
                            Ty::Con {
                                module: Vec::new(),
                                name: self.builtins.task,
                                args: vec![int],
                            },
                        ),
                    ),
                )
            }

            // Db.query : Db -> String -> List SqlValue -> Task Error (List (Dict String String))
            (Some("Db"), Some("query")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let sv_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sqlvalue,
                    args: Vec::new(),
                };
                let row_ty = dict(string.clone(), string.clone());
                fun(
                    db_ty,
                    fun(
                        string,
                        fun(
                            list(sv_ty),
                            Ty::Con {
                                module: Vec::new(),
                                name: self.builtins.task,
                                args: vec![list(row_ty)],
                            },
                        ),
                    ),
                )
            }

            // Db.queryDecode : Db -> String -> List SqlValue -> Decoder a -> Task Error (List a)
            //
            // `var(0)` is the element type `a`.  `dec(var(0))` is `Decoder a`.
            // The solver unifies `Decoder a` with whatever Db.Decode.* expression is
            // passed, binding `a` to the concrete column type (e.g. String, Int).
            // The result `Task (List a)` then carries the same concrete element type —
            // no raw `var(0)` leaks into the caller's inferred type.
            (Some("Db"), Some("queryDecode")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let sv_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sqlvalue,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string,
                        fun(
                            list(sv_ty),
                            fun(
                                dec(var(0)),
                                Ty::Con {
                                    module: Vec::new(),
                                    name: self.builtins.task,
                                    args: vec![list(var(0))],
                                },
                            ),
                        ),
                    ),
                )
            }

            // Db.getString / Db.getField : String -> Dict String String -> String  (pure)
            // Both return `String` (empty string for absent columns), NOT `Maybe
            // String`.  The `Maybe` variant is deprecated; plain-String accessor is
            // the Go-parity surface.
            (Some("Db"), Some("getString" | "getField")) => fun(
                string.clone(),
                fun(dict(string.clone(), string.clone()), string),
            ),

            // Db.getInt : String -> Dict String String -> Int  (pure)
            (Some("Db"), Some("getInt")) => {
                fun(string.clone(), fun(dict(string.clone(), string), int))
            }

            // Db.getBool : String -> Dict String String -> Bool  (pure)
            (Some("Db"), Some("getBool")) => {
                fun(string.clone(), fun(dict(string.clone(), string), bool_ty))
            }

            // Db.insertRow : Db -> String -> List (String, String) -> Task Error Int
            (Some("Db"), Some("insertRow")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            list(tuple2(string.clone(), string)),
                            Ty::Con {
                                module: Vec::new(),
                                name: self.builtins.task,
                                args: vec![int],
                            },
                        ),
                    ),
                )
            }

            // Db.getById : Db -> String -> String -> Task Error (Maybe (Dict String String))
            (Some("Db"), Some("getById")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let row_ty = dict(string.clone(), string.clone());
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            string,
                            Ty::Con {
                                module: Vec::new(),
                                name: self.builtins.task,
                                args: vec![maybe(row_ty)],
                            },
                        ),
                    ),
                )
            }

            // Db.updateById : Db -> String -> String -> List (String, String) -> Task Error Int
            (Some("Db"), Some("updateById")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            string.clone(),
                            fun(
                                list(tuple2(string.clone(), string)),
                                Ty::Con {
                                    module: Vec::new(),
                                    name: self.builtins.task,
                                    args: vec![int],
                                },
                            ),
                        ),
                    ),
                )
            }

            // Db.deleteById : Db -> String -> String -> Task Error Int
            (Some("Db"), Some("deleteById")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            string,
                            Ty::Con {
                                module: Vec::new(),
                                name: self.builtins.task,
                                args: vec![int],
                            },
                        ),
                    ),
                )
            }

            // Db.findOneByField : Db -> String -> String -> String -> Task Error (Maybe (Dict String String))
            (Some("Db"), Some("findOneByField")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let row_ty = dict(string.clone(), string.clone());
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            string.clone(),
                            fun(
                                string,
                                Ty::Con {
                                    module: Vec::new(),
                                    name: self.builtins.task,
                                    args: vec![maybe(row_ty)],
                                },
                            ),
                        ),
                    ),
                )
            }

            // Db.findManyByField : Db -> String -> String -> String -> Task Error (List (Dict String String))
            (Some("Db"), Some("findManyByField")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let row_ty = dict(string.clone(), string.clone());
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            string.clone(),
                            fun(
                                string,
                                Ty::Con {
                                    module: Vec::new(),
                                    name: self.builtins.task,
                                    args: vec![list(row_ty)],
                                },
                            ),
                        ),
                    ),
                )
            }

            // Db.findByConditions : Db -> String -> Dict String String -> Task Error (List (Dict String String))
            //
            // The runtime `db_find_by_conditions` takes `HashMap<String, String>` —
            // a `Dict String String` in Sky — for AND-joined equality conditions.
            // An earlier version incorrectly typed this as `List (String, SqlValue)`;
            // the corrected type matches the runtime signature and the CLAUDE.md spec.
            (Some("Db"), Some("findByConditions")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let row_ty = dict(string.clone(), string.clone());
                let conditions_ty = dict(string.clone(), string.clone());
                fun(
                    db_ty,
                    fun(
                        string,
                        fun(
                            conditions_ty,
                            Ty::Con {
                                module: Vec::new(),
                                name: self.builtins.task,
                                args: vec![list(row_ty)],
                            },
                        ),
                    ),
                )
            }

            // Db.unsafeFindWhere : Db -> String -> String -> List String -> Task Error (List (Dict String String))
            //
            // The runtime `db_unsafe_find_where` takes 4 parameters:
            //   conn: Db, table: String, where_clause: String, args: Vec<String>
            // The `args` parameter is the parameterized-binding channel — it is
            // essential for SQL injection safety on this sole sanctioned raw-SQL path.
            // An earlier version was missing this 4th parameter, which would have
            // forced callers to string-interpolate values into where_clause.
            (Some("Db"), Some("unsafeFindWhere")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let row_ty = dict(string.clone(), string.clone());
                fun(
                    db_ty,
                    fun(
                        string.clone(), // table
                        fun(
                            string.clone(), // where_clause
                            fun(
                                list(string), // args: List String
                                Ty::Con {
                                    module: Vec::new(),
                                    name: self.builtins.task,
                                    args: vec![list(row_ty)],
                                },
                            ),
                        ),
                    ),
                )
            }

            // Db.insertFields : Db -> String -> List (String, SqlField) -> Task Error Int
            (Some("Db"), Some("insertFields")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let sf_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sqlfield,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            list(tuple2(string, sf_ty)),
                            Ty::Con {
                                module: Vec::new(),
                                name: self.builtins.task,
                                args: vec![int],
                            },
                        ),
                    ),
                )
            }

            // Db.updateFields : Db -> String -> List (String, SqlValue) -> List (String, SqlField) -> Task Error Int
            (Some("Db"), Some("updateFields")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let sqlvalue_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sqlvalue,
                    args: Vec::new(),
                };
                let sqlfield_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sqlfield,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            list(tuple2(string.clone(), sqlvalue_ty)),
                            fun(
                                list(tuple2(string, sqlfield_ty)),
                                Ty::Con {
                                    module: Vec::new(),
                                    name: self.builtins.task,
                                    args: vec![int],
                                },
                            ),
                        ),
                    ),
                )
            }

            // Db.insertFieldsReturning : Db -> String -> List (String, SqlField) -> String -> Decoder a -> Task Error (List a)
            //
            // Same `dec(var(0))` / `list(var(0))` linkage as `queryDecode`.
            (Some("Db"), Some("insertFieldsReturning")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let sf_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sqlfield,
                    args: Vec::new(),
                };
                fun(
                    db_ty,
                    fun(
                        string.clone(),
                        fun(
                            list(tuple2(string.clone(), sf_ty)),
                            fun(
                                string,
                                fun(
                                    dec(var(0)),
                                    Ty::Con {
                                        module: Vec::new(),
                                        name: self.builtins.task,
                                        args: vec![list(var(0))],
                                    },
                                ),
                            ),
                        ),
                    ),
                )
            }

            // Db.withTransaction : Db -> (Db -> Task Error a) -> Task Error a
            (Some("Db"), Some("withTransaction")) => {
                let db_ty_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                let db_ty_b = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                // body : Db -> Task Error a  where a = var(0)
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let body_ty = fun(db_ty_b, task_a.clone());
                fun(db_ty_a, fun(body_ty, task_a))
            }

            // Db.migrate : Db -> List (String, String) -> Task Error (List String)
            (Some("Db"), Some("migrate")) => {
                let db_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.db,
                    args: Vec::new(),
                };
                // `string` appears 3 times: name-arg, version-arg, result-list-elem.
                // Clone the first two uses, leave the last as a move.
                fun(
                    db_ty,
                    fun(
                        list(tuple2(string.clone(), string.clone())),
                        Ty::Con {
                            module: Vec::new(),
                            name: self.builtins.task,
                            args: vec![list(string)],
                        },
                    ),
                )
            }

            // ── Db.Decode kernels (M5b-db) ───────────────────────────────────────
            // All use the shared `Decoder<E, T>` type (same runtime type as JsonDec).
            // `dec(T)` = `Ty::Con { name: "Decoder", args: [T] }`, which lower.rs maps
            // to `IrType::Decoder(Box<IrType>)` via the `"Decoder" if args.len() == 1`
            // branch in `ir_type_from_ty`.
            //
            // Using proper `dec(T)` types instead of bare `var(0)` prevents unsound
            // programs like `let n : Int = Db.Decode.int "c"` (a `Decoder Int` cannot
            // unify with `Int`, so the type error is correctly reported).

            // Db.Decode.string : String -> Decoder String
            (Some("Db.Decode"), Some("string")) => fun(string.clone(), dec(string)),

            // Db.Decode.int : String -> Decoder Int
            (Some("Db.Decode"), Some("int")) => fun(string, dec(int)),

            // Db.Decode.float : String -> Decoder Float
            (Some("Db.Decode"), Some("float")) => fun(string, dec(float)),

            // Db.Decode.bool : String -> Decoder Bool
            (Some("Db.Decode"), Some("bool")) => fun(string, dec(bool_ty)),

            // Db.Decode.fail : String -> Decoder a  (always-fail decoder, polymorphic)
            (Some("Db.Decode"), Some("fail")) => fun(string, dec(var(0))),

            // Db.Decode.nullable : Decoder a -> Decoder (Maybe a)
            (Some("Db.Decode"), Some("nullable")) => fun(dec(var(0)), dec(maybe(var(0)))),

            // Db.Decode.map : (a -> b) -> Decoder a -> Decoder b
            (Some("Db.Decode"), Some("map")) => {
                fun(fun(var(0), var(1)), fun(dec(var(0)), dec(var(1))))
            }

            // Db.Decode.andThen : (a -> Decoder b) -> Decoder a -> Decoder b
            (Some("Db.Decode"), Some("andThen")) => {
                fun(fun(var(0), dec(var(1))), fun(dec(var(0)), dec(var(1))))
            }

            // Db.Decode.succeed : a -> Decoder a
            (Some("Db.Decode"), Some("succeed")) => fun(var(0), dec(var(0))),

            // Db.Decode.map2 : (a -> b -> c) -> Decoder a -> Decoder b -> Decoder c
            (Some("Db.Decode"), Some("map2")) => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dec(var(0)), fun(dec(var(1)), dec(var(2)))),
            ),

            // Db.Decode.map3 : (a -> b -> c -> d) -> Decoder a -> Decoder b -> Decoder c -> Decoder d
            (Some("Db.Decode"), Some("map3")) => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(dec(var(0)), fun(dec(var(1)), fun(dec(var(2)), dec(var(3))))),
            ),

            // Db.Decode.map4 : (a->b->c->d->e) -> Decoder a -> Decoder b -> Decoder c -> Decoder d -> Decoder e
            (Some("Db.Decode"), Some("map4")) => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    dec(var(0)),
                    fun(dec(var(1)), fun(dec(var(2)), fun(dec(var(3)), dec(var(4))))),
                ),
            ),

            // Db.Decode.required : String -> Decoder a -> Decoder (a -> b) -> Decoder b
            //
            // `var(0)` = field type a; `var(1)` = accumulated record type b.
            // The accumulator decoder `Decoder (a -> b)` has inner type `Fun([a], b)`.
            (Some("Db.Decode"), Some("required")) => fun(
                string,
                fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),
            ),

            // Db.Decode.optional : String -> Decoder a -> a -> Decoder (a -> b) -> Decoder b
            //
            // Same structure as `required` with an extra `a` default-value argument.
            (Some("Db.Decode"), Some("optional")) => fun(
                string,
                fun(
                    dec(var(0)),
                    fun(var(0), fun(dec(fun(var(0), var(1))), dec(var(1)))),
                ),
            ),

            // ── M5c: TEA Cmd / Sub / Time.every kernels ──────────────────────
            //
            // `cmd(m)` / `sub(m)` are opaque one-parameter type constructors
            // lowered to `IrType::Cmd` / `IrType::Sub` in `sky_lower`.
            //
            // Cmd.none : Cmd msg
            (Some("Cmd"), Some("none")) => Ty::Con {
                module: Vec::new(),
                name: self.builtins.cmd,
                args: vec![var(0)],
            },

            // Cmd.batch : List (Cmd msg) -> Cmd msg
            (Some("Cmd"), Some("batch")) => {
                let cmd = |m: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.cmd,
                    args: vec![m],
                };
                fun(list(cmd(var(0))), cmd(var(0)))
            }

            // Cmd.perform : Task Error a -> (Result Error a -> msg) -> Cmd msg
            //
            // The error channel is pinned to `Error` (the concrete Sky runtime
            // error type), not a free variable, matching the Sky stdlib sig.
            (Some("Cmd"), Some("perform")) => {
                let error_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.error,
                    args: Vec::new(),
                };
                let task_a = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![var(0)],
                };
                let cmd = |m: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.cmd,
                    args: vec![m],
                };
                // Task Error a -> (Result Error a -> msg) -> Cmd msg
                fun(
                    task_a,
                    fun(fun(result(error_ty, var(0)), var(1)), cmd(var(1))),
                )
            }

            // Sub.none : Sub msg
            (Some("Sub"), Some("none")) => Ty::Con {
                module: Vec::new(),
                name: self.builtins.sub,
                args: vec![var(0)],
            },

            // Sub.batch : List (Sub msg) -> Sub msg
            (Some("Sub"), Some("batch")) => {
                let sub = |m: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sub,
                    args: vec![m],
                };
                fun(list(sub(var(0))), sub(var(0)))
            }

            // Sub.every : Int -> msg -> Sub msg
            (Some("Sub"), Some("every")) => {
                let sub = |m: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sub,
                    args: vec![m],
                };
                fun(int, fun(var(0), sub(var(0))))
            }

            // Time.every : Int -> msg -> Sub msg   (alias for Sub.every)
            (Some("Time"), Some("every")) => {
                let sub = |m: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sub,
                    args: vec![m],
                };
                fun(int, fun(var(0), sub(var(0))))
            }

            // ── M6: Sky.Http.Server kernels ─────────────────────────────────
            //
            // Opaque con helpers — identical to `db` and `cmd` patterns above.
            // `Request` / `Response` / `Route` / `Cookie` are all Ty::Con with
            // an empty module path, matching how the lowerer looks them up.

            // Server.get  : String -> (Request -> Task Error Response) -> Route
            // Server.post : String -> (Request -> Task Error Response) -> Route
            // Server.put  : String -> (Request -> Task Error Response) -> Route
            // Server.delete : String -> (Request -> Task Error Response) -> Route
            // Server.any  : String -> (Request -> Task Error Response) -> Route
            // Server.api  : String -> (Request -> Task Error Response) -> Route
            (Some("Server"), Some("get" | "post" | "put" | "delete" | "any" | "api")) => {
                let req = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_request,
                    args: Vec::new(),
                };
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                let route = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_route,
                    args: Vec::new(),
                };
                let error_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.error,
                    args: Vec::new(),
                };
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![resp],
                };
                // error channel: Task has one type param (the ok type) — the
                // error channel is conceptually Error but erased at the Ty level.
                let _ = error_ty; // unused — task takes 1 arg in Sky-Rust Ty
                fun(string, fun(fun(req, task_resp), route))
            }

            // Server.static : String -> String -> Route
            (Some("Server"), Some("static")) => {
                let route = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_route,
                    args: Vec::new(),
                };
                fun(string.clone(), fun(string, route))
            }

            // Server.listen : Int -> List Route -> Task Error ()
            (Some("Server"), Some("listen")) => {
                let route = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_route,
                    args: Vec::new(),
                };
                let task_unit_here = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![Ty::Unit],
                };
                fun(int, fun(list(route), task_unit_here))
            }

            // Server.text     : String -> Response
            // Server.json     : String -> Response
            // Server.html     : String -> Response
            // Server.redirect : String -> Response
            (Some("Server"), Some("text" | "json" | "html" | "redirect")) => {
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                fun(string, resp)
            }

            // Server.withStatus : Int -> Response -> Response
            (Some("Server"), Some("withStatus")) => {
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                fun(int, fun(resp.clone(), resp))
            }

            // Server.withHeader : String -> String -> Response -> Response
            (Some("Server"), Some("withHeader")) => {
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                fun(string.clone(), fun(string, fun(resp.clone(), resp)))
            }

            // Server.param       : String -> Request -> Maybe String
            // Server.queryParam  : String -> Request -> Maybe String
            // Server.header      : String -> Request -> Maybe String
            // Server.getCookie   : String -> Request -> Maybe String
            (Some("Server"), Some("param" | "queryParam" | "header" | "getCookie")) => {
                let req = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_request,
                    args: Vec::new(),
                };
                let string2 = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.string,
                    args: Vec::new(),
                };
                fun(string, fun(req, maybe(string2)))
            }

            // Server.body   : Request -> String
            // Server.path   : Request -> String
            // Server.method : Request -> String
            (Some("Server"), Some("body" | "path" | "method")) => {
                let req = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_request,
                    args: Vec::new(),
                };
                fun(req, string)
            }

            // Server.cookie : String -> String -> Cookie
            (Some("Server"), Some("cookie")) => {
                let cookie = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_cookie,
                    args: Vec::new(),
                };
                fun(string.clone(), fun(string, cookie))
            }

            // Server.withCookie : Cookie -> Response -> Response
            (Some("Server"), Some("withCookie")) => {
                let cookie = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_cookie,
                    args: Vec::new(),
                };
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                fun(cookie, fun(resp.clone(), resp))
            }

            // Middleware.withCors : List String -> (Request -> Task Error Response) -> (Request -> Task Error Response)
            (Some("Middleware"), Some("withCors")) => {
                let req = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_request,
                    args: Vec::new(),
                };
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![resp],
                };
                let handler = fun(req, task_resp);
                fun(list(string), fun(handler.clone(), handler))
            }

            // Middleware.withLogging : (Request -> Task Error Response) -> (Request -> Task Error Response)
            (Some("Middleware"), Some("withLogging")) => {
                let req = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_request,
                    args: Vec::new(),
                };
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![resp],
                };
                let handler = fun(req, task_resp);
                fun(handler.clone(), handler)
            }

            // Middleware.withBasicAuth : String -> String -> (Request -> Task Error Response) -> (Request -> Task Error Response)
            (Some("Middleware"), Some("withBasicAuth")) => {
                let req = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_request,
                    args: Vec::new(),
                };
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![resp],
                };
                let handler = fun(req, task_resp);
                fun(string.clone(), fun(string, fun(handler.clone(), handler)))
            }

            // Middleware.withRateLimit : String -> Int -> Int -> (Request -> Task Error Response) -> (Request -> Task Error Response)
            (Some("Middleware"), Some("withRateLimit")) => {
                let req = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_request,
                    args: Vec::new(),
                };
                let resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.server_response,
                    args: Vec::new(),
                };
                let task_resp = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.task,
                    args: vec![resp],
                };
                let handler = fun(req, task_resp);
                fun(
                    string,
                    fun(int.clone(), fun(int, fun(handler.clone(), handler))),
                )
            }

            // RateLimit.allow : String -> String -> Int -> Int -> Bool
            (Some("RateLimit"), Some("allow")) => fun(
                string.clone(),
                fun(string, fun(int.clone(), fun(int, bool_ty))),
            ),

            // ── M7: Std.Ui layout / element / event kernel types ──────────────────
            //
            // These schemes give the HM solver correct knowledge of the core Std.Ui
            // kernel function types.  Without them the solver treats every Ui kernel
            // as `Ty::Var(u32::MAX)` (a single flexible variable), so:
            //
            // 1. An empty attrs list `[]` passed to `Ui.layout` / `Ui.el` / etc.
            //    keeps its element type as a bare `Ty::Var` (never constrained to
            //    `Attribute msg`).  `list_elem_ir` then returns `IrType::Json`, and
            //    `emit_list` emits the annotation-free `Vec::new()` — which Rust
            //    rejects with E0283 when M cannot be inferred from any other
            //    position in the expression.
            //
            // 2. Event kernels like `Ui.onClick : msg -> Attribute msg` would not
            //    propagate their concrete `M` type (e.g. `MainMsg`) back through
            //    `Ui.el`'s scheme to the enclosing `Ui.layout`'s shared `tv`
            //    variable.  Without this propagation the outer `[]` would be
            //    annotated as `Vec::<Attribute<()>>::new()` even when the real
            //    M is `MainMsg`, causing a Rust E0308.
            //
            // Design notes:
            // • `var(0)` is the shared `msg` type variable within each arm.
            //   `instantiate` mints one fresh flexible unification variable per
            //   distinct raw id within a single arm, sharing it across every
            //   occurrence — so `var(0)` in `Attr(var(0))` and `Elem(var(0))`
            //   refers to the SAME fresh variable.
            // • `Con.module` is `Vec::new()` (empty) for all Ui type constructors
            //   here.  The T2 disambiguation in `ir_type_from_ty` only looks for
            //   "Html" in the module path to choose `HtmlAttribute` vs
            //   `UiAttribute`; an empty module path → `UiAttribute`, which is
            //   the correct choice for `Std.Ui.Attribute`.
            // • These arms cover ONLY the kernels tested by the M7 golden suite.
            //   Add further arms as new Std.Ui tests arrive.

            // ── Layout entry points ───────────────────────────────────────────
            // layout : List (Attribute msg) -> Element msg -> Html msg
            (Some("Ui"), Some("layout")) => {
                let attr = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                let elem_t = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.element,
                    args: vec![msg],
                };
                let html_t = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.html_con,
                    args: vec![msg],
                };
                fun(list(attr(var(0))), fun(elem_t(var(0)), html_t(var(0))))
            }

            // layoutWith :
            //   { wrapperAttrs : List (Attribute msg), rootAttrs : List (Attribute msg) }
            //   -> Element msg
            //   -> Html msg
            //
            // The record type for the first argument is built inline with
            // `Ty::Record(BTreeMap)`, keyed by the pre-interned field-name
            // symbols `lw_wrapper_attrs` and `lw_root_attrs`.  The shared
            // `var(0)` links both field types and the Element / Html results
            // to the same `msg` unification variable, so a concrete M in any
            // field (e.g. an event in `rootAttrs`) propagates to the rest.
            //
            // Design note: the `Ty::Record` here is an EXPECTED TYPE for the
            // first arg, not a synthesised struct.  The solver unifies the
            // call-site record literal's field types against `List(Attr tv)`,
            // giving `list_elem_ir` a concrete `List(Attribute tv)` to work
            // with.  `ir_type_from_ty_ui_msg(tv)` then returns `IrType::Unit`
            // when `tv` is a free variable (message-free render) and the
            // concrete user enum type when an event kernel pins `tv`.  The
            // lowerer's `emit_list` change for non-empty Ui-typed lists then
            // wraps the Rust `vec![...]` in a typed let so that Rust can infer
            // M even when no explicit turbofish appears on `ui_layout_with_vecs`.
            (Some("Ui"), Some("layoutWith")) => {
                let attr = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                let elem_t = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.element,
                    args: vec![msg],
                };
                let html_t = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.html_con,
                    args: vec![msg],
                };
                let cfg_rec = Ty::Record({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(self.builtins.lw_wrapper_attrs, list(attr(var(0))));
                    m.insert(self.builtins.lw_root_attrs, list(attr(var(0))));
                    m
                });
                fun(cfg_rec, fun(elem_t(var(0)), html_t(var(0))))
            }

            // ── Std.Html special nodes ────────────────────────────────────────
            // styleNode : List (Attribute msg) -> String -> Html msg
            //
            // The attribute list is a `Std.Html.Attribute` (the module path
            // carries `html_con`'s "Html" symbol so `ir_type_from_ty`'s T2
            // disambiguation selects `HtmlAttribute`, matching the runtime
            // `html_style_node_(Vec<html::Attribute<M>>, String)` signature).
            // The css body is a plain `String`; the result is `Html msg`. This
            // replaces the fail-closed `Ty::Var(u32::MAX)` fallthrough so the
            // arity-2 kernel is typed exactly (F7 arity-mis-wire fix).
            (Some("Html"), Some("styleNode")) => {
                let attr = |msg: Ty| Ty::Con {
                    module: vec![self.builtins.html_con],
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                let html_t = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.html_con,
                    args: vec![msg],
                };
                fun(list(attr(var(0))), fun(string, html_t(var(0))))
            }

            // ── Element builders ──────────────────────────────────────────────
            // el : List (Attribute msg) -> Element msg -> Element msg
            (Some("Ui"), Some("el")) => {
                let attr = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                let elem_t = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.element,
                    args: vec![msg],
                };
                fun(list(attr(var(0))), fun(elem_t(var(0)), elem_t(var(0))))
            }

            // column / row / wrappedRow / grid / paragraph / textColumn :
            //   List (Attribute msg) -> List (Element msg) -> Element msg
            (
                Some("Ui"),
                Some("column" | "row" | "wrappedRow" | "grid" | "paragraph" | "textColumn"),
            ) => {
                let attr = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                let elem_t = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.element,
                    args: vec![msg],
                };
                fun(
                    list(attr(var(0))),
                    fun(list(elem_t(var(0))), elem_t(var(0))),
                )
            }

            // ── Event kernels (all return `Attribute msg`) ────────────────────
            // Both "Ui" and "Event" qualifiers are covered here, mirroring
            // lower.rs's `("Ui" | "Event", ...)` arms exactly (Phase-1a round 2).
            // "Ui" = canonical qualifier for `import Std.Ui as Ui`.
            // "Event" = canonical qualifier for `import Std.Html.Events as Event`
            //   (env.rs L979 alias table: `("Std.Html.Events", "Event")`).
            //
            // onClick / onFocus / onBlur / onMouseOver / onMouseOut / onMsg :
            //   msg -> Attribute msg
            (
                Some("Ui" | "Event"),
                Some("onClick" | "onFocus" | "onBlur" | "onMouseOver" | "onMouseOut" | "onMsg"),
            ) => {
                let attr = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                fun(var(0), attr(var(0)))
            }

            // onInput / onChange / onKeyDown / onKeyUp / onKeyPress :
            //   (String -> msg) -> Attribute msg
            // (B3: merged from two arms to eliminate redundant_clone; round 2:
            //  widened to cover "Event" qualifier so `Event.onInput` with a
            //  wrong-typed handler is rejected by skyc, not deferred to cargo.)
            (
                Some("Ui" | "Event"),
                Some("onInput" | "onChange" | "onKeyDown" | "onKeyUp" | "onKeyPress"),
            ) => {
                let attr = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                fun(fun(string, var(0)), attr(var(0)))
            }

            // onBool : (Bool -> msg) -> Attribute msg
            // (B1: Bool-carrying events; round 2: widened to "Ui" | "Event".)
            (Some("Ui" | "Event"), Some("onBool")) => {
                let attr = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.attribute,
                    args: vec![msg],
                };
                fun(fun(bool_ty, var(0)), attr(var(0)))
            }

            // ── Std.Live / Sky.Live app-entry kernels (Phase-1b) ─────────────
            //
            // `Live.app` and `Live.appRouted` share the same core cfg-record
            // scheme; the routed variant just adds `routes` and `notFound`
            // fields.  The solver constrains `init/update/view/subscriptions`
            // to their concrete user-function types via the shared `var(0)`
            // (Model) and `var(1)` (Msg) across all four fields.
            //
            // var(0) = Model
            // var(1) = Msg
            //
            // init         : LiveReq -> (Model, Cmd Msg)
            // update       : Msg -> Model -> (Model, Cmd Msg)
            // view         : Model -> Html Msg
            // subscriptions: Model -> Sub Msg
            //
            // `Live.appRouted` is gated at lower (SKY-L0118); only `Live.app`
            // is constrained here.  The qualifier set must equal the lower
            // resolved set (no exit-0-then-cargo-fail).
            (Some("Live"), Some("app")) => {
                let live_req_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.live_req,
                    args: Vec::new(),
                };
                let cmd = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.cmd,
                    args: vec![msg],
                };
                let sub = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sub,
                    args: vec![msg],
                };
                let html = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.html_con,
                    args: vec![msg],
                };
                // (Model, Cmd Msg)
                let init_ret = tuple2(var(0), cmd(var(1)));
                // init : LiveReq -> (Model, Cmd Msg)
                let init_ty = fun(live_req_ty, init_ret.clone());
                // update : Msg -> Model -> (Model, Cmd Msg)
                let update_ty = fun(var(1), fun(var(0), init_ret));
                // view : Model -> Html Msg
                let view_ty = fun(var(0), html(var(1)));
                // subscriptions : Model -> Sub Msg
                let subs_ty = fun(var(0), sub(var(1)));
                let cfg_rec = Ty::Record({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(self.builtins.live_f_init, init_ty);
                    m.insert(self.builtins.live_f_update, update_ty);
                    m.insert(self.builtins.live_f_view, view_ty);
                    m.insert(self.builtins.live_f_subscriptions, subs_ty);
                    m
                });
                fun(cfg_rec, task_unit)
            }

            // `Live.route : String -> (List String -> Page) -> LiveRoute`
            // var(0) = Page
            (Some("Live"), Some("route")) => {
                let live_route_ty = Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.live_route_con,
                    args: Vec::new(),
                };
                // (List String -> Page) — the builder function
                let builder_ty = fun(list(string.clone()), var(0));
                fun(string, fun(builder_ty, live_route_ty))
            }

            // `Live.renderStatic : (Model -> Html Msg) -> Model -> Task Error ()`
            // var(0) = Model, var(1) = Msg
            (Some("Live"), Some("renderStatic")) => {
                let html = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.html_con,
                    args: vec![msg],
                };
                let view_ty = fun(var(0), html(var(1)));
                fun(view_ty, fun(var(0), task_unit))
            }

            // ── Std.Tui / Sky.Tui app-entry kernels (Phase-1c) ──────────────────
            //
            // Both `Tui.app` and `Tui.program` share the same 5-field closed cfg
            // shape.  The qualifier set here MUST equal the lower resolved set
            // (lower.rs:4026-4027: `("Tui","app")→TuiApp`, `("Tui","program")→TuiProgram`)
            // — any mismatch reopens the exit-0-then-cargo-fail class (task #45).
            //
            // var(0) = Model
            // var(1) = Msg
            //
            // init         : () -> (Model, Cmd Msg)
            //   Note: Tui init takes `()` (unit), NOT `LiveReq`.
            //   The runtime bound is `FInit: Fn(()) -> (Model, SkyCmd<Msg>)`.
            // update       : Msg -> Model -> (Model, Cmd Msg)
            // view         : Model -> Element Msg   (TuiApp)
            //             OR Model -> String         (TuiProgram)
            // subscriptions: Model -> Sub Msg
            // onKey        : String -> String -> Msg   (flat — bytes-matches the
            //   runtime bound `FOnKey: Fn(String, String) -> Msg`)
            //
            // HARD SOUNDNESS CONSTRAINT: `onKey` MUST be in the closed cfg.
            // The runtime calls `on_key(kind, value)` on every key event and returns
            // a `Msg` (not `Option`) — there is no total way to fabricate a `Msg`
            // without the handler.  Omitting `onKey` from the scheme would leave the
            // runtime's `FOnKey` generic unconstrained, causing a Rust E0282.
            (Some("Tui"), Some("app")) => {
                let cmd = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.cmd,
                    args: vec![msg],
                };
                let sub = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sub,
                    args: vec![msg],
                };
                let elem = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.element,
                    args: vec![msg],
                };
                // `()` in Sky is the dedicated `Ty::Unit` variant, NOT an empty
                // tuple.  An empty `Ty::Tuple` prints as `()` but does not unify
                // with the `Ty::Unit` that a `() -> …` annotation produces (would
                // surface as SKY-T0001 "expected (), found ()" at the call site).
                let unit_ty = Ty::Unit;
                // (Model, Cmd Msg)
                let tup = tuple2(var(0), cmd(var(1)));
                // init : () -> (Model, Cmd Msg)
                let init_ty = fun(unit_ty, tup.clone());
                // update : Msg -> Model -> (Model, Cmd Msg)
                let update_ty = fun(var(1), fun(var(0), tup));
                // view : Model -> Element Msg
                let view_ty = fun(var(0), elem(var(1)));
                // subscriptions : Model -> Sub Msg
                let subs_ty = fun(var(0), sub(var(1)));
                // onKey : String -> String -> Msg
                let on_key_ty = fun(string.clone(), fun(string, var(1)));
                let cfg_rec = Ty::Record({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(self.builtins.live_f_init, init_ty);
                    m.insert(self.builtins.live_f_update, update_ty);
                    m.insert(self.builtins.live_f_view, view_ty);
                    m.insert(self.builtins.live_f_subscriptions, subs_ty);
                    m.insert(self.builtins.tui_f_on_key, on_key_ty);
                    m
                });
                fun(cfg_rec, task_unit)
            }

            // `Tui.program` — same as `Tui.app` but view returns `String`.
            // `view : Model -> String` renders the raw ANSI frame directly;
            // the runtime paints it verbatim (tui_app in app.rs:316).
            (Some("Tui"), Some("program")) => {
                let cmd = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.cmd,
                    args: vec![msg],
                };
                let sub = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sub,
                    args: vec![msg],
                };
                // `()` in Sky is `Ty::Unit`, not an empty tuple (see the
                // `Tui.app` arm above for the full rationale).
                let unit_ty = Ty::Unit;
                let tup = tuple2(var(0), cmd(var(1)));
                let init_ty = fun(unit_ty, tup.clone());
                let update_ty = fun(var(1), fun(var(0), tup));
                // view : Model -> String
                let view_ty = fun(var(0), string.clone());
                let subs_ty = fun(var(0), sub(var(1)));
                let on_key_ty = fun(string.clone(), fun(string, var(1)));
                let cfg_rec = Ty::Record({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(self.builtins.live_f_init, init_ty);
                    m.insert(self.builtins.live_f_update, update_ty);
                    m.insert(self.builtins.live_f_view, view_ty);
                    m.insert(self.builtins.live_f_subscriptions, subs_ty);
                    m.insert(self.builtins.tui_f_on_key, on_key_ty);
                    m
                });
                fun(cfg_rec, task_unit)
            }

            // ── Std.Webview / Sky.Webview app-entry kernel (Phase-1d) ────────────
            //
            // `Webview.app` has a 5-field closed cfg-record scheme.  The qualifier
            // set here MUST equal the lower resolved set
            // (lower.rs: `("Webview","app")→WebviewApp`)
            // — any mismatch reopens the exit-0-then-cargo-fail class.
            //
            // var(0) = Model
            // var(1) = Msg
            //
            // init         : () -> (Model, Cmd Msg)
            //   Note: Webview init takes `()` (unit), same as Tui — NOT `LiveReq`.
            // update       : Msg -> Model -> (Model, Cmd Msg)
            // view         : Model -> Html Msg
            //   Uses `html_con` (view must return `Html Msg` via `Ui.layout`),
            //   same as Live.app — the Webview runtime drives the same HTML renderer.
            // subscriptions: Model -> Sub Msg
            // window       : { title : String, size : (Int, Int) }
            //   Closed nested record — mirrors `WebviewWindowCfg { title, size }`.
            //   The `size` field is `Ty::Tuple([Int, Int])` (width × height).
            //
            // SOUNDNESS NOTE: `init` uses `Ty::Unit` (not empty `Ty::Tuple`).
            // An empty tuple `Ty::Tuple([])` prints as `()` but does NOT unify
            // with `Ty::Unit` — that would surface as SKY-T0001.
            (Some("Webview"), Some("app")) => {
                let cmd = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.cmd,
                    args: vec![msg],
                };
                let sub = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.sub,
                    args: vec![msg],
                };
                let html = |msg: Ty| Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.html_con,
                    args: vec![msg],
                };
                // `()` in Sky is `Ty::Unit`, not an empty tuple (see the
                // `Tui.app` arm above for the full rationale).
                let unit_ty = Ty::Unit;
                // (Model, Cmd Msg)
                let tup = tuple2(var(0), cmd(var(1)));
                // init : () -> (Model, Cmd Msg)
                let init_ty = fun(unit_ty, tup.clone());
                // update : Msg -> Model -> (Model, Cmd Msg)
                let update_ty = fun(var(1), fun(var(0), tup));
                // view : Model -> Html Msg  (Webview reuses the Live HTML renderer)
                let view_ty = fun(var(0), html(var(1)));
                // subscriptions : Model -> Sub Msg
                let subs_ty = fun(var(0), sub(var(1)));
                // window : { title : String, size : (Int, Int) }
                let window_ty = Ty::Record({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(self.builtins.webview_f_title, string);
                    m.insert(self.builtins.webview_f_size, tuple2(int.clone(), int));
                    m
                });
                let cfg_rec = Ty::Record({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(self.builtins.live_f_init, init_ty);
                    m.insert(self.builtins.live_f_update, update_ty);
                    m.insert(self.builtins.live_f_view, view_ty);
                    m.insert(self.builtins.live_f_subscriptions, subs_ty);
                    m.insert(self.builtins.webview_f_window, window_ty);
                    m
                });
                fun(cfg_rec, task_unit)
            }

            // Unknown kernel: a single flexible variable. The raw id is chosen
            // to be distinct from any real interned symbol's typical range; it
            // only needs to differ between the two `Ty::Var` arms of one
            // instantiate call, which a constant id trivially satisfies.
            _ => Ty::Var(u32::MAX),
        }
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
                        results.push(Ty::Var(root));
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
                    Content::Structure(FlatType::Record(fields)) => {
                        // Capture the field names (BTreeMap order) for the
                        // rebuild, and visit each field var in reverse so the
                        // results land in the same order the names are popped.
                        let names: Vec<Symbol> = fields.keys().copied().collect();
                        work.push(ZonkTask::BuildRecord { names });
                        for v in fields.values().copied().rev() {
                            work.push(ZonkTask::Visit(v));
                        }
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
                results.push(Ty::Record(fields));
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
// Phase C — kernel-registry migration tripwires
// ===========================================================================

#[cfg(test)]
impl<'a> Builder<'a> {
    /// Minimal [`Builder`] for exercising the pure scheme tables
    /// ([`Self::stdlib_scheme`] / [`Self::kernel_ty`]) in tests. Only `uf`,
    /// `interner`, and `builtins` are load-bearing for those two methods; every
    /// other field is empty. Pre-intern any needed strings BEFORE taking the
    /// immutable borrow into `interner`.
    fn for_scheme_test(
        uf: &'a mut UnionFind<Content>,
        interner: &'a Interner,
        builtins: Builtins,
    ) -> Self {
        Self {
            uf,
            interner,
            builtins,
            regions: BTreeMap::new(),
            constraints: Vec::new(),
            top_level: BTreeMap::new(),
            untyped: BTreeMap::new(),
            field_accesses: Vec::new(),
            record_updates: Vec::new(),
            ctors: BTreeMap::new(),
            typed_rigids: Vec::new(),
            scheme_apps: Vec::new(),
            super_vars: Vec::new(),
        }
    }
}

#[cfg(test)]
mod registry_phase_c_tests {
    use super::{Builder, Builtins, Content, Diagnostic, Feature, LowerError, Ty, UnionFind};
    use sky_diagnostics::Span;
    use sky_intern::Interner;
    use sky_kernels::StdlibKernel;

    /// Kernels RELOCATED into `stdlib_scheme` from the legacy `kernel_ty` table
    /// (Phase C's String/List/Math + Phase D's remaining backed families).
    /// Each carries a byte-faithful legacy oracle, so `stdlib_scheme_matches_legacy`
    /// proves the relocation changed no type. Monotone burndown anchor: GROWS per
    /// family task, never shrinks, and must exactly match the RELOCATED slice of
    /// what `stdlib_scheme` returns `Some` for.
    ///
    /// `Math.min` / `Math.max` are RELOCATED here as their *base* scheme
    /// (`a -> a -> a`); the `Comparable` obligation is layered separately in
    /// `constrain_var_kernel` (M4c gate), so their base is parity-checked like any
    /// other relocation while the bound still fires in production.
    const RELOCATED: &[StdlibKernel] = {
        use StdlibKernel as K;
        &[
            // Log (1)
            K::LogPrintln,
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
            // Math including min/max base (37)
            K::MathPi,
            K::MathE,
            K::MathPhi,
            K::MathSqrt2,
            K::MathInf,
            K::MathNan,
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
            // Task (11)
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
            // Io (3)
            K::IoReadLine,
            K::IoWriteStdout,
            K::IoWriteStderr,
            // Time (4)
            K::TimeNow,
            K::TimeUnixMillis,
            K::TimeSleep,
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
            // Random (3)
            K::RandomInt,
            K::RandomFloat,
            K::RandomChoice,
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
            // Http (9)
            K::HttpGet,
            K::HttpPost,
            K::HttpRequest,
            K::HttpParseQuery,
            K::HttpDefaultRequest,
            K::HttpWithMethod,
            K::HttpWithTimeout,
            K::HttpWithBody,
            K::HttpWithHeader,
            // Cmd (3)
            K::CmdNone,
            K::CmdBatch,
            K::CmdPerform,
            // Sub (3)
            K::SubNone,
            K::SubBatch,
            K::SubEvery,
            // Middleware (4)
            K::MiddlewareWithCors,
            K::MiddlewareWithLogging,
            K::MiddlewareWithBasicAuth,
            K::MiddlewareWithRateLimit,
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
            // Db (23)
            K::DbConnect,
            K::DbOpen,
            K::DbClose,
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
            K::DbUnsafeFindWhere,
            K::DbInsertFields,
            K::DbUpdateFields,
            K::DbInsertFieldsReturning,
            K::DbWithTransaction,
            K::DbMigrate,
            // Db.Decode (14)
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
            // Std.Ui layout / element / event (17)
            K::UiLayout,
            K::UiLayoutWith,
            K::UiEl,
            K::UiRow,
            K::UiColumn,
            K::UiWrappedRow,
            K::UiGrid,
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
            // Std.Live app-entry (3)
            K::LiveApp,
            K::LiveRoute,
            K::LiveRenderStatic,
            // Std.Tui app-entry (2)
            K::TuiApp,
            K::TuiProgram,
            // Std.Webview app-entry (1)
            K::WebviewApp,
        ]
    };

    /// Families that had NO legacy scheme (`kernel_ty` → `Ty::Var(u32::MAX)`) and
    /// receive their FIRST correct scheme in Phase D (D8–D13). No parity oracle
    /// exists; correctness is pinned by `first_schemed_were_holes` (the scheme
    /// closes a genuine hole) plus the skyc→cargo build fixtures. GROWS per
    /// family task; never shrinks. Empty until the first-scheme family tasks land.
    ///
    /// Phase D8–D13 (this slice) schemes the independent holed families from
    /// their runtime + `.sky` signatures. Task #58 additionally schemed Crypto
    /// AEAD (`aesGcm*`/`chacha20*`) and Jwt ENCODE (`encodeHs256`/`encodeRs256`)
    /// after correcting their registry `decl().arity` 3→2 to match the Rust
    /// runtime (the AEAD nonce is internal; encode takes secret + claims-JSON),
    /// so the arrow-count == arity invariant now holds. Task #69 additionally
    /// schemed the Std.Ui `Length` builders (`px`/`fill`/`content`/`shrink`/
    /// `fillPortion`/`vh`/`vw`/`minimum`/`maximum`), the Std.Ui `Color` builders
    /// (`rgb`/`rgba`/`white`/`black`/`transparent`), and the `Sky.Core.Json.Encode`
    /// encoders (`string`/`int`/`float`/`bool`/`null`/`list`/`object`/`encode`):
    /// `Length` / `Color` lower to `IrType::UiPlain(_)` and the JSON `Value` type
    /// to `IrType::Json`, all pre-existing IR forms with a complete emit path.
    /// EXCLUDED (still on the `Ty::Var` fallback): `PubSub`
    /// (`publish`/`publishNoEcho`) — a KNOWN-UNBACKED exclusion
    /// (`KNOWN_UNBACKED`), no runtime backing and qualifier absent from canon
    /// `qual_vars`; Uuid (task #54) and Encoding (task #55).
    const FIRST_SCHEMED: &[StdlibKernel] = {
        use StdlibKernel as K;
        &[
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
            // Char (8)
            K::CharIsAlpha,
            K::CharIsDigit,
            K::CharIsLower,
            K::CharIsUpper,
            K::CharToLower,
            K::CharToUpper,
            K::CharToCode,
            K::CharFromCode,
            // Crypto (17 — AEAD included after the arity 3→2 correction, #58)
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
            // Jwt (4 — encode included after the arity 3→2 correction, #58)
            K::JwtDecodeHs256,
            K::JwtDecodeRs256,
            K::JwtEncodeHs256,
            K::JwtEncodeRs256,
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
            // Std.Ui Length builders (9) — result type `Length`
            K::UiPx,
            K::UiFill,
            K::UiContent,
            K::UiShrink,
            K::UiFillPortion,
            K::UiVh,
            K::UiVw,
            K::UiMinimum,
            K::UiMaximum,
            // Std.Ui Color builders (5) — result type `Color`
            K::UiRgb,
            K::UiRgba,
            K::UiWhite,
            K::UiBlack,
            K::UiTransparent,
            // Sky.Core.Json.Encode (8) — `Value` positions map to `IrType::Json`
            K::JsonEncString,
            K::JsonEncInt,
            K::JsonEncFloat,
            K::JsonEncBool,
            K::JsonEncNull,
            K::JsonEncList,
            K::JsonEncObject,
            K::JsonEncEncode,
        ]
    };

    /// KNOWN-UNBACKED kernels: present in `StdlibKernel::ALL` (so they carry a
    /// registry index) but deliberately NEVER schemed. `PubSub.publish` /
    /// `PubSub.publishNoEcho` have no Rust runtime fn AND their `"PubSub"`
    /// qualifier is absent from canon `qual_vars`, so no user program can name
    /// them — they are unreachable. Scheming them would forge a well-typed
    /// exit-0 path to an unbacked kernel, so they stay on the `Ty::Var(u32::MAX)`
    /// fallback. Named explicitly here so Phase E's totality flip accounts for
    /// them deliberately rather than tripping on an unexplained `None`.
    /// Enforced by `known_unbacked_never_schemed`.
    const KNOWN_UNBACKED: &[StdlibKernel] = {
        use StdlibKernel as K;
        &[K::PubSubPublish, K::PubSubPublishNoEcho]
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
        let builder = Builder::for_scheme_test(&mut uf, &interner, builtins);

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

    /// Condition 4 — per-migrated-kernel PARITY TRIPWIRE. For every kernel that
    /// `stdlib_scheme` returns `Some` for, that scheme must be STRUCTURALLY
    /// EQUAL to the legacy `kernel_ty(decl.qualifier, decl.name)` — the
    /// Go-parity proof that the relocation was byte-faithful. Also enforces
    /// condition 5: the delegation key is `decl(k).(qualifier, name)`, so a
    /// transposed decl would compare against the wrong legacy arm and fail.
    #[test]
    fn stdlib_scheme_matches_legacy() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        // Pre-intern every kernel's (qualifier, name) BEFORE the immutable
        // borrow into the Builder.
        let syms: Vec<(StdlibKernel, sky_intern::Symbol, sky_intern::Symbol)> = StdlibKernel::ALL
            .iter()
            .map(|&k| {
                let d = k.decl();
                (
                    k,
                    interner.intern(d.qualifier).expect("intern qualifier"),
                    interner.intern(d.name).expect("intern name"),
                )
            })
            .collect();
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_test(&mut uf, &interner, builtins);

        let mut relocated_count = 0usize;
        for &(k, qual, name) in &syms {
            if let Some(scheme) = builder.stdlib_scheme(k) {
                if RELOCATED.contains(&k) {
                    let legacy = builder.kernel_ty(qual, name);
                    assert_eq!(
                        scheme,
                        legacy,
                        "stdlib_scheme({k:?}) is NOT byte-faithful to \
                         kernel_ty({:?}, {:?}); the relocation changed the \
                         type (Go-parity break).",
                        k.decl().qualifier,
                        k.decl().name,
                    );
                    relocated_count += 1;
                } else {
                    // A `Some` scheme that is NOT a relocation must be a
                    // deliberate FIRST_SCHEMED entry (a closed hole), never an
                    // unclassified type sneaking past the oracle.
                    assert!(
                        FIRST_SCHEMED.contains(&k),
                        "stdlib_scheme({k:?}) is Some but k is in neither \
                         RELOCATED nor FIRST_SCHEMED — classify it.",
                    );
                }
            }
        }

        // Every relocation accounted for.
        assert_eq!(
            relocated_count,
            RELOCATED.len(),
            "stdlib_scheme returned Some for {relocated_count} relocated kernels \
             but RELOCATED lists {}; update RELOCATED (burndown must track the \
             real set).",
            RELOCATED.len(),
        );
    }

    /// Every `FIRST_SCHEMED` kernel had NO legacy scheme (`kernel_ty` →
    /// `Ty::Var(u32::MAX)`). Proves the new scheme closes a genuine exit-0 hole
    /// rather than silently diverging from an existing legacy type — the
    /// Recipe-F was-a-hole guarantee.
    #[test]
    fn first_schemed_were_holes() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let syms: Vec<(StdlibKernel, sky_intern::Symbol, sky_intern::Symbol)> = FIRST_SCHEMED
            .iter()
            .map(|&k| {
                let d = k.decl();
                (
                    k,
                    interner.intern(d.qualifier).expect("intern qualifier"),
                    interner.intern(d.name).expect("intern name"),
                )
            })
            .collect();
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_test(&mut uf, &interner, builtins);
        for (k, q, n) in syms {
            assert_eq!(
                builder.kernel_ty(q, n),
                Ty::Var(u32::MAX),
                "FIRST_SCHEMED {k:?} had a legacy scheme — it is a relocation; \
                 move it to RELOCATED so its parity is checked.",
            );
        }
    }

    /// Condition 4 — monotone burndown. `stdlib_scheme` returns `Some` for
    /// EXACTLY `RELOCATED ∪ FIRST_SCHEMED` and `None` for every other variant.
    /// Pins the migrated set so an accidental over- or under-migration is caught.
    #[test]
    fn migrated_set_burndown() {
        let mut interner = Interner::new();
        let builtins = make_builder(&mut interner);
        let mut uf = UnionFind::<Content>::new();
        let builder = Builder::for_scheme_test(&mut uf, &interner, builtins);

        for &k in StdlibKernel::ALL {
            let migrated = builder.stdlib_scheme(k).is_some();
            let expected = RELOCATED.contains(&k) || FIRST_SCHEMED.contains(&k);
            assert_eq!(
                migrated, expected,
                "stdlib_scheme({k:?}).is_some() = {migrated} but \
                 RELOCATED∪FIRST_SCHEMED membership = {expected}",
            );
        }
    }

    /// Condition 2 — the fail-closed path is REACHABLE. When neither the
    /// registry nor the legacy table types a kernel, `kernel_scheme_or_unsupported`
    /// raises the SKY-L0108-shaped `Err` (loud), NOT a silent `Ty::Var`. Also
    /// checks registry-first precedence and single-source resolution.
    ///
    /// In production the legacy table is TOTAL (Phase C preserves the
    /// `Ty::Var(u32::MAX)` fallback), so the `(None, None)` input cannot arise
    /// yet — Phase E flips `legacy_kernel_ty` to return `None` for un-typed
    /// kernels, at which point this exact `Err` fires in the constrain path.
    #[test]
    fn both_miss_is_fail_closed() {
        let span = Span::DUMMY;
        let a = Ty::Var(0);
        let b = Ty::Var(1);

        // BOTH miss → fail-closed SKY-L0108.
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
            "expected SKY-L0108 (Feature::Kernels), got {err:?}",
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
}
