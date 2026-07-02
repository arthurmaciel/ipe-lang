//! The canonicalisation environment: the name → resolution tables consulted
//! during name resolution. Port of the M0 subset of
//! `Sky.Canonicalise.Environment`.
//!
//! Iteration order is never observable (lookups only), but the tables are
//! `BTreeMap`s so the structure is deterministic regardless of insertion order.

use std::collections::BTreeMap;

use sky_diagnostics::DResult;
use sky_intern::{Interner, Symbol};
use sky_kernels::StdlibKernel;

/// Where a (possibly qualified) variable resolves to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VarHome {
    /// A locally-bound name.
    Local,
    /// A top-level binding of the named module.
    TopLevel(Vec<Symbol>),
    /// A stdlib kernel function: kernel module, function name.
    Kernel(Symbol, Symbol),
}

/// Where a constructor resolves to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CtorHome {
    pub home: Vec<Symbol>,
    pub type_name: Symbol,
    pub name: Symbol,
    pub index: usize,
    pub arity: usize,
}

/// The name-resolution environment.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// The module being canonicalised.
    pub home: Vec<Symbol>,
    /// Unqualified variable bindings.
    pub vars: BTreeMap<Symbol, VarHome>,
    /// Unqualified constructor bindings.
    pub ctors: BTreeMap<Symbol, CtorHome>,
    /// Qualified variable bindings: qualifier → (name → home).
    pub qual_vars: BTreeMap<Symbol, BTreeMap<Symbol, VarHome>>,
    /// **Phase-A parse-once registry index.**  Maps `(qualifier_sym, name_sym)`
    /// to the typed [`StdlibKernel`] variant, built anti-drift from
    /// [`StdlibKernel::ALL`] in `install_prelude_qualifiers`.
    ///
    /// Phase B will thread this through `VarHome::Kernel`; Phase A exposes it
    /// here so the `canon_equals_registry` tripwire test can validate parity
    /// with `qual_vars` without touching any downstream path.
    pub stdlib_index: BTreeMap<(Symbol, Symbol), StdlibKernel>,
}

impl Env {
    /// Build the base environment with Sky's built-in variables and the
    /// auto-qualified prelude kernel modules. The `home` module's top-level
    /// names and unions are registered separately by the caller.
    ///
    /// # Errors
    /// [`sky_diagnostics::Diagnostic::CompilerBug`] if the interner's symbol
    /// table is exhausted while interning the built-in names.
    pub fn initial(home: Vec<Symbol>, interner: &mut Interner) -> DResult<Self> {
        let mut env = Self {
            home,
            ..Self::default()
        };
        env.install_builtin_vars(interner)?;
        env.install_builtin_ctors(interner)?;
        env.install_prelude_qualifiers(interner)?;
        Ok(env)
    }

    /// Register the Prelude-exposed built-in constructors so `Just` / `Nothing` /
    /// `Ok` / `Err` / `True` / `False` resolve as constructors — both as value
    /// expressions and in `case` patterns — without an explicit import. These
    /// belong to the built-in `Maybe a` / `Result e a` / `Bool` types, which have
    /// no user `type` declaration; `home` is left empty (matching how the builtin
    /// type names carry no user module) and `type_name` is the built-in type's
    /// symbol so downstream stages recognise it by name.
    ///
    /// # Errors
    /// [`sky_diagnostics::Diagnostic::CompilerBug`] if the interner is exhausted.
    fn install_builtin_ctors(&mut self, interner: &mut Interner) -> DResult<()> {
        let maybe = interner.intern("Maybe")?;
        let result = interner.intern("Result")?;
        let bool_ = interner.intern("Bool")?;
        // Db ADTs (M5b-db).
        let sqlvalue = interner.intern("SqlValue")?;
        let sqlfield = interner.intern("SqlField")?;
        // (constructor name, owning built-in type, index within the type, arity).
        for (name, type_name, index, arity) in [
            ("True", bool_, 0, 0),
            ("False", bool_, 1, 0),
            ("Just", maybe, 0, 1),
            ("Nothing", maybe, 1, 0),
            ("Ok", result, 0, 1),
            ("Err", result, 1, 1),
            // ── SqlValue variants (M5b-db) ────────────────────────────────────
            // Index order matches the `StdDbSqlValue` enum emitted by the backend
            // and the `into_sql_param()` dispatch in the runtime; DO NOT reorder.
            ("SqlString", sqlvalue, 0, 1),
            ("SqlInt", sqlvalue, 1, 1),
            ("SqlFloat", sqlvalue, 2, 1),
            ("SqlBool", sqlvalue, 3, 1),
            ("SqlBytes", sqlvalue, 4, 1),
            ("SqlTime", sqlvalue, 5, 1),    // millis: i64
            ("SqlDecimal", sqlvalue, 6, 1), // lossless TEXT decimal representation
            ("SqlMoney", sqlvalue, 7, 1),   // "ISO_CODE AMOUNT" format (TEXT)
            ("SqlNull", sqlvalue, 8, 1),    // type-witness inner value → Null
            // ── SqlField variants (M5b-db) ────────────────────────────────────
            ("SetField", sqlfield, 0, 1), // SetField : SqlValue -> SqlField
            ("OmitField", sqlfield, 1, 0), // OmitField : SqlField (nullary)
        ] {
            let name = interner.intern(name)?;
            self.ctors.insert(
                name,
                CtorHome {
                    home: Vec::new(),
                    type_name,
                    name,
                    index,
                    arity,
                },
            );
        }
        Ok(())
    }

    /// Bind a name as a local (function parameter / `case` binding).
    pub fn add_local(&mut self, name: Symbol) {
        self.vars.insert(name, VarHome::Local);
    }

    /// Look up an unqualified variable.
    #[must_use]
    pub fn lookup_var(&self, name: Symbol) -> Option<&VarHome> {
        self.vars.get(&name)
    }

    /// Look up an unqualified constructor.
    #[must_use]
    pub fn lookup_ctor(&self, name: Symbol) -> Option<&CtorHome> {
        self.ctors.get(&name)
    }

    /// Look up a qualified variable (`Qualifier.name`).
    #[must_use]
    pub fn lookup_qual_var(&self, qualifier: Symbol, name: Symbol) -> Option<&VarHome> {
        self.qual_vars.get(&qualifier).and_then(|m| m.get(&name))
    }

    /// The member table for a qualifier, or `None` when the qualifier names no
    /// known module/import alias. Lets a caller distinguish an unknown
    /// qualifier from a known qualifier missing the member.
    #[must_use]
    pub fn qual_members(&self, qualifier: Symbol) -> Option<&BTreeMap<Symbol, VarHome>> {
        self.qual_vars.get(&qualifier)
    }

    /// Built-in unqualified variables (from the Prelude). M0 subset of
    /// `Environment.builtinVars`.
    fn install_builtin_vars(&mut self, interner: &mut Interner) -> DResult<()> {
        let basics = interner.intern("Basics")?;
        let log = interner.intern("Log")?;
        for (name, module, func) in [
            ("identity", basics, "identity"),
            ("always", basics, "always"),
            ("not", basics, "not"),
            ("toString", basics, "toString"),
            ("modBy", basics, "modBy"),
            ("clamp", basics, "clamp"),
            ("fst", basics, "fst"),
            ("snd", basics, "snd"),
            ("errorToString", basics, "errorToString"),
            ("println", log, "println"),
        ] {
            let key = interner.intern(name)?;
            let func = interner.intern(func)?;
            self.vars.insert(key, VarHome::Kernel(module, func));
        }
        Ok(())
    }

    /// Auto-qualified prelude kernel modules. M0 subset of
    /// `Environment.preludeQualifiers` — `String.fromInt`, `String.fromFloat`,
    /// etc. resolve without an explicit `import String`.
    #[allow(clippy::too_many_lines)] // declarative table — extracting a helper would obscure the data
    fn install_prelude_qualifiers(&mut self, interner: &mut Interner) -> DResult<()> {
        const QUALIFIERS: &[(&str, &[&str])] = &[
            (
                "String",
                &[
                    // ── Arity-1 kernels ───────────────────────────────────
                    "length",
                    "reverse",
                    "isEmpty",
                    "toUpper",
                    "toLower",
                    "casefold",
                    "trim",
                    "trimStart",
                    "trimEnd",
                    "toInt",
                    "fromInt",
                    "toFloat",
                    "fromFloat",
                    "fromChar",
                    "fromList",
                    "concat",
                    "words",
                    "lines",
                    "toList",
                    "isEmail",
                    "isUrl",
                    // ── Arity-2 kernels ───────────────────────────────────
                    "append",
                    "split",
                    "join",
                    "contains",
                    "startsWith",
                    "endsWith",
                    "equalFold",
                    "repeat",
                    "dropLeft",
                    "dropRight",
                    // ── Arity-3 kernels ───────────────────────────────────
                    "replace",
                    "slice",
                    "padLeft",
                    "padRight",
                    // ── Haystack-first pure-Sky aliases (compile from source) ──
                    "containsIn",
                    "startsWithIn",
                    "endsWithIn",
                    // ── Legacy entry kept for compatibility ───────────────
                    "toChar",
                ],
            ),
            (
                "Char",
                &[
                    "isAlpha", "isDigit", "isLower", "isUpper", "toLower", "toUpper", "toCode",
                    "fromCode",
                ],
            ),
            (
                "List",
                &[
                    "map",
                    "filter",
                    "foldl",
                    "foldr",
                    "length",
                    "head",
                    "tail",
                    "take",
                    "drop",
                    "append",
                    "concat",
                    "concatMap",
                    "reverse",
                    "member",
                    "any",
                    "all",
                    "range",
                    "zip",
                    "isEmpty",
                    "cons",
                ],
            ),
            ("Maybe", &["withDefault", "map", "andThen"]),
            ("Result", &["withDefault", "map", "andThen", "mapError"]),
            // `Sky.Core.Math` — `min` / `max` are polymorphic `a -> a -> a`
            // (Elm `Basics.min`/`max` semantics). Wired in the lowerer to the
            // runtime's generic compare. All other Math kernels have concrete
            // monomorphic types (abs : Int->Int, sqrt : Float->Float, etc.).
            (
                "Math",
                &[
                    "min",
                    "max",
                    // constants
                    "pi",
                    "e",
                    "phi",
                    "sqrt2",
                    "inf",
                    "nan",
                    // arity-1 Int→Int
                    "abs",
                    // arity-1 Float→Float
                    "sqrt",
                    "cbrt",
                    "exp",
                    "exp2",
                    "log",
                    "log2",
                    "log10",
                    "sin",
                    "cos",
                    "tan",
                    "asin",
                    "acos",
                    "atan",
                    "sinh",
                    "cosh",
                    "tanh",
                    "asinh",
                    "acosh",
                    "atanh",
                    // arity-1 Float→Int
                    "floor",
                    "ceil",
                    "round",
                    "trunc",
                    // arity-2 Float→Float→Float
                    "pow",
                    "hypot",
                    "atan2",
                    "mod",
                    "remainder",
                ],
            ),
            (
                "Basics",
                &[
                    "identity", "always", "not", "toString", "modBy", "clamp", "fst", "snd",
                    "compare", "negate", "abs", "sqrt", "min", "max",
                ],
            ),
            // `Sky.Core.Dict` — associative map kernels (M4d).
            (
                "Dict",
                &[
                    "empty", "isEmpty", "size", "insert", "get", "remove", "member", "keys",
                    "values", "toList", "fromList", "map", "foldl", "union",
                ],
            ),
            // `Sky.Core.Set` — set kernels (M4d).
            (
                "Set",
                &[
                    "empty",
                    "size",
                    "insert",
                    "remove",
                    "member",
                    "toList",
                    "fromList",
                    "union",
                    "intersect",
                    "diff",
                ],
            ),
            // `Sky.Core.Bytes` — byte-buffer kernels (M4e).
            (
                "Bytes",
                &[
                    "empty",
                    "length",
                    "isEmpty",
                    "fromString",
                    "toString",
                    "fromHex",
                    "toHex",
                    "fromBase64",
                    "toBase64",
                    "append",
                    "slice",
                ],
            ),
            // `Sky.Core.Encoding` — text encoding helpers (M4f).
            (
                "Encoding",
                &[
                    "base64Encode",
                    "base64Decode",
                    "urlEncode",
                    "urlDecode",
                    "hexEncode",
                    "hexDecode",
                ],
            ),
            // `Sky.Core.Json.Encode` — JSON encoder (M4g).
            (
                "JsonEnc",
                &[
                    "string", "int", "float", "bool", "null", "list", "object", "encode",
                ],
            ),
            // `Sky.Core.Json.Decode` — JSON decoder combinators (M4h).
            (
                "JsonDec",
                &[
                    "string",
                    "int",
                    "float",
                    "bool",
                    "decodeString",
                    "field",
                    "at",
                    "index",
                    "list",
                    "map",
                    "andThen",
                    "succeed",
                    "fail",
                    "oneOf",
                    "map2",
                    "map3",
                    "map4",
                ],
            ),
            // `Sky.Core.Json.Decode.Pipeline` — pipeline-style record decoders (M4h).
            (
                "JsonDecP",
                &["required", "optional", "custom", "requiredAt"],
            ),
            // `Sky.Core.Crypto` — hashes / HMAC / RSA / AEAD / key-derivation / random (M5a).
            (
                "Crypto",
                &[
                    "sha256",
                    "sha512",
                    "sha1",
                    "md5",
                    "hmacSha256",
                    "hmacSha512",
                    "rsaSha256Sign",
                    "rsaSha256Verify",
                    "constantTimeEqual",
                    "aesGcmEncrypt",
                    "aesGcmDecrypt",
                    "chacha20Encrypt",
                    "chacha20Decrypt",
                    "aesKeyFromPassword",
                    "chachaKeyFromPassword",
                    "randomBytes",
                    "randomToken",
                ],
            ),
            // `Sky.Core.Uuid` — UUID generation and parsing (M5b).
            // `v4` and `v7` are arity-0 (bare value); `parse` is arity-1.
            ("Uuid", &["v4", "v7", "parse"]),
            // `Sky.Core.Jwt` — JWT encode/decode for HS256 and RS256 (M5b).
            // All functions take (key, payload) — arity 2.
            (
                "Jwt",
                &["encodeHs256", "decodeHs256", "encodeRs256", "decodeRs256"],
            ),
            // `Sky.Core.Task` — Task combinators (M5a).
            (
                "Task",
                &[
                    "succeed",
                    "fail",
                    "map",
                    "andThen",
                    "mapError",
                    "onError",
                    "fromResult",
                    "andThenResult",
                    "sequence",
                    "parallel",
                    "run",
                ],
            ),
            // `Sky.Core.Io` — I/O effects (M5a).
            ("Io", &["readLine", "writeStdout", "writeStderr"]),
            // `Sky.Core.Time` — time effects (M5a) + M5c TEA tick subscription.
            ("Time", &["now", "sleep", "unixMillis", "every"]),
            // `Sky.Core.System` — system effects (M5a).
            (
                "System",
                &[
                    "args",
                    "getenv",
                    "getenvOr",
                    "getArg",
                    "getenvInt",
                    "getenvBool",
                    "setenv",
                    "unsetenv",
                    "cwd",
                    "loadEnv",
                    "exit",
                ],
            ),
            // `Sky.Core.Random` — random effects (M5a).
            ("Random", &["int", "float", "choice"]),
            // `Sky.Core.File` — file effects (M5a).
            (
                "File",
                &[
                    "readFile",
                    "writeFile",
                    "exists",
                    "remove",
                    "mkdirAll",
                    "readFileLimit",
                    "readFileBytes",
                    "append",
                    "readDir",
                    "isDir",
                    "tempFile",
                    "tempDir",
                    "copy",
                    "rename",
                    "delete",
                ],
            ),
            // `Sky.Core.Http` — outbound HTTP client (M5b).
            // `get` / `post` / `request` are effect kernels (Task Error
            // HttpResponse); `parseQuery` is a pure kernel (String -> Dict
            // String String); the `with*` builders + `defaultRequest` are ALSO
            // pure kernels (HttpRequest record-update emission in the backend) —
            // cross-module pure-Sky stdlib calls are not resolved by skyc, so the
            // builders cannot live as pure Sky in Http.sky. Every name below is
            // registered so `Http.foo` resolves during name-resolution and lands
            // as `Callee::Kernel` (see lower.rs ("Http", _) arms + constrain.rs
            // kernel_ty Http entries that give each its record type).
            (
                "Http",
                &[
                    "get",
                    "post",
                    "request",
                    "defaultRequest",
                    "withMethod",
                    "withHeader",
                    "withTimeout",
                    "withBody",
                    "parseQuery",
                ],
            ),
            // ── M5c: TEA Cmd / Sub kernels ──────────────────────────────────────
            // Construct-only in M5c; the TEA dispatch loop lands in M6.
            // `Cmd.publish*` / `Sub.subscribeTopic` / `PubSub.*` are NOT listed
            // here — they will get their own qualifier entries in M6.
            ("Cmd", &["none", "batch", "perform"]),
            ("Sub", &["none", "batch", "every"]),
            // ── Db kernels (M5b-db) ─────────────────────────────────────────────
            // `Std.Db` — database connection + query surface.
            // All effect-returning kernels (Task Error …) and pure helpers
            // (`getString`, `getInt`, `getBool`, `getField`) are registered here.
            // `SqlValue` / `SqlField` ADT constructors are handled by
            // `install_builtin_ctors` above; they are unqualified.
            (
                "Db",
                &[
                    "connect",
                    "open",
                    "close",
                    "execRaw",
                    "exec",
                    "query",
                    "queryDecode",
                    "getString",
                    "getInt",
                    "getBool",
                    "getField",
                    "insertRow",
                    "getById",
                    "updateById",
                    "deleteById",
                    "findOneByField",
                    "findManyByField",
                    "findByConditions",
                    "unsafeFindWhere",
                    "insertFields",
                    "updateFields",
                    "insertFieldsReturning",
                    "withTransaction",
                    "migrate",
                ],
            ),
            // `Std.Db.Decode` — row decoder combinators (M5b-db).
            // The qualifier string contains a dot ("Db.Decode") which the parser
            // produces correctly for the 3-segment path `Db.Decode.string` — see
            // sky_parse::parser::ident_expr (qualifier = init.join(".")).
            (
                "Db.Decode",
                &[
                    "string", "int", "float", "bool", "nullable", "map", "andThen", "succeed",
                    "fail", "map2", "map3", "map4", "required", "optional",
                ],
            ),
            // M6: Sky.Http.Server kernels.
            (
                "Server",
                &[
                    "get",
                    "post",
                    "put",
                    "delete",
                    "any",
                    "api",
                    "static",
                    "listen",
                    "text",
                    "json",
                    "html",
                    "withStatus",
                    "withHeader",
                    "redirect",
                    "param",
                    "queryParam",
                    "header",
                    "getCookie",
                    "body",
                    "path",
                    "method",
                    "cookie",
                    "withCookie",
                ],
            ),
            // M6: Sky.Http.Middleware kernels.
            (
                "Middleware",
                &["withCors", "withLogging", "withBasicAuth", "withRateLimit"],
            ),
            // M6: Sky.Http.RateLimit kernels.
            ("RateLimit", &["allow"]),
            // ── M7: Std.Ui — element / attribute / color / layout builders ──────
            // `layout` and `layoutWith` are render kernels; the rest are element /
            // attribute / length / color value builders wired as kernel helpers.
            // All names below resolve as `VarHome::Kernel("Ui", name)` so that
            // qualified references like `Ui.column [...]` succeed in the canon phase.
            (
                "Ui",
                &[
                    // ── render kernels ────────────────────────────────────────
                    "layout",
                    "layoutWith",
                    // ── element builders ─────────────────────────────────────
                    "none",
                    "text",
                    "el",
                    "row",
                    "column",
                    "wrappedRow",
                    "grid",
                    "html",
                    // ── attribute builders ───────────────────────────────────
                    "spacing",
                    "padding",
                    "paddingXY",
                    "paddingEach",
                    "width",
                    "height",
                    "centerX",
                    "centerY",
                    "alignLeft",
                    "alignRight",
                    "alignTop",
                    "alignBottom",
                    "pointer",
                    "clip",
                    "clipX",
                    "clipY",
                    "scrollbars",
                    "scrollbarX",
                    "scrollbarY",
                    "gridColumns",
                    "above",
                    "below",
                    "onLeft",
                    "onRight",
                    "inFront",
                    "behind",
                    "onClick",
                    "onSubmit",
                    "onInput",
                    "onChange",
                    "onFocus",
                    "onBlur",
                    "onMouseOver",
                    "onMouseOut",
                    "onKeyDown",
                    "onKeyUp",
                    "onBool",
                    "onFile",
                    "htmlAttribute",
                    "mediaQuery",
                    "breakpoint",
                    "aspectRatio",
                    "aspectRatioWH",
                    "square",
                    "widescreen",
                    "onPseudo",
                    "hover",
                    "focus",
                    "focusVisible",
                    "active",
                    "disabled",
                    "mobile",
                    "tablet",
                    "desktop",
                    "darkMode",
                    "lightMode",
                    "reducedMotion",
                    // ── Length builders ─────────────────────────────────────
                    "px",
                    "fill",
                    "fillPortion",
                    "content",
                    "shrink",
                    "minimum",
                    "maximum",
                    "vh",
                    "vw",
                    // ── Color builders ──────────────────────────────────────
                    "rgb",
                    "rgba",
                    "white",
                    "black",
                    "transparent",
                    // ── Other ────────────────────────────────────────────────
                    "paragraph",
                    "textColumn",
                    "image",
                    "link",
                    "button",
                    "input",
                    "form",
                ],
            ),
            // ── M7: Std.Ui.Background sub-module ─────────────────────────────────
            (
                "Background",
                &[
                    "color",
                    "image",
                    "hoverColor",
                    "focusColor",
                    "activeColor",
                    "disabledColor",
                    "linearGradient",
                ],
            ),
            // ── M7: Std.Ui.Border sub-module ─────────────────────────────────────
            (
                "Border",
                &[
                    "width",
                    "widthEach",
                    "color",
                    "rounded",
                    "solid",
                    "dashed",
                    "dotted",
                    "shadow",
                    "glow",
                    "innerShadow",
                    "hoverColor",
                    "focusColor",
                    "activeColor",
                    "hoverWidth",
                    "hoverRounded",
                ],
            ),
            // ── M7: Std.Ui.Font sub-module ───────────────────────────────────────
            (
                "Font",
                &[
                    "color",
                    "family",
                    "size",
                    "weight",
                    "bold",
                    "semiBold",
                    "regular",
                    "light",
                    "extraBold",
                    "black",
                    "italic",
                    "underline",
                    "noDecoration",
                    "letterSpacing",
                    "wordSpacing",
                    "alignLeft",
                    "alignRight",
                    "center",
                    "justify",
                    "sansSerif",
                    "serif",
                    "monospace",
                    "hoverColor",
                    "focusColor",
                    "activeColor",
                    "disabledColor",
                    "hoverSize",
                ],
            ),
            // ── M7: Std.Html — typed HTML element / text surface ─────────────────
            // `render` / `escapeHtml` / `escapeAttr` / `attrToString` are render
            // kernels; all element-builder names create `Html msg` values.
            (
                "Html",
                &[
                    // render kernels
                    "render",
                    "toString",
                    "escapeHtml",
                    "escapeAttr",
                    "attrToString",
                    // text / raw nodes
                    "text",
                    "raw",
                    // generic builder
                    "node",
                    "voidNode",
                    "doctype",
                    "styleNode",
                    "titleNode",
                    // common containers
                    "div",
                    "span",
                    "p",
                    "a",
                    "button",
                    "form",
                    "label",
                    "nav",
                    "section",
                    "article",
                    "header",
                    "footer",
                    "main",
                    "aside",
                    "ul",
                    "ol",
                    "li",
                    "table",
                    "thead",
                    "tbody",
                    "tfoot",
                    "tr",
                    "th",
                    "td",
                    "textarea",
                    "select",
                    "option",
                    "pre",
                    "code",
                    "strong",
                    "em",
                    "small",
                    "fieldset",
                    "legend",
                    "blockquote",
                    "figure",
                    "figcaption",
                    "details",
                    "summary",
                    "dialog",
                    "video",
                    "audio",
                    "canvas",
                    "iframe",
                    "progress",
                    "meter",
                    "script",
                    // headings
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    // void elements
                    "img",
                    "input",
                    "br",
                    "hr",
                    "meta",
                    "link",
                    "area",
                    "base",
                    "col",
                    "embed",
                    "source",
                    "track",
                    "wbr",
                    // document elements
                    "body",
                    "htmlNode",
                    "headNode",
                    "title",
                    // legacy compat aliases
                    "headerNode",
                    "codeNode",
                    "mainNode",
                    "footerNode",
                    "linkNode",
                ],
            ),
            // ── M7: Std.Html.Attributes alias ────────────────────────────────────
            (
                "Attr",
                &[
                    "attribute",
                    "boolAttribute",
                    "style",
                    "class",
                    "id",
                    "type_",
                    "name",
                    "value",
                    "placeholder",
                    "href",
                    "src",
                    "alt",
                    "for_",
                    "checked",
                    "disabled",
                    "readonly",
                    "required",
                    "multiple",
                    "selected",
                    "autofocus",
                    "tabindex",
                    "noAttr",
                ],
            ),
            // ── M7: Std.Html.Events alias ─────────────────────────────────────────
            (
                "Event",
                &[
                    "onClick",
                    "onInput",
                    "onChange",
                    "onSubmit",
                    "onFocus",
                    "onBlur",
                    "onMouseOver",
                    "onMouseOut",
                    "onKeyDown",
                    "onKeyUp",
                    "onBool",
                    "onMsg",
                ],
            ),
            // ── M7: Sky.Live / Std.Live app-entry kernels ────────────────────────
            ("Live", &["app", "appRouted", "route", "renderStatic"]),
            // ── M7: Sky.Tui / Std.Tui app-entry kernels ──────────────────────────
            ("Tui", &["app", "program"]),
            // ── M7: Sky.Webview / Std.Webview app-entry kernel ───────────────────
            ("Webview", &["app"]),
        ];

        // ── M7: Per-qualifier function name aliases ───────────────────────────
        // Maps a Sky-source alias name (e.g. `htmlRender`) to its canonical
        // kernel function name (e.g. `render`) within a qualifier module, so
        // `Html.htmlRender` and `Std.Html.htmlRender` both produce
        // `VarKernel { module: html_sym, name: render_sym }` — which lower.rs
        // matches under the same `("Html", "render")` arm.
        //
        // Declared here (before the first `for` statement) to satisfy
        // `clippy::items_after_statements`.
        //
        // MUST be processed BEFORE QUALIFIER_ALIASES (installed below) so that
        // alias entries are included in any qual-to-qual copy.
        const FUNC_ALIASES: &[(&str, &str, &str)] = &[
            // ("qualifier", "alias_name", "canonical_kernel_name")
            ("Html", "htmlRender", "render"),
            ("Html", "htmlEscapeText", "escapeHtml"),
            ("Html", "htmlEscapeAttr", "escapeAttr"),
            ("Html", "htmlAttrToString", "attrToString"),
        ];

        // ── M7: Qualifier module aliases (Std.X / Sky.X → short canonical) ───
        // Clones every entry from the canonical qualifier's member map into the
        // alias qualifier key. Because each entry already holds
        // `VarHome::Kernel(canonical_sym, fn_sym)` (NOT the alias key's symbol),
        // `resolve_qual_var` in `resolve.rs` produces a `VarKernel` whose
        // `module` field is always the canonical short name ("Html", "Ui", …).
        // lower.rs match arms therefore work unmodified.
        //
        // Declared here (before the first `for` statement) to satisfy
        // `clippy::items_after_statements`.
        const QUALIFIER_ALIASES: &[(&str, &str)] = &[
            // (alias_qualifier, canonical_qualifier)
            ("Std.Html", "Html"),
            ("Std.Ui", "Ui"),
            ("Std.Html.Attributes", "Attr"),
            ("Std.Html.Events", "Event"),
            ("Std.Live", "Live"),
            ("Std.Tui", "Tui"),
            ("Std.Webview", "Webview"),
            // Sky.* forms for consistency with other kernel module conventions.
            ("Sky.Html", "Html"),
            ("Sky.Ui", "Ui"),
            ("Sky.Live", "Live"),
            ("Sky.Tui", "Tui"),
        ];

        for (qual, funcs) in QUALIFIERS {
            let qual_sym = interner.intern(qual)?;
            let mut module = BTreeMap::new();
            for func in *funcs {
                let func_sym = interner.intern(func)?;
                module.insert(func_sym, VarHome::Kernel(qual_sym, func_sym));
            }
            self.qual_vars.entry(qual_sym).or_default().extend(module);
        }

        for (qual, alias, canonical) in FUNC_ALIASES {
            let qual_sym = interner.intern(qual)?;
            let alias_sym = interner.intern(alias)?;
            let canonical_sym = interner.intern(canonical)?;
            // VarHome stores the CANONICAL module + fn symbols so lower.rs
            // match arms (`("Html", "render")`) work without any changes.
            let home = VarHome::Kernel(qual_sym, canonical_sym);
            self.qual_vars
                .entry(qual_sym)
                .or_default()
                .insert(alias_sym, home);
        }

        for (alias, canonical) in QUALIFIER_ALIASES {
            let alias_sym = interner.intern(alias)?;
            let canonical_sym = interner.intern(canonical)?;
            // `.cloned()` releases the shared borrow before the mutable
            // `entry(alias_sym)` borrow — required by the borrow checker.
            if let Some(canonical_members) = self.qual_vars.get(&canonical_sym).cloned() {
                self.qual_vars
                    .entry(alias_sym)
                    .or_default()
                    .extend(canonical_members);
            }
        }

        // ── Phase-A: parse-once registry index ───────────────────────────────
        // Derived from the SAME StdlibKernel::ALL + decl() source as the
        // StdlibKernel enum itself — anti-drift by construction.
        // Skip internal-only qualifiers (e.g. "_internal_") and the unqualified
        // Log/Cmd/Sub variants whose qualifiers are absent from qual_vars
        // (installed via install_builtin_vars or not yet wired); the tripwire
        // test checks only the registry→canon direction so those omissions are
        // safe for Phase A.
        for sk in StdlibKernel::ALL {
            let decl = sk.decl();
            if decl.qualifier.starts_with('_') {
                continue; // e.g. "_internal_" — skip
            }
            let qual_sym = interner.intern(decl.qualifier)?;
            let name_sym = interner.intern(decl.name)?;
            self.stdlib_index.insert((qual_sym, name_sym), *sk);
        }

        Ok(())
    }
}
