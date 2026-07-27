//! A readable, indented pretty-printer for the typed IR, backing the
//! `--emit-ir` developer flag. This is intentionally *not* the derived
//! `Debug` rendering: it resolves interned [`Symbol`]s back to their source
//! names and lays the program out as a shallow tree (modules → types / funcs →
//! params / expressions / match arms / kernels) that a human can scan.
//!
//! The function is pure and total: it never panics, never indexes a slice
//! directly, and resolves a forged or cross-interner symbol to an explicit
//! `<sym#N>` placeholder rather than crashing or silently emitting an empty
//! name. Output is deterministic — the same `(program, interner)` always
//! renders the same string.

use ipe_intern::{Interner, Symbol};

use crate::ir::{
    Arm, BinOp, BoundSet, Callee, EnumDef, Expr, Func, IrType, KernelFn, Match, ModPath, Module,
    Pat, Program, TypeDef, UiCtor, UiPlain, Variant,
};

/// Upper bound on the nesting depth `ir_type_name`/`pat_name`/`write_expr`
/// will recurse before rendering a `<depth limit>` placeholder.
///
/// Past this depth these functions stop recursing rather than overflowing
/// the native stack. The real Rust backend emitter refuses a program past
/// this same depth (`ipe_backend_rust::emit_expr`'s `MAX_EMIT_DEPTH`,
/// IPE-L0200) — this is the single source of truth both share, so
/// `--emit-ir` (a dev-flag path with no other gate) can never stack-overflow
/// on a program the emitter would already have refused, nor drift out of
/// step with the real bound.
pub const MAX_IR_RENDER_DEPTH: u16 = 96;

/// A `<depth limit>` placeholder, rendered instead of recursing further once
/// [`MAX_IR_RENDER_DEPTH`] is exceeded.
const DEPTH_LIMIT_PLACEHOLDER: &str = "<depth limit>";

/// Render `program` as a readable indented tree, resolving every [`Symbol`]
/// against `interner`.
///
/// Pure and total: no panics, no direct indexing, deterministic output.
#[must_use]
pub fn pretty(program: &Program, interner: &Interner) -> String {
    let mut out = String::new();
    out.push_str("program\n");
    for module in &program.modules {
        write_module(&mut out, module, interner);
    }
    out
}

/// Append `text` at the given indentation `level` (two spaces per level),
/// followed by a newline.
fn line(out: &mut String, level: usize, text: &str) {
    for _ in 0..level {
        out.push_str("  ");
    }
    out.push_str(text);
    out.push('\n');
}

/// Resolve a symbol to its interned name, or an explicit placeholder when the
/// symbol was never handed out by this interner.
fn sym_name(interner: &Interner, sym: Symbol) -> String {
    interner
        .resolve(sym)
        .map_or_else(|| format!("<sym#{}>", sym.as_raw()), str::to_owned)
}

/// Render a dotted module path, e.g. `Ipe.Io`.
fn mod_path_name(interner: &Interner, path: &ModPath) -> String {
    path.0
        .iter()
        .map(|seg| sym_name(interner, *seg))
        .collect::<Vec<_>>()
        .join(".")
}

/// Render an [`IrType`] as its source-facing name.
///
/// Depth-bounded via [`ir_type_name_at`] (starting at depth 0) — every
/// call site here stays the plain 2-argument form; only the recursive
/// descent inside `ir_type_name_at` threads the counter.
fn ir_type_name(interner: &Interner, ty: &IrType) -> String {
    ir_type_name_at(interner, ty, 0)
}

/// [`ir_type_name`]'s depth-tracked recursion. Past [`MAX_IR_RENDER_DEPTH`]
/// this renders [`DEPTH_LIMIT_PLACEHOLDER`] instead of recursing further —
/// total and stack-safe on a pathologically nested type, matching the real
/// Rust backend emitter's own bound rather than trusting the caller never to
/// hand it one.
#[allow(clippy::too_many_lines)]
fn ir_type_name_at(interner: &Interner, ty: &IrType, depth: u16) -> String {
    if depth > MAX_IR_RENDER_DEPTH {
        return DEPTH_LIMIT_PLACEHOLDER.to_owned();
    }
    let depth = depth + 1;
    match ty {
        IrType::Int => "Int".to_owned(),
        IrType::Float => "Float".to_owned(),
        IrType::Bool => "Bool".to_owned(),
        IrType::Str => "String".to_owned(),
        IrType::Char => "Char".to_owned(),
        IrType::Unit => "()".to_owned(),
        IrType::Task(inner) => format!("Task Error {}", ir_type_name_at(interner, inner, depth)),
        // A generic type variable renders by its source name (e.g. `a`); the
        // Rust generic spelling (`T1`, …) is a backend concern, so the IR view
        // keeps the source-facing name.
        IrType::Generic(name) => sym_name(interner, *name),
        // An enum renders by its type name, applied to its type arguments in
        // source-like prefix form (`Maybe Int`). A non-generic enum (empty
        // `args`) is just the bare type name.
        IrType::Enum { name, args, .. } => {
            let base = sym_name(interner, *name);
            if args.is_empty() {
                base
            } else {
                let rendered = args
                    .iter()
                    .map(|t| ir_type_name_at(interner, t, depth))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{base} {rendered}")
            }
        }
        // The built-in `Maybe a` / `Result e a` render in source-like prefix
        // form, exactly like a generic enum would.
        IrType::Maybe(elem) => format!("Maybe {}", ir_type_name_at(interner, elem, depth)),
        IrType::Result(err, ok) => format!(
            "Result {} {}",
            ir_type_name_at(interner, err, depth),
            ir_type_name_at(interner, ok, depth)
        ),
        IrType::List(elem) => format!("List {}", ir_type_name_at(interner, elem, depth)),
        IrType::Dict(k, v) => format!(
            "Dict {} {}",
            ir_type_name_at(interner, k, depth),
            ir_type_name_at(interner, v, depth)
        ),
        IrType::Set(a) => format!("Set {}", ir_type_name_at(interner, a, depth)),
        IrType::Bytes => "Bytes".to_owned(),
        IrType::Json => "Json".to_owned(),
        IrType::Decoder(inner) => format!("Decoder {}", ir_type_name_at(interner, inner, depth)),
        IrType::Db => "Db".to_owned(),
        IrType::Cmd(inner) => format!("Cmd {}", ir_type_name_at(interner, inner, depth)),
        IrType::Sub(inner) => format!("Sub {}", ir_type_name_at(interner, inner, depth)),
        // Opaque server types — source-facing names match Ipê stdlib.
        IrType::ServerRequest => "Request".to_owned(),
        IrType::ServerResponse => "Response".to_owned(),
        IrType::ServerRoute => "Route".to_owned(),
        IrType::ServerCookie => "Cookie".to_owned(),
        // stream writer handle.
        IrType::StreamWriter => "StreamWriter".to_owned(),
        // HTTP request handle (opaque, structural record folded to this variant).
        IrType::HttpRequest => "HttpRequest".to_owned(),
        // Ipe.Http.Server.WebSocket opaque handles.
        IrType::WebSocketServer => "WebSocketServer".to_owned(),
        IrType::WebSocketServerCfg => "WebSocketServerCfg".to_owned(),
        // Ipe.Ui / Ipe.Html parametric types.
        IrType::Ui { ctor, msg } => {
            let ctor_name = match ctor {
                UiCtor::Html => "Html",
                UiCtor::Element => "Element",
                UiCtor::UiAttribute => "Ui.Attribute",
                UiCtor::HtmlAttribute => "Html.Attribute",
                UiCtor::HtmlEvent => "Html.Event",
                // Ipe.Ui.Input parametric label / placeholder types.
                UiCtor::Label => "Input.Label",
                UiCtor::Placeholder => "Input.Placeholder",
                // Ipe.Ui.Input radio option type.
                UiCtor::RadioOption => "Input.RadioOption",
            };
            format!("{} {}", ctor_name, ir_type_name_at(interner, msg, depth))
        }
        IrType::UiPlain(plain) => match plain {
            UiPlain::Length => "Length".to_owned(),
            UiPlain::Color => "Color".to_owned(),
            UiPlain::HAlign => "HAlign".to_owned(),
            UiPlain::VAlign => "VAlign".to_owned(),
            UiPlain::Location => "Location".to_owned(),
            UiPlain::PseudoClass => "PseudoClass".to_owned(),
            UiPlain::Description => "Description".to_owned(),
            UiPlain::LayoutContext => "LayoutContext".to_owned(),
        },
        IrType::LiveReq => "LiveReq".to_owned(),
        IrType::LiveRoute(page) => format!("LiveRoute {}", ir_type_name_at(interner, page, depth)),
        IrType::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(|t| ir_type_name_at(interner, t, depth))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        IrType::Record(fields) => {
            // Render in field-name order (the BTreeMap is keyed by Symbol, so
            // sort the resolved names for a deterministic, source-like form).
            let mut entries: Vec<(String, String)> = fields
                .iter()
                .map(|(name, ty)| {
                    (
                        sym_name(interner, *name),
                        ir_type_name_at(interner, ty, depth),
                    )
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            if entries.is_empty() {
                "{}".to_owned()
            } else {
                let inner = entries
                    .iter()
                    .map(|(n, t)| format!("{n} : {t}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ {inner} }}")
            }
        }
        // All three function carriers share one source-like arrow form `T0 ->
        // T1 -> R` (a nullary shows its unit parameter explicitly). The
        // Box-vs-Arc-vs-FnOnce carrier distinction is a backend Rust-emission
        // concern, invisible at the IR pretty-printer's source-facing level.
        IrType::Fun(params, ret)
        | IrType::SharedFun(params, ret)
        | IrType::FnOnceChain(params, ret) => {
            let mut parts: Vec<String> = params
                .iter()
                .map(|t| ir_type_name_at(interner, t, depth))
                .collect();
            if parts.is_empty() {
                parts.push("()".to_owned());
            }
            parts.push(ir_type_name_at(interner, ret, depth));
            parts.join(" -> ")
        }
        IrType::Order => "Order".to_owned(),
        IrType::Decimal => "Decimal".to_owned(),
        IrType::ErrorKind => "ErrorKind".to_owned(),
        IrType::Error => "Error".to_owned(),
        IrType::ErrorDetails => "ErrorDetails".to_owned(),
        IrType::ErrorInfo => "ErrorInfo".to_owned(),
        IrType::PanicInfo => "PanicInfo".to_owned(),
        IrType::TypeInfo => "TypeInfo".to_owned(),
        IrType::SqlFragment => "SqlFragment".to_owned(),
        IrType::Secret => "Secret".to_owned(),
        // Ipe.Cache config / stats records.
        IrType::CacheCfg => "CacheCfg".to_owned(),
        IrType::CacheStats => "CacheStats".to_owned(),
        // Ipe.WebSocket connect-config record.
        IrType::WebSocketClientCfg => "WebSocketCfg".to_owned(),
        // Ipe.Csv document record.
        IrType::CsvDoc => "Csv".to_owned(),
        // Ipe.Email records + provider ADT (surface names as the Ipê author sees).
        IrType::EmailMessage => "EmailMessage".to_owned(),
        IrType::EmailAttachment => "Attachment".to_owned(),
        IrType::EmailSesConfig => "SesConfig".to_owned(),
        IrType::EmailSmtpConfig => "SmtpConfig".to_owned(),
        IrType::EmailProvider => "EmailProvider".to_owned(),
    }
}

/// Render a binary operator's surface (Ipê source) token.
const fn binop_token(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Neq => "/=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::IntDiv => "//",
        BinOp::Append => "++",
    }
}

/// Render a kernel function's qualified source name.
#[allow(clippy::too_many_lines)]
const fn kernel_name(kernel: KernelFn) -> &'static str {
    match kernel {
        KernelFn::StringFromInt => "String.fromInt",
        KernelFn::StringFromFloat => "String.fromFloat",
        // ── String arity-1 ──────────────────────────────────────────────────
        KernelFn::StringLength => "String.length",
        KernelFn::StringIsEmpty => "String.isEmpty",
        KernelFn::StringReverse => "String.reverse",
        KernelFn::StringToUpper => "String.toUpper",
        KernelFn::StringToLower => "String.toLower",
        KernelFn::StringCasefold => "String.casefold",
        KernelFn::StringTrim => "String.trim",
        KernelFn::StringTrimStart => "String.trimStart",
        KernelFn::StringTrimEnd => "String.trimEnd",
        KernelFn::StringToInt => "String.toInt",
        KernelFn::StringToFloat => "String.toFloat",
        KernelFn::StringFromChar => "String.fromChar",
        KernelFn::StringFromList => "String.fromList",
        KernelFn::StringConcat => "String.concat",
        KernelFn::StringWords => "String.words",
        KernelFn::StringLines => "String.lines",
        KernelFn::StringToList => "String.toList",
        KernelFn::StringIsEmail => "String.isEmail",
        KernelFn::StringIsUrl => "String.isUrl",
        // ── String arity-2 ──────────────────────────────────────────────────
        KernelFn::StringAppend => "String.append",
        KernelFn::StringContains => "String.contains",
        KernelFn::StringStartsWith => "String.startsWith",
        KernelFn::StringEndsWith => "String.endsWith",
        KernelFn::StringEqualFold => "String.equalFold",
        KernelFn::StringJoin => "String.join",
        KernelFn::StringSplit => "String.split",
        KernelFn::StringRepeat => "String.repeat",
        KernelFn::StringDropLeft => "String.dropLeft",
        KernelFn::StringDropRight => "String.dropRight",
        // ── String arity-3 ──────────────────────────────────────────────────
        KernelFn::StringReplace => "String.replace",
        KernelFn::StringSlice => "String.slice",
        KernelFn::StringPadLeft => "String.padLeft",
        KernelFn::StringPadRight => "String.padRight",
        KernelFn::StringContainsIn => "String.containsIn",
        KernelFn::StringStartsWithIn => "String.startsWithIn",
        KernelFn::StringEndsWithIn => "String.endsWithIn",
        KernelFn::StringLeft => "String.left",
        KernelFn::StringRight => "String.right",
        KernelFn::StringCons => "String.cons",
        KernelFn::StringUncons => "String.uncons",
        KernelFn::StringPad => "String.pad",
        KernelFn::StringIndexes => "String.indexes",
        KernelFn::StringMap => "String.map",
        KernelFn::StringFilter => "String.filter",
        KernelFn::StringFoldl => "String.foldl",
        KernelFn::StringFoldr => "String.foldr",
        KernelFn::StringAny => "String.any",
        KernelFn::StringAll => "String.all",
        // ── Char arity-1 ────────────────────────────────────────────────────
        KernelFn::CharIsAlpha => "Char.isAlpha",
        KernelFn::CharIsDigit => "Char.isDigit",
        KernelFn::CharIsLower => "Char.isLower",
        KernelFn::CharIsUpper => "Char.isUpper",
        KernelFn::CharToLower => "Char.toLower",
        KernelFn::CharToUpper => "Char.toUpper",
        KernelFn::CharToCode => "Char.toCode",
        KernelFn::CharFromCode => "Char.fromCode",
        KernelFn::CharIsAlphaNum => "Char.isAlphaNum",
        KernelFn::CharIsHexDigit => "Char.isHexDigit",
        KernelFn::CharIsOctDigit => "Char.isOctDigit",
        KernelFn::LogPrintln => "Log.println",
        KernelFn::LogInfo => "Log.info",
        KernelFn::LogDebug => "Log.debug",
        KernelFn::LogWarn => "Log.warn",
        KernelFn::LogError => "Log.error",
        KernelFn::LogInfoWith => "Log.infoWith",
        KernelFn::LogDebugWith => "Log.debugWith",
        KernelFn::LogWarnWith => "Log.warnWith",
        KernelFn::LogErrorWith => "Log.errorWith",
        KernelFn::ListMap => "List.map",
        KernelFn::ListFilter => "List.filter",
        KernelFn::ListFoldl => "List.foldl",
        KernelFn::ListFoldr => "List.foldr",
        KernelFn::ListLength => "List.length",
        KernelFn::ListHead => "List.head",
        KernelFn::ListTail => "List.tail",
        KernelFn::ListMember => "List.member",
        KernelFn::ListRange => "List.range",
        KernelFn::ListReverse => "List.reverse",
        KernelFn::ListAppend => "List.append",
        KernelFn::ListConcat => "List.concat",
        KernelFn::ListTake => "List.take",
        KernelFn::ListDrop => "List.drop",
        KernelFn::ListZip => "List.zip",
        KernelFn::ListCons => "List.cons",
        KernelFn::ListIsEmpty => "List.isEmpty",
        KernelFn::ListConcatMap => "List.concatMap",
        KernelFn::ListIndexedMap => "List.indexedMap",
        KernelFn::ListAny => "List.any",
        KernelFn::ListAll => "List.all",
        KernelFn::ListFind => "List.find",
        // ── List batch ────────────────────────────────────────────────
        KernelFn::ListFilterMap => "List.filterMap",
        KernelFn::ListSortBy => "List.sortBy",
        KernelFn::ListSort => "List.sort",
        KernelFn::ListSortWith => "List.sortWith",
        KernelFn::ListSingleton => "List.singleton",
        KernelFn::ListRepeat => "List.repeat",
        KernelFn::ListSum => "List.sum",
        KernelFn::ListProduct => "List.product",
        KernelFn::ListMaximum => "List.maximum",
        KernelFn::ListMinimum => "List.minimum",
        KernelFn::ListIntersperse => "List.intersperse",
        KernelFn::ListPartition => "List.partition",
        KernelFn::ListUnzip => "List.unzip",
        KernelFn::ListMap2 => "List.map2",
        KernelFn::ListMap3 => "List.map3",
        KernelFn::ListMap4 => "List.map4",
        KernelFn::ListMap5 => "List.map5",
        KernelFn::BasicsNot => "Basics.not",
        KernelFn::BasicsIdentity => "Basics.identity",
        KernelFn::BasicsAlways => "Basics.always",
        KernelFn::BasicsFst => "Basics.fst",
        KernelFn::BasicsSnd => "Basics.snd",
        KernelFn::BasicsModBy => "Basics.modBy",
        KernelFn::BasicsClamp => "Basics.clamp",
        KernelFn::BasicsToString => "Basics.toString",
        // ── Basics numerics ──────────────────────────────────────────
        KernelFn::BasicsNegate => "Basics.negate",
        KernelFn::BasicsAbs => "Basics.abs",
        KernelFn::BasicsSqrt => "Basics.sqrt",
        KernelFn::BasicsMin => "Basics.min",
        KernelFn::BasicsMax => "Basics.max",
        // ── end Basics numerics ──────────────────────────────────────
        // ── Error kernels (Ipe.Error — minimal `Error = String` slice) ─
        KernelFn::ErrorUnexpected => "Error.unexpected",
        KernelFn::ErrorInvalidInput => "Error.invalidInput",
        KernelFn::ErrorIo => "Error.io",
        KernelFn::ErrorNetwork => "Error.network",
        KernelFn::ErrorFfi => "Error.ffi",
        KernelFn::ErrorDecode => "Error.decode",
        KernelFn::ErrorConflict => "Error.conflict",
        KernelFn::ErrorUnavailable => "Error.unavailable",
        KernelFn::ErrorTimeout => "Error.timeout",
        KernelFn::ErrorNotFound => "Error.notFound",
        KernelFn::ErrorPermissionDenied => "Error.permissionDenied",
        KernelFn::ErrorToString => "Error.toString",
        KernelFn::ErrorWithMessage => "Error.withMessage",
        KernelFn::ErrorIsRetryable => "Error.isRetryable",
        KernelFn::ErrorWithDetails => "Error.withDetails",
        // CssSafety (Ipe.CssSafety — Ipe.Css leaf security kernels)
        KernelFn::CssSafetySafeValue => "CssSafety.safeValue",
        KernelFn::CssSafetySafePropName => "CssSafety.safePropName",
        KernelFn::CssSafetySafeSelector => "CssSafety.safeSelector",
        KernelFn::CssSafetyStripStyleClose => "CssSafety.stripStyleClose",
        KernelFn::MaybeWithDefault => "Maybe.withDefault",
        KernelFn::MaybeMap => "Maybe.map",
        KernelFn::MaybeAndThen => "Maybe.andThen",
        KernelFn::MaybeMap2 => "Maybe.map2",
        KernelFn::MaybeMap3 => "Maybe.map3",
        KernelFn::MaybeMap4 => "Maybe.map4",
        KernelFn::MaybeMap5 => "Maybe.map5",
        KernelFn::MaybeAndMap => "Maybe.andMap",
        KernelFn::MaybeCombine => "Maybe.combine",
        KernelFn::ResultWithDefault => "Result.withDefault",
        KernelFn::ResultMap => "Result.map",
        KernelFn::ResultAndThen => "Result.andThen",
        KernelFn::ResultMapError => "Result.mapError",
        KernelFn::ResultMap2 => "Result.map2",
        KernelFn::ResultMap3 => "Result.map3",
        KernelFn::ResultMap4 => "Result.map4",
        KernelFn::ResultMap5 => "Result.map5",
        KernelFn::ResultAndMap => "Result.andMap",
        KernelFn::ResultCombine => "Result.combine",
        KernelFn::ResultTraverse => "Result.traverse",
        KernelFn::ResultToMaybe => "Result.toMaybe",
        KernelFn::ResultFromMaybe => "Result.fromMaybe",
        KernelFn::MathMin => "Math.min",
        KernelFn::MathMax => "Math.max",
        // ── Math constants ───────────────────────────────────────────────────
        KernelFn::MathPi => "Math.pi",
        KernelFn::MathE => "Math.e",
        KernelFn::MathPhi => "Math.phi",
        KernelFn::MathSqrt2 => "Math.sqrt2",
        KernelFn::MathInf => "Math.inf",
        KernelFn::MathNan => "Math.nan",
        KernelFn::MathIsNaN => "Math.isNaN",
        // ── Math arity-1 (Int → Int) ─────────────────────────────────────────
        KernelFn::MathAbs => "Math.abs",
        // ── Math arity-1 (Float → Float) ────────────────────────────────────
        KernelFn::MathSqrt => "Math.sqrt",
        KernelFn::MathCbrt => "Math.cbrt",
        KernelFn::MathExp => "Math.exp",
        KernelFn::MathExp2 => "Math.exp2",
        KernelFn::MathLog => "Math.log",
        KernelFn::MathLog2 => "Math.log2",
        KernelFn::MathLog10 => "Math.log10",
        KernelFn::MathSin => "Math.sin",
        KernelFn::MathCos => "Math.cos",
        KernelFn::MathTan => "Math.tan",
        KernelFn::MathAsin => "Math.asin",
        KernelFn::MathAcos => "Math.acos",
        KernelFn::MathAtan => "Math.atan",
        KernelFn::MathSinh => "Math.sinh",
        KernelFn::MathCosh => "Math.cosh",
        KernelFn::MathTanh => "Math.tanh",
        KernelFn::MathAsinh => "Math.asinh",
        KernelFn::MathAcosh => "Math.acosh",
        KernelFn::MathAtanh => "Math.atanh",
        // ── Math arity-1 (Float → Int) ───────────────────────────────────────
        KernelFn::MathFloor => "Math.floor",
        KernelFn::MathCeil => "Math.ceil",
        KernelFn::MathRound => "Math.round",
        KernelFn::MathTrunc => "Math.trunc",
        // ── Math arity-2 (Float → Float → Float) ────────────────────────────
        KernelFn::MathPow => "Math.pow",
        KernelFn::MathHypot => "Math.hypot",
        KernelFn::MathAtan2 => "Math.atan2",
        KernelFn::MathMod => "Math.mod",
        KernelFn::MathRemainder => "Math.remainder",
        KernelFn::ResultOkDefault => "Result.Ok",
        // ── Dict kernels ─────────────────────────────────────────────────────
        KernelFn::DictEmpty => "Dict.empty",
        KernelFn::DictIsEmpty => "Dict.isEmpty",
        KernelFn::DictSize => "Dict.size",
        KernelFn::DictKeys => "Dict.keys",
        KernelFn::DictValues => "Dict.values",
        KernelFn::DictToList => "Dict.toList",
        KernelFn::DictFromList => "Dict.fromList",
        KernelFn::DictGet => "Dict.get",
        KernelFn::DictMember => "Dict.member",
        KernelFn::DictRemove => "Dict.remove",
        KernelFn::DictUnion => "Dict.union",
        KernelFn::DictMap => "Dict.map",
        KernelFn::DictInsert => "Dict.insert",
        KernelFn::DictFoldl => "Dict.foldl",
        KernelFn::DictSingleton => "Dict.singleton",
        KernelFn::DictFoldr => "Dict.foldr",
        KernelFn::DictFilter => "Dict.filter",
        KernelFn::DictPartition => "Dict.partition",
        KernelFn::DictIntersect => "Dict.intersect",
        KernelFn::DictDiff => "Dict.diff",
        KernelFn::DictUpdate => "Dict.update",
        // ── Set kernels ──────────────────────────────────────────────────────
        KernelFn::SetEmpty => "Set.empty",
        KernelFn::SetSize => "Set.size",
        KernelFn::SetToList => "Set.toList",
        KernelFn::SetFromList => "Set.fromList",
        KernelFn::SetMember => "Set.member",
        KernelFn::SetInsert => "Set.insert",
        KernelFn::SetRemove => "Set.remove",
        KernelFn::SetUnion => "Set.union",
        KernelFn::SetIntersect => "Set.intersect",
        KernelFn::SetDiff => "Set.diff",
        KernelFn::SetIsEmpty => "Set.isEmpty",
        KernelFn::SetSingleton => "Set.singleton",
        KernelFn::SetFoldl => "Set.foldl",
        KernelFn::SetFoldr => "Set.foldr",
        KernelFn::SetMap => "Set.map",
        KernelFn::SetFilter => "Set.filter",
        KernelFn::SetPartition => "Set.partition",
        // ── Bytes kernels ──────────────────────────────────────────────
        KernelFn::BytesEmpty => "Bytes.empty",
        KernelFn::BytesLength => "Bytes.length",
        KernelFn::BytesIsEmpty => "Bytes.isEmpty",
        KernelFn::BytesFromString => "Bytes.fromString",
        KernelFn::BytesToString => "Bytes.toString",
        KernelFn::BytesFromHex => "Bytes.fromHex",
        KernelFn::BytesToHex => "Bytes.toHex",
        KernelFn::BytesFromBase64 => "Bytes.fromBase64",
        KernelFn::BytesToBase64 => "Bytes.toBase64",
        KernelFn::BytesAppend => "Bytes.append",
        KernelFn::BytesSlice => "Bytes.slice",
        // ── Encoding ────────────────────────────────────────────────────
        KernelFn::EncodingBase64Encode => "Encoding.base64Encode",
        KernelFn::EncodingBase64Decode => "Encoding.base64Decode",
        KernelFn::EncodingUrlEncode => "Encoding.urlEncode",
        KernelFn::EncodingUrlDecode => "Encoding.urlDecode",
        KernelFn::EncodingHexEncode => "Encoding.hexEncode",
        KernelFn::EncodingHexDecode => "Encoding.hexDecode",
        // ── JsonEnc ────────────────────────────────────────────────────
        KernelFn::JsonEncString => "JsonEnc.string",
        KernelFn::JsonEncInt => "JsonEnc.int",
        KernelFn::JsonEncFloat => "JsonEnc.float",
        KernelFn::JsonEncBool => "JsonEnc.bool",
        KernelFn::JsonEncNull => "JsonEnc.null",
        KernelFn::JsonEncList => "JsonEnc.list",
        KernelFn::JsonEncObject => "JsonEnc.object",
        KernelFn::JsonEncEncode => "JsonEnc.encode",
        // ── JsonDec ────────────────────────────────────────────────────
        KernelFn::JsonDecString => "JsonDec.string",
        KernelFn::JsonDecInt => "JsonDec.int",
        KernelFn::JsonDecFloat => "JsonDec.float",
        KernelFn::JsonDecBool => "JsonDec.bool",
        KernelFn::JsonDecDecodeString => "JsonDec.decodeString",
        KernelFn::JsonDecField => "JsonDec.field",
        KernelFn::JsonDecAt => "JsonDec.at",
        KernelFn::JsonDecIndex => "JsonDec.index",
        KernelFn::JsonDecList => "JsonDec.list",
        KernelFn::JsonDecMap => "JsonDec.map",
        KernelFn::JsonDecAndThen => "JsonDec.andThen",
        KernelFn::JsonDecSucceed => "JsonDec.succeed",
        KernelFn::JsonDecFail => "JsonDec.fail",
        KernelFn::JsonDecOneOf => "JsonDec.oneOf",
        KernelFn::JsonDecMap2 => "JsonDec.map2",
        KernelFn::JsonDecMap3 => "JsonDec.map3",
        KernelFn::JsonDecMap4 => "JsonDec.map4",
        // ── JsonDecP ───────────────────────────────────────────────────
        KernelFn::JsonDecPRequired => "JsonDecP.required",
        KernelFn::JsonDecPOptional => "JsonDecP.optional",
        KernelFn::JsonDecPCustom => "JsonDecP.custom",
        KernelFn::JsonDecPRequiredAt => "JsonDecP.requiredAt",
        // ── Crypto kernels ─────────────────────────────────────────────
        KernelFn::CryptoSha256 => "Crypto.sha256",
        KernelFn::CryptoSha512 => "Crypto.sha512",
        KernelFn::CryptoSha1 => "Crypto.sha1",
        KernelFn::CryptoMd5 => "Crypto.md5",
        KernelFn::CryptoHmacSha256 => "Crypto.hmacSha256",
        KernelFn::CryptoHmacSha512 => "Crypto.hmacSha512",
        KernelFn::CryptoRsaSha256Sign => "Crypto.rsaSha256Sign",
        KernelFn::CryptoRsaSha256Verify => "Crypto.rsaSha256Verify",
        KernelFn::CryptoConstantTimeEqual => "Crypto.constantTimeEqual",
        KernelFn::CryptoAesGcmEncrypt => "Crypto.aesGcmEncrypt",
        KernelFn::CryptoAesGcmDecrypt => "Crypto.aesGcmDecrypt",
        KernelFn::CryptoChacha20Encrypt => "Crypto.chacha20Encrypt",
        KernelFn::CryptoChacha20Decrypt => "Crypto.chacha20Decrypt",
        KernelFn::CryptoAesKeyFromPassword => "Crypto.aesKeyFromPassword",
        KernelFn::CryptoChachaKeyFromPassword => "Crypto.chachaKeyFromPassword",
        KernelFn::CryptoRandomBytes => "Crypto.randomBytes",
        KernelFn::CryptoRandomToken => "Crypto.randomToken",
        // ── Uuid kernels ───────────────────────────────────────────────
        KernelFn::UuidV4 => "Uuid.v4",
        KernelFn::UuidV7 => "Uuid.v7",
        KernelFn::UuidParse => "Uuid.parse",
        // ── Jwt kernels ────────────────────────────────────────────────
        KernelFn::JwtEncodeHs256 => "Jwt.encodeHs256",
        KernelFn::JwtDecodeHs256 => "Jwt.decodeHs256",
        KernelFn::JwtEncodeRs256 => "Jwt.encodeRs256",
        KernelFn::JwtDecodeRs256 => "Jwt.decodeRs256",
        // ── Task combinators ────────────────────────────────────────────
        KernelFn::TaskSucceed => "Task.succeed",
        KernelFn::TaskFail => "Task.fail",
        KernelFn::TaskMap => "Task.map",
        KernelFn::TaskMap2 => "Task.map2",
        KernelFn::TaskMap3 => "Task.map3",
        KernelFn::TaskMap4 => "Task.map4",
        KernelFn::TaskMap5 => "Task.map5",
        KernelFn::TaskAttempt => "Task.attempt",
        KernelFn::TaskAndThen => "Task.andThen",
        KernelFn::TaskMapError => "Task.mapError",
        KernelFn::TaskOnError => "Task.onError",
        KernelFn::TaskFromResult => "Task.fromResult",
        KernelFn::TaskAndThenResult => "Task.andThenResult",
        KernelFn::TaskSequence => "Task.sequence",
        KernelFn::TaskParallel => "Task.parallel",
        KernelFn::TaskRun => "Task.run",
        KernelFn::TaskPerform => "Task.perform",
        KernelFn::TaskLazy => "Task.lazy",
        // ── Task retry surface ──────────────────────────────────────────
        KernelFn::TaskRetryWith => "Task.retryWith",
        KernelFn::TaskLinearBackoff => "Task.linearBackoff",
        KernelFn::TaskExponentialBackoff => "Task.exponentialBackoff",
        KernelFn::TaskWithJitter => "Task.withJitter",
        KernelFn::TaskRetryOn => "Task.retryOn",
        KernelFn::TaskWithRetryOn => "Task.withRetryOn",
        KernelFn::TaskDefaultRetryPolicy => "Task.defaultRetryPolicy",
        KernelFn::TaskWithMaxAttempts => "Task.withMaxAttempts",
        KernelFn::TaskWithBaseMs => "Task.withBaseMs",
        KernelFn::TaskWithKind => "Task.withKind",
        // ── Io kernels ──────────────────────────────────────────────────
        KernelFn::IoReadLine => "Io.readLine",
        KernelFn::IoWriteStdout => "Io.writeStdout",
        KernelFn::IoWriteStderr => "Io.writeStderr",
        // ── Time kernels ────────────────────────────────────────────────
        KernelFn::TimeNow => "Time.now",
        KernelFn::TimeSleep => "Time.sleep",
        KernelFn::TimeUnixMillis => "Time.unixMillis",
        KernelFn::TimeTimeString => "Time.timeString",
        KernelFn::TimeIsLeapYear => "Time.isLeapYear",
        KernelFn::TimeDaysInMonth => "Time.daysInMonth",
        // ── System kernels ──────────────────────────────────────────────
        KernelFn::SystemArgs => "System.args",
        KernelFn::SystemGetenv => "System.getenv",
        KernelFn::SystemGetenvOr => "System.getenvOr",
        KernelFn::SystemGetArg => "System.getArg",
        KernelFn::SystemGetenvInt => "System.getenvInt",
        KernelFn::SystemGetenvBool => "System.getenvBool",
        KernelFn::SystemSetenv => "System.setenv",
        KernelFn::SystemUnsetenv => "System.unsetenv",
        KernelFn::SystemCwd => "System.cwd",
        KernelFn::SystemLoadEnv => "System.loadEnv",
        KernelFn::SystemExit => "System.exit",
        // ── Random kernels ──────────────────────────────────────────────
        KernelFn::RandomInt => "Random.int",
        KernelFn::RandomFloat => "Random.float",
        KernelFn::RandomChoice => "Random.choice",
        // ── File kernels ────────────────────────────────────────────────
        KernelFn::FileReadFile => "File.readFile",
        KernelFn::FileWriteFile => "File.writeFile",
        KernelFn::FileExists => "File.exists",
        KernelFn::FileRemove => "File.remove",
        KernelFn::FileMkdirAll => "File.mkdirAll",
        KernelFn::FileReadFileLimit => "File.readFileLimit",
        KernelFn::FileReadFileBytes => "File.readFileBytes",
        KernelFn::FileAppend => "File.append",
        KernelFn::FileReadDir => "File.readDir",
        KernelFn::FileIsDir => "File.isDir",
        KernelFn::FileTempFile => "File.tempFile",
        KernelFn::FileTempDir => "File.tempDir",
        KernelFn::FileCopy => "File.copy",
        KernelFn::FileRename => "File.rename",
        KernelFn::FileDelete => "File.delete",
        // ── Http kernels ──────────────────────────────────────────────
        KernelFn::HttpGet => "Http.get",
        KernelFn::HttpPost => "Http.post",
        KernelFn::HttpRequest => "Http.request",
        KernelFn::HttpParseQuery => "Http.parseQuery",
        KernelFn::HttpDefaultRequest => "Http.defaultRequest",
        KernelFn::HttpWithMethod => "Http.withMethod",
        KernelFn::HttpWithTimeout => "Http.withTimeout",
        KernelFn::HttpWithBody => "Http.withBody",
        KernelFn::HttpWithHeader => "Http.withHeader",
        KernelFn::HttpWithUrl => "Http.withUrl",
        KernelFn::HttpWithFollowRedirects => "Http.withFollowRedirects",
        KernelFn::HttpWithMaxRedirects => "Http.withMaxRedirects",
        // ── Db kernels ──────────────────────────────────────────────
        KernelFn::DbConnect => "Db.connect",
        KernelFn::DbOpen => "Db.open",
        KernelFn::DbClose => "Db.close",
        KernelFn::DbExecRaw => "Db.execRaw",
        KernelFn::DbExec => "Db.exec",
        KernelFn::DbQuery => "Db.query",
        KernelFn::DbQueryDecode => "Db.queryDecode",
        KernelFn::DbGetString => "Db.getString",
        KernelFn::DbGetInt => "Db.getInt",
        KernelFn::DbGetBool => "Db.getBool",
        KernelFn::DbGetField => "Db.getField",
        KernelFn::DbInsertRow => "Db.insertRow",
        KernelFn::DbGetById => "Db.getById",
        KernelFn::DbUpdateById => "Db.updateById",
        KernelFn::DbDeleteById => "Db.deleteById",
        KernelFn::DbFindOneByField => "Db.findOneByField",
        KernelFn::DbFindManyByField => "Db.findManyByField",
        KernelFn::DbFindByConditions => "Db.findByConditions",
        KernelFn::DbFindWhere => "Db.findWhere",
        KernelFn::DbDeleteWhere => "Db.deleteWhere",
        KernelFn::DbInsertFields => "Db.insertFields",
        KernelFn::DbUpdateFields => "Db.updateFields",
        KernelFn::DbInsertFieldsReturning => "Db.insertFieldsReturning",
        KernelFn::DbWithTransaction => "Db.withTransaction",
        KernelFn::DbMigrate => "Db.migrate",
        KernelFn::DbDefaultMigration => "Db.defaultMigration",
        // ── Db.Decode kernels ───────────────────────────────────────
        KernelFn::DbDecString => "Db.Decode.string",
        KernelFn::DbDecInt => "Db.Decode.int",
        KernelFn::DbDecFloat => "Db.Decode.float",
        KernelFn::DbDecBool => "Db.Decode.bool",
        KernelFn::DbDecNullable => "Db.Decode.nullable",
        KernelFn::DbDecMap => "Db.Decode.map",
        KernelFn::DbDecAndThen => "Db.Decode.andThen",
        KernelFn::DbDecSucceed => "Db.Decode.succeed",
        KernelFn::DbDecFail => "Db.Decode.fail",
        KernelFn::DbDecMap2 => "Db.Decode.map2",
        KernelFn::DbDecMap3 => "Db.Decode.map3",
        KernelFn::DbDecMap4 => "Db.Decode.map4",
        KernelFn::DbDecRequired => "Db.Decode.required",
        KernelFn::DbDecOptional => "Db.Decode.optional",
        KernelFn::DbDecMoney => "Db.Decode.money",
        KernelFn::DbDecBytes => "Db.Decode.bytes",
        // ── Ipe.Db.Sql — SqlFragment builder ───────────────────
        KernelFn::SqlColumn => "Sql.column",
        KernelFn::SqlParam => "Sql.param",
        KernelFn::SqlInt => "Sql.int",
        KernelFn::SqlString => "Sql.string",
        KernelFn::SqlFloat => "Sql.float",
        KernelFn::SqlBool => "Sql.bool",
        KernelFn::SqlEq => "Sql.eq",
        KernelFn::SqlNe => "Sql.ne",
        KernelFn::SqlGt => "Sql.gt",
        KernelFn::SqlLt => "Sql.lt",
        KernelFn::SqlGte => "Sql.gte",
        KernelFn::SqlLte => "Sql.lte",
        KernelFn::SqlAnd => "Sql.and",
        KernelFn::SqlOr => "Sql.or",
        KernelFn::SqlNot => "Sql.not",
        KernelFn::SqlIsNull => "Sql.isNull",
        KernelFn::SqlIsNotNull => "Sql.isNotNull",
        KernelFn::SqlInList => "Sql.inList",
        KernelFn::SqlLike => "Sql.like",
        KernelFn::SecretFromString => "Secret.fromString",
        KernelFn::SecretReveal => "Secret.reveal",
        KernelFn::SecretRedacted => "Secret.redacted",
        // Ipe.Regex
        KernelFn::RegexMatch => "Regex.match",
        KernelFn::RegexFind => "Regex.find",
        KernelFn::RegexFindAll => "Regex.findAll",
        KernelFn::RegexReplace => "Regex.replace",
        KernelFn::RegexSplit => "Regex.split",
        // Ipe.Path
        KernelFn::PathBase => "Path.base",
        KernelFn::PathDir => "Path.dir",
        KernelFn::PathExt => "Path.ext",
        KernelFn::PathIsAbsolute => "Path.isAbsolute",
        // Ipe.Trace
        KernelFn::TraceSpan => "Trace.span",
        KernelFn::TraceEvent => "Trace.event",
        KernelFn::TraceAttr => "Trace.attr",
        // Ipe.Compression
        KernelFn::CompressionGzip => "Compression.gzip",
        KernelFn::CompressionGunzip => "Compression.gunzip",
        KernelFn::CompressionZstdCompress => "Compression.zstdCompress",
        KernelFn::CompressionZstdDecompress => "Compression.zstdDecompress",
        // Ipe.Csv
        KernelFn::CsvParse => "Csv.parse",
        KernelFn::CsvParseWithDelimiter => "Csv.parseWithDelimiter",
        KernelFn::CsvEncode => "Csv.encode",
        KernelFn::CsvEncodeWithDelimiter => "Csv.encodeWithDelimiter",
        KernelFn::CsvParseStreamFromFile => "Csv.parseStreamFromFile",
        // Ipe.Cache (the `*Raw` kernel aliases; the surface names carry
        // no `Raw` suffix but the pretty form names the underlying kernel).
        KernelFn::CacheNewRaw => "Cache.newRaw",
        KernelFn::CacheGet => "Cache.getRaw",
        KernelFn::CachePut => "Cache.putRaw",
        KernelFn::CacheRemove => "Cache.removeRaw",
        KernelFn::CacheClear => "Cache.clearRaw",
        KernelFn::CacheSize => "Cache.sizeRaw",
        KernelFn::CacheStats => "Cache.statsRaw",
        // Ipe.Config
        KernelFn::ConfigString => "Config.string",
        KernelFn::ConfigInt => "Config.int",
        KernelFn::ConfigFloat => "Config.float",
        KernelFn::ConfigBool => "Config.bool",
        KernelFn::ConfigNullable => "Config.nullable",
        KernelFn::ConfigField => "Config.field",
        KernelFn::ConfigAt => "Config.at",
        KernelFn::ConfigList => "Config.list",
        KernelFn::ConfigSucceed => "Config.succeed",
        KernelFn::ConfigFail => "Config.fail",
        KernelFn::ConfigMap => "Config.map",
        KernelFn::ConfigAndThen => "Config.andThen",
        KernelFn::ConfigMap2 => "Config.map2",
        KernelFn::ConfigMap3 => "Config.map3",
        KernelFn::ConfigMap4 => "Config.map4",
        KernelFn::ConfigMap5 => "Config.map5",
        KernelFn::ConfigMap6 => "Config.map6",
        KernelFn::ConfigMap7 => "Config.map7",
        KernelFn::ConfigMap8 => "Config.map8",
        KernelFn::ConfigOneOf => "Config.oneOf",
        KernelFn::ConfigIndex => "Config.index",
        KernelFn::ConfigKeyValuePairs => "Config.keyValuePairs",
        KernelFn::ConfigMaybe => "Config.maybe",
        KernelFn::ConfigDict => "Config.dict",
        KernelFn::ConfigDecodeToml => "Config.decodeToml",
        KernelFn::ConfigDecodeYaml => "Config.decodeYaml",
        KernelFn::ConfigDecodeJson => "Config.decodeJson",
        KernelFn::ConfigLoadFromFile => "Config.loadFromFile",
        // Ipe.Email
        KernelFn::EmailSend => "Email.send",
        // TEA Cmd / Sub / Time.every
        KernelFn::CmdNone => "Cmd.none",
        KernelFn::CmdBatch => "Cmd.batch",
        KernelFn::CmdPerform => "Cmd.perform",
        KernelFn::CmdMap => "Cmd.map",
        KernelFn::SubNone => "Sub.none",
        KernelFn::SubBatch => "Sub.batch",
        KernelFn::SubEvery => "Sub.every",
        KernelFn::SubMap => "Sub.map",
        KernelFn::TimeEvery => "Time.every",
        // reserved
        KernelFn::CmdPublish => "Cmd.publish",
        KernelFn::CmdPublishNoEcho => "Cmd.publishNoEcho",
        KernelFn::SubSubscribeTopic => "Sub.subscribeTopic",
        KernelFn::PubSubPublish => "PubSub.publish",
        KernelFn::PubSubPublishNoEcho => "PubSub.publishNoEcho",
        // Ipe.Http.Server kernels
        KernelFn::ServerGet => "Server.get",
        KernelFn::ServerPost => "Server.post",
        KernelFn::ServerPut => "Server.put",
        KernelFn::ServerDelete => "Server.delete",
        KernelFn::ServerAny => "Server.any",
        KernelFn::ServerApi => "Server.api",
        KernelFn::ServerStatic => "Server.static",
        KernelFn::ServerListen => "Server.listen",
        KernelFn::ServerText => "Server.text",
        KernelFn::ServerJson => "Server.json",
        KernelFn::ServerHtml => "Server.html",
        KernelFn::ServerWithStatus => "Server.withStatus",
        KernelFn::ServerWithHeader => "Server.withHeader",
        KernelFn::ServerRedirect => "Server.redirect",
        KernelFn::ServerParam => "Server.param",
        KernelFn::ServerQueryParam => "Server.queryParam",
        KernelFn::ServerHeader => "Server.header",
        KernelFn::ServerGetCookie => "Server.getCookie",
        KernelFn::ServerBody => "Server.body",
        KernelFn::ServerPath => "Server.path",
        KernelFn::ServerMethod => "Server.method",
        KernelFn::ServerCookieNew => "Server.cookie",
        KernelFn::ServerWithCookie => "Server.withCookie",
        KernelFn::MiddlewareWithCors => "Middleware.withCors",
        KernelFn::MiddlewareWithLogging => "Middleware.withLogging",
        KernelFn::MiddlewareWithBasicAuth => "Middleware.withBasicAuth",
        KernelFn::MiddlewareWithRateLimit => "Middleware.withRateLimit",
        KernelFn::MiddlewareWithCsrf => "Middleware.withCsrf",
        KernelFn::RateLimitAllow => "RateLimit.allow",
        // ── Ipe.Ui / Ipe.Html render kernels ─────────────────────────────
        KernelFn::UiLayout => "Ui.layout",
        KernelFn::UiLayoutWith => "Ui.layoutWith",
        KernelFn::HtmlRender => "Html.render",
        KernelFn::HtmlEscapeText => "Html.escapeText",
        KernelFn::HtmlEscapeAttr => "Html.escapeAttr",
        KernelFn::HtmlAttrToString => "Html.attrToString",
        // ── Ipe.Live app-entry kernels ───────────────────────────────────
        KernelFn::LiveApp => "Live.app",
        KernelFn::LiveAppRouted => "Live.appRouted",
        KernelFn::LiveRoute => "Live.route",
        KernelFn::LiveRenderStatic => "Live.renderStatic",
        // ── Ipe.Tui app-entry kernels ────────────────────────────────────
        KernelFn::TuiProgram => "Tui.program",
        KernelFn::TuiApp => "Tui.app",
        // ── Ipe.Webview app-entry kernel ─────────────────────────────────
        KernelFn::WebviewApp => "Webview.app",
        // ── Ipe.Ui element builders ──────────────────────────────────────
        KernelFn::UiNone => "Ui.none",
        KernelFn::UiText => "Ui.text",
        KernelFn::UiHtml => "Ui.html",
        KernelFn::UiEl => "Ui.el",
        KernelFn::UiRow => "Ui.row",
        KernelFn::UiColumn => "Ui.column",
        KernelFn::UiWrappedRow => "Ui.wrappedRow",
        KernelFn::UiGrid => "Ui.grid",
        KernelFn::UiParagraph => "Ui.paragraph",
        KernelFn::UiTextColumn => "Ui.textColumn",
        KernelFn::UiButton => "Ui.button",
        KernelFn::UiLink => "Ui.link",
        KernelFn::UiImage => "Ui.image",
        // ── Ipe.Ui attribute builders ────────────────────────────────────
        KernelFn::UiSpacing => "Ui.spacing",
        KernelFn::UiPadding => "Ui.padding",
        KernelFn::UiPaddingXY => "Ui.paddingXY",
        KernelFn::UiPaddingEach => "Ui.paddingEach",
        KernelFn::UiWidth => "Ui.width",
        KernelFn::UiHeight => "Ui.height",
        KernelFn::UiCenterX => "Ui.centerX",
        KernelFn::UiCenterY => "Ui.centerY",
        KernelFn::UiAlignLeft => "Ui.alignLeft",
        KernelFn::UiAlignRight => "Ui.alignRight",
        KernelFn::UiAlignTop => "Ui.alignTop",
        KernelFn::UiAlignBottom => "Ui.alignBottom",
        KernelFn::UiPointer => "Ui.pointer",
        KernelFn::UiClip => "Ui.clip",
        KernelFn::UiClipX => "Ui.clipX",
        KernelFn::UiClipY => "Ui.clipY",
        KernelFn::UiScrollbars => "Ui.scrollbars",
        KernelFn::UiScrollbarX => "Ui.scrollbarX",
        KernelFn::UiScrollbarY => "Ui.scrollbarY",
        KernelFn::UiGridColumns => "Ui.gridColumns",
        // ── Ipe.Ui Length builders ───────────────────────────────────────
        KernelFn::UiPx => "Ui.px",
        KernelFn::UiFill => "Ui.fill",
        KernelFn::UiContent => "Ui.content",
        KernelFn::UiShrink => "Ui.shrink",
        KernelFn::UiFillPortion => "Ui.fillPortion",
        KernelFn::UiVh => "Ui.vh",
        KernelFn::UiVw => "Ui.vw",
        KernelFn::UiMinimum => "Ui.minimum",
        KernelFn::UiMaximum => "Ui.maximum",
        // ── Ipe.Ui Color builders ────────────────────────────────────────
        KernelFn::UiRgb => "Ui.rgb",
        KernelFn::UiRgba => "Ui.rgba",
        KernelFn::UiWhite => "Ui.white",
        KernelFn::UiBlack => "Ui.black",
        KernelFn::UiTransparent => "Ui.transparent",
        KernelFn::UiColorCss => "Ui.colorCss",
        // ── Background / Border / Font sub-modules ───────────────────────
        KernelFn::BackgroundColor => "Background.color",
        KernelFn::BackgroundImage => "Background.image",
        KernelFn::BackgroundLinearGradient => "Background.linearGradient",
        KernelFn::BorderWidth => "Border.width",
        KernelFn::BorderRounded => "Border.rounded",
        KernelFn::BorderColor => "Border.color",
        KernelFn::BorderWidthEach => "Border.widthEach",
        KernelFn::BorderShadow => "Border.shadow",
        KernelFn::BorderGlow => "Border.glow",
        KernelFn::BorderInnerShadow => "Border.innerShadow",
        KernelFn::FontSize => "Font.size",
        KernelFn::FontColor => "Font.color",
        KernelFn::FontFamily => "Font.family",
        KernelFn::FontBold => "Font.bold",
        KernelFn::FontItalic => "Font.italic",
        KernelFn::UiSquare => "Ui.square",
        KernelFn::UiWidescreen => "Ui.widescreen",
        KernelFn::UiCinemascope => "Ui.cinemascope",
        KernelFn::UiAspectRatio => "Ui.aspectRatio",
        KernelFn::UiAspectRatioWH => "Ui.aspectRatioWH",
        KernelFn::UiHtmlAttribute => "Ui.htmlAttribute",
        KernelFn::UiName => "Ui.name",
        KernelFn::UiStyle => "Ui.style",
        KernelFn::UiTransitionRaw => "Ui.transitionRaw",
        KernelFn::UiGridTracksRaw => "Ui.gridTracksRaw",
        KernelFn::UiAnimateRaw => "Ui.animateRaw",
        KernelFn::UiBreakpoint => "Ui.breakpoint",
        KernelFn::UiMediaQuery => "Ui.mediaQuery",
        KernelFn::UiMobile => "Ui.mobile",
        KernelFn::UiTablet => "Ui.tablet",
        KernelFn::UiDesktop => "Ui.desktop",
        KernelFn::UiDarkMode => "Ui.darkMode",
        KernelFn::UiLightMode => "Ui.lightMode",
        KernelFn::UiReducedMotion => "Ui.reducedMotion",
        KernelFn::UiOnPseudo => "Ui.onPseudo",
        KernelFn::UiHover => "Ui.hover",
        KernelFn::UiFocus => "Ui.focus",
        KernelFn::UiFocusVisible => "Ui.focusVisible",
        KernelFn::UiActive => "Ui.active",
        KernelFn::UiDisabled => "Ui.disabled",
        KernelFn::BackgroundHoverColor => "Background.hoverColor",
        KernelFn::BackgroundFocusColor => "Background.focusColor",
        KernelFn::BackgroundActiveColor => "Background.activeColor",
        KernelFn::BackgroundDisabledColor => "Background.disabledColor",
        KernelFn::BorderSolid => "Border.solid",
        KernelFn::BorderDashed => "Border.dashed",
        KernelFn::BorderDotted => "Border.dotted",
        KernelFn::BorderHoverColor => "Border.hoverColor",
        KernelFn::BorderFocusColor => "Border.focusColor",
        KernelFn::BorderActiveColor => "Border.activeColor",
        KernelFn::BorderHoverWidth => "Border.hoverWidth",
        KernelFn::BorderHoverRounded => "Border.hoverRounded",
        KernelFn::FontWeight => "Font.weight",
        KernelFn::FontSemiBold => "Font.semiBold",
        KernelFn::FontRegular => "Font.regular",
        KernelFn::FontLight => "Font.light",
        KernelFn::FontExtraBold => "Font.extraBold",
        KernelFn::FontBlack => "Font.black",
        KernelFn::FontUnderline => "Font.underline",
        KernelFn::FontNoDecoration => "Font.noDecoration",
        KernelFn::FontLineThrough => "Font.lineThrough",
        KernelFn::FontLetterSpacing => "Font.letterSpacing",
        KernelFn::FontWordSpacing => "Font.wordSpacing",
        KernelFn::FontAlignLeft => "Font.alignLeft",
        KernelFn::FontAlignRight => "Font.alignRight",
        KernelFn::FontAlignCenter => "Font.alignCenter",
        KernelFn::FontCenter => "Font.center",
        KernelFn::FontJustify => "Font.justify",
        KernelFn::FontSansSerif => "Font.sansSerif",
        KernelFn::FontSerif => "Font.serif",
        KernelFn::FontMonospace => "Font.monospace",
        KernelFn::FontHoverColor => "Font.hoverColor",
        KernelFn::FontFocusColor => "Font.focusColor",
        KernelFn::FontActiveColor => "Font.activeColor",
        KernelFn::FontDisabledColor => "Font.disabledColor",
        KernelFn::FontHoverSize => "Font.hoverSize",
        KernelFn::HtmlAttrTabindex => "Attr.tabindex",
        KernelFn::HtmlAttrRows => "Attr.rows",
        // ── Ipe.Ui.Keyed ──────────────────────────────────────────────────────
        KernelFn::KeyedColumn => "Keyed.column",
        KernelFn::KeyedRow => "Keyed.row",
        // ── Ipe.Ui.Region ──────────────────────────────────────────────
        KernelFn::RegionMainContent => "Region.mainContent",
        KernelFn::RegionNavigation => "Region.navigation",
        KernelFn::RegionFooter => "Region.footer",
        KernelFn::RegionAside => "Region.aside",
        KernelFn::RegionHeading => "Region.heading",
        KernelFn::RegionLabel => "Region.label",
        KernelFn::RegionAnnounce => "Region.announce",
        KernelFn::RegionAnnounceUrgently => "Region.announceUrgently",
        // ── Ui.input + Ui.describe + desc* constructors ───────────────────────
        KernelFn::UiInput => "Ui.input",
        KernelFn::UiDescribe => "Ui.describe",
        KernelFn::UiDescMain => "Ui.descMain",
        KernelFn::UiDescNavigation => "Ui.descNavigation",
        KernelFn::UiDescContentInfo => "Ui.descContentInfo",
        KernelFn::UiDescComplementary => "Ui.descComplementary",
        KernelFn::UiDescLivePolite => "Ui.descLivePolite",
        KernelFn::UiDescLiveAssertive => "Ui.descLiveAssertive",
        KernelFn::UiDescHeading => "Ui.descHeading",
        KernelFn::UiDescLabel => "Ui.descLabel",
        // ── Html element builders ────────────────────────────────────────
        KernelFn::HtmlTextNode => "Html.text",
        KernelFn::HtmlRawNode => "Html.raw",
        KernelFn::HtmlNode => "Html.node",
        KernelFn::HtmlVoidNode => "Html.voidNode",
        KernelFn::HtmlDoctype => "Html.doctype",
        KernelFn::HtmlTitleNode => "Html.titleNode",
        KernelFn::HtmlToString => "Html.toString",
        KernelFn::HtmlStyleNode => "Html.styleNode",
        KernelFn::HtmlDiv => "Html.div",
        KernelFn::HtmlSpan => "Html.span",
        KernelFn::HtmlA => "Html.a",
        KernelFn::HtmlButton => "Html.button",
        KernelFn::HtmlP => "Html.p",
        KernelFn::HtmlInput => "Html.input",
        KernelFn::HtmlImg => "Html.img",
        // Ipe.Html element builders.
        KernelFn::HtmlH1 => "Html.h1",
        KernelFn::HtmlH2 => "Html.h2",
        KernelFn::HtmlH3 => "Html.h3",
        KernelFn::HtmlH4 => "Html.h4",
        KernelFn::HtmlH5 => "Html.h5",
        KernelFn::HtmlH6 => "Html.h6",
        KernelFn::HtmlNav => "Html.nav",
        KernelFn::HtmlSection => "Html.section",
        KernelFn::HtmlArticle => "Html.article",
        KernelFn::HtmlHeader => "Html.header",
        KernelFn::HtmlHeaderNode => "Html.headerNode",
        KernelFn::HtmlFooter => "Html.footer",
        KernelFn::HtmlFooterNode => "Html.footerNode",
        KernelFn::HtmlMain => "Html.main",
        KernelFn::HtmlMainNode => "Html.mainNode",
        KernelFn::HtmlAside => "Html.aside",
        KernelFn::HtmlUl => "Html.ul",
        KernelFn::HtmlOl => "Html.ol",
        KernelFn::HtmlLi => "Html.li",
        KernelFn::HtmlTable => "Html.table",
        KernelFn::HtmlThead => "Html.thead",
        KernelFn::HtmlTbody => "Html.tbody",
        KernelFn::HtmlTfoot => "Html.tfoot",
        KernelFn::HtmlTr => "Html.tr",
        KernelFn::HtmlTh => "Html.th",
        KernelFn::HtmlTd => "Html.td",
        KernelFn::HtmlTextarea => "Html.textarea",
        KernelFn::HtmlSelect => "Html.select",
        KernelFn::HtmlOption => "Html.option",
        KernelFn::HtmlLabel => "Html.label",
        KernelFn::HtmlForm => "Html.form",
        KernelFn::HtmlFieldset => "Html.fieldset",
        KernelFn::HtmlLegend => "Html.legend",
        KernelFn::HtmlPre => "Html.pre",
        KernelFn::HtmlCode => "Html.code",
        KernelFn::HtmlCodeNode => "Html.codeNode",
        KernelFn::HtmlStrong => "Html.strong",
        KernelFn::HtmlEm => "Html.em",
        KernelFn::HtmlSmall => "Html.small",
        KernelFn::HtmlBlockquote => "Html.blockquote",
        KernelFn::HtmlFigure => "Html.figure",
        KernelFn::HtmlFigcaption => "Html.figcaption",
        KernelFn::HtmlDetails => "Html.details",
        KernelFn::HtmlSummary => "Html.summary",
        KernelFn::HtmlDialog => "Html.dialog",
        KernelFn::HtmlVideo => "Html.video",
        KernelFn::HtmlAudio => "Html.audio",
        KernelFn::HtmlCanvas => "Html.canvas",
        KernelFn::HtmlIframe => "Html.iframe",
        KernelFn::HtmlProgress => "Html.progress",
        KernelFn::HtmlMeter => "Html.meter",
        KernelFn::HtmlScript => "Html.script",
        KernelFn::HtmlBody => "Html.body",
        KernelFn::HtmlTitle => "Html.title",
        KernelFn::HtmlHtmlNode => "Html.htmlNode",
        KernelFn::HtmlHeadNode => "Html.headNode",
        KernelFn::HtmlBr => "Html.br",
        KernelFn::HtmlHr => "Html.hr",
        KernelFn::HtmlMeta => "Html.meta",
        KernelFn::HtmlLink => "Html.link",
        KernelFn::HtmlLinkNode => "Html.linkNode",
        KernelFn::HtmlArea => "Html.area",
        KernelFn::HtmlBase => "Html.base",
        KernelFn::HtmlCol => "Html.col",
        KernelFn::HtmlEmbed => "Html.embed",
        KernelFn::HtmlSource => "Html.source",
        KernelFn::HtmlTrack => "Html.track",
        KernelFn::HtmlWbr => "Html.wbr",
        // Ipe.Html.Attributes builders (source-facing names).
        KernelFn::HtmlAttrClass => "Attr.class",
        KernelFn::HtmlAttrId => "Attr.id",
        KernelFn::HtmlAttrHref => "Attr.href",
        KernelFn::HtmlAttrSrc => "Attr.src",
        KernelFn::HtmlAttrAlt => "Attr.alt",
        KernelFn::HtmlAttrValue => "Attr.value",
        KernelFn::HtmlAttrName => "Attr.name",
        KernelFn::HtmlAttrPlaceholder => "Attr.placeholder",
        KernelFn::HtmlAttrType => "Attr.type_",
        KernelFn::HtmlAttrFor => "Attr.for_",
        KernelFn::HtmlAttrStyle => "Attr.style",
        KernelFn::HtmlAttrTitle => "Attr.title",
        KernelFn::HtmlAttrChecked => "Attr.checked",
        KernelFn::HtmlAttrDisabled => "Attr.disabled",
        KernelFn::HtmlAttrReadonly => "Attr.readonly",
        KernelFn::HtmlAttrRequired => "Attr.required",
        KernelFn::HtmlAttrMultiple => "Attr.multiple",
        KernelFn::HtmlAttrSelected => "Attr.selected",
        KernelFn::HtmlAttrAutofocus => "Attr.autofocus",
        KernelFn::HtmlAttrAutocomplete => "Attr.autocomplete",
        KernelFn::HtmlAttribute => "Attr.attribute",
        KernelFn::HtmlBoolAttribute => "Attr.boolAttribute",
        KernelFn::HtmlNoAttr => "Attr.noAttr",
        // event-attribute builders
        KernelFn::UiOnClick => "Ui.onClick",
        KernelFn::UiOnFocus => "Ui.onFocus",
        KernelFn::UiOnBlur => "Ui.onBlur",
        KernelFn::UiOnMouseOver => "Ui.onMouseOver",
        KernelFn::UiOnMouseOut => "Ui.onMouseOut",
        KernelFn::UiOnInput => "Ui.onInput",
        KernelFn::UiOnChange => "Ui.onChange",
        KernelFn::UiOnKeyDown => "Ui.onKeyDown",
        KernelFn::UiOnKeyUp => "Ui.onKeyUp",
        KernelFn::UiOnBool => "Ui.onBool",
        KernelFn::UiOnSubmit => "Ui.onSubmit",
        KernelFn::UiOnFile => "Ui.onFile",
        // Ipe.Html.Events builders (produce Ipe.Html.Attribute).
        KernelFn::HtmlOnClick => "Event.onClick",
        KernelFn::HtmlOnFocus => "Event.onFocus",
        KernelFn::HtmlOnBlur => "Event.onBlur",
        KernelFn::HtmlOnMouseOver => "Event.onMouseOver",
        KernelFn::HtmlOnMouseOut => "Event.onMouseOut",
        KernelFn::HtmlOnSubmit => "Event.onSubmit",
        KernelFn::HtmlOnInput => "Event.onInput",
        KernelFn::HtmlOnChange => "Event.onChange",
        KernelFn::HtmlOnKeyDown => "Event.onKeyDown",
        KernelFn::HtmlOnKeyUp => "Event.onKeyUp",
        KernelFn::HtmlOnBool => "Event.onBool",
        // ── Cli app-entry + Auth + Stream + HttpStream ─────────────────
        KernelFn::ConsoleApp => "Console.app",
        KernelFn::AuthHashPassword => "Auth.hashPassword",
        KernelFn::AuthHashPasswordCost => "Auth.hashPasswordCost",
        KernelFn::AuthVerifyPassword => "Auth.verifyPassword",
        KernelFn::AuthPasswordStrength => "Auth.passwordStrength",
        KernelFn::AuthSignToken => "Auth.signToken",
        KernelFn::AuthVerifyToken => "Auth.verifyToken",
        KernelFn::AuthRegister => "Auth.register",
        KernelFn::AuthLogin => "Auth.login",
        KernelFn::AuthSetRole => "Auth.setRole",
        KernelFn::StreamStream => "Stream.stream",
        KernelFn::StreamEmit => "Stream.emit",
        KernelFn::StreamFinish => "Stream.finish",
        KernelFn::StreamWithContentType => "Stream.withContentType",
        KernelFn::HttpStreamOpen => "HttpStream.open",
        KernelFn::HttpStreamForEachChunk => "HttpStream.forEachChunk",
        KernelFn::HttpStreamClose => "HttpStream.close",
        KernelFn::HttpStreamChunks => "HttpStream.chunks",
        // ── Ipe.Http.Server.WebSocket (12 kernels) ─────────────────────
        KernelFn::WsDefaultCfg => "Ws.defaultCfg",
        KernelFn::WsWithOnConnect => "Ws.withOnConnect",
        KernelFn::WsWithOnMessage => "Ws.withOnMessage",
        KernelFn::WsWithOnClose => "Ws.withOnClose",
        KernelFn::WsWithOnError => "Ws.withOnError",
        KernelFn::WsWithMaxMessageBytes => "Ws.withMaxMessageBytes",
        KernelFn::WsWithOriginPatterns => "Ws.withOriginPatterns",
        KernelFn::WsUpgrade => "Ws.upgrade",
        KernelFn::WsSendToClient => "Ws.sendToClient",
        KernelFn::WsSendBinaryToClient => "Ws.sendBinaryToClient",
        KernelFn::WsBroadcast => "Ws.broadcast",
        KernelFn::WsCloseClient => "Ws.closeClient",
        // ── Ipe.WebSocket — outbound WebSocket client ─────────────
        KernelFn::WebSocketConnect => "WebSocket.connect",
        KernelFn::WebSocketConnectWith => "WebSocket.connectWith",
        KernelFn::WebSocketSend => "WebSocket.send",
        KernelFn::WebSocketSendBinary => "WebSocket.sendBinary",
        KernelFn::WebSocketClose => "WebSocket.close",
        KernelFn::WebSocketCloseWithCode => "WebSocket.closeWithCode",
        KernelFn::SubSubscribeWebSocket => "Sub.subscribeWebSocket",
        KernelFn::EnvPublic => "Env.public",
        // ── Ipe.Ui.Input ───────────────────────────────────────────────
        KernelFn::InputLabelAbove => "Input.labelAbove",
        KernelFn::InputLabelBelow => "Input.labelBelow",
        KernelFn::InputLabelLeft => "Input.labelLeft",
        KernelFn::InputLabelRight => "Input.labelRight",
        KernelFn::InputLabelHidden => "Input.labelHidden",
        KernelFn::InputPlaceholder => "Input.placeholder",
        KernelFn::InputText => "Input.text",
        KernelFn::InputMultiline => "Input.multiline",
        KernelFn::InputEmail => "Input.email",
        KernelFn::InputUsername => "Input.username",
        KernelFn::InputSearch => "Input.search",
        KernelFn::InputCurrentPassword => "Input.currentPassword",
        KernelFn::InputNewPassword => "Input.newPassword",
        KernelFn::InputCheckbox => "Input.checkbox",
        KernelFn::InputSlider => "Input.slider",
        KernelFn::InputOption => "Input.option",
        KernelFn::InputRadio => "Input.radio",
        KernelFn::InputRadioRow => "Input.radioRow",
        // ── Ipe.Ui.Lazy ────────────────────────────────────────────────
        KernelFn::LazyLazy => "Lazy.lazy",
        KernelFn::LazyLazy2 => "Lazy.lazy2",
        KernelFn::LazyLazy3 => "Lazy.lazy3",
        KernelFn::LazyLazy4 => "Lazy.lazy4",
        KernelFn::LazyLazy5 => "Lazy.lazy5",
        KernelFn::BasicsCompare => "Basics.compare",
        // ── Jwt builder API ─────────────────────────────────────
        KernelFn::JwtClaims => "Jwt.claims",
        KernelFn::JwtHs256 => "Jwt.hs256",
        KernelFn::JwtRs256 => "Jwt.rs256",
        KernelFn::JwtSubject => "Jwt.subject",
        KernelFn::JwtIssuer => "Jwt.issuer",
        KernelFn::JwtAudience => "Jwt.audience",
        KernelFn::JwtExpiresAt => "Jwt.expiresAt",
        KernelFn::JwtNotBefore => "Jwt.notBefore",
        KernelFn::JwtIssuedAt => "Jwt.issuedAt",
        KernelFn::JwtJwtId => "Jwt.jwtId",
        KernelFn::JwtWithClaim => "Jwt.withClaim",
        KernelFn::JwtEncode => "Jwt.encode",
        KernelFn::JwtDecode => "Jwt.decode",
        // ── Ui.form + Ui.nearby family ───────────────────────────────────────
        KernelFn::UiForm => "Ui.form",
        KernelFn::UiAbove => "Ui.above",
        KernelFn::UiBelow => "Ui.below",
        KernelFn::UiOnLeft => "Ui.onLeft",
        KernelFn::UiOnRight => "Ui.onRight",
        KernelFn::UiInFront => "Ui.inFront",
        KernelFn::UiBehind => "Ui.behind",
        // ── Ipe.Decimal ───────────────────────────────────────────────────────
        KernelFn::DecZero => "Decimal.zero",
        KernelFn::DecOne => "Decimal.one",
        KernelFn::DecOneHundred => "Decimal.oneHundred",
        KernelFn::DecFromString => "Decimal.fromString",
        KernelFn::DecFromInt => "Decimal.fromInt",
        KernelFn::DecFromFloat => "Decimal.fromFloat",
        KernelFn::DecFromMinor => "Decimal.fromMinor",
        KernelFn::DecToString => "Decimal.toString",
        KernelFn::DecToStringFixed => "Decimal.toStringFixed",
        KernelFn::DecToFloat => "Decimal.toFloat",
        KernelFn::DecToInt => "Decimal.toInt",
        KernelFn::DecToMinor => "Decimal.toMinor",
        KernelFn::DecAdd => "Decimal.add",
        KernelFn::DecSub => "Decimal.sub",
        KernelFn::DecMul => "Decimal.mul",
        KernelFn::DecDiv => "Decimal.div",
        KernelFn::DecMod => "Decimal.mod",
        KernelFn::DecNeg => "Decimal.neg",
        KernelFn::DecAbs => "Decimal.abs",
        KernelFn::DecFloor => "Decimal.floor",
        KernelFn::DecCeil => "Decimal.ceil",
        KernelFn::DecRound => "Decimal.round",
        KernelFn::DecRoundHalfUp => "Decimal.roundHalfUp",
        KernelFn::DecTruncate => "Decimal.truncate",
        KernelFn::DecCompare => "Decimal.compare",
        KernelFn::DecEq => "Decimal.eq",
        KernelFn::DecNeq => "Decimal.neq",
        KernelFn::DecLt => "Decimal.lt",
        KernelFn::DecLte => "Decimal.lte",
        KernelFn::DecGt => "Decimal.gt",
        KernelFn::DecGte => "Decimal.gte",
        KernelFn::DecMin => "Decimal.min",
        KernelFn::DecMax => "Decimal.max",
        KernelFn::DecIsZero => "Decimal.isZero",
        KernelFn::DecIsPositive => "Decimal.isPositive",
        KernelFn::DecIsNegative => "Decimal.isNegative",
        KernelFn::DecPercentOf => "Decimal.percentOf",
        KernelFn::DecAddPercent => "Decimal.addPercent",
        KernelFn::DecSubPercent => "Decimal.subPercent",
        KernelFn::DecFormatWith => "Decimal.formatWith",
        KernelFn::MoneyMinorUnits => "Money.minorUnits",
        KernelFn::MoneySymbol => "Money.symbol",
        KernelFn::MoneyCurrencyName => "Money.currencyName",
        KernelFn::MoneyIsKnownCurrency => "Money.isKnownCurrency",
        KernelFn::MoneyFormat => "Money.format",
        KernelFn::MoneyFormatWithCode => "Money.formatWithCode",
        KernelFn::MoneyAllocate => "Money.allocate",
        KernelFn::MoneySetRate => "Money.setRate",
        KernelFn::MoneyGetRate => "Money.getRate",
        KernelFn::MoneyHasRate => "Money.hasRate",
        KernelFn::MoneyClearRates => "Money.clearRates",
    }
}

/// Render a call target. Functions are shown by id (their name lives at the
/// declaration site); kernels by their qualified source name.
fn callee_name(callee: &Callee) -> String {
    match callee {
        Callee::Func(id) => format!("fn#{}", id.as_raw()),
        Callee::Kernel(kernel) => format!("kernel {}", kernel_name(*kernel)),
        Callee::Ffi { ident } => format!("ffi #{}", ident.as_raw()),
    }
}

/// Render a pattern, e.g. `Msg.Increment`, `Maybe.Just x`, `Maybe.Just _`.
/// Render a [`Pat`] as its source-facing name.
///
/// Depth-bounded via [`pat_name_at`] (starting at depth 0) — every call
/// site here stays the plain 2-argument form; only the recursive descent
/// inside `pat_name_at` threads the counter.
fn pat_name(interner: &Interner, pat: &Pat) -> String {
    pat_name_at(interner, pat, 0)
}

/// [`pat_name`]'s depth-tracked recursion. Past [`MAX_IR_RENDER_DEPTH`] this
/// renders [`DEPTH_LIMIT_PLACEHOLDER`] instead of recursing further —
/// total and stack-safe on a pathologically nested pattern, matching the
/// real Rust backend emitter's own bound rather than trusting the caller
/// never to hand it one.
fn pat_name_at(interner: &Interner, pat: &Pat, depth: u16) -> String {
    if depth > MAX_IR_RENDER_DEPTH {
        return DEPTH_LIMIT_PLACEHOLDER.to_owned();
    }
    let depth = depth + 1;
    match pat {
        Pat::Var(sym) => sym_name(interner, *sym),
        Pat::Wildcard => "_".to_owned(),
        Pat::Int(n) => n.to_string(),
        Pat::Bool(b) => if *b { "True" } else { "False" }.to_owned(),
        Pat::Char(c) => format!("'{c}'"),
        Pat::Str(s) => format!("{s:?}"),
        Pat::Alias(inner, name) => {
            format!(
                "{} as {}",
                pat_name_at(interner, inner, depth),
                sym_name(interner, *name)
            )
        }
        Pat::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(|p| pat_name_at(interner, p, depth))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Pat::Ctor {
            ty, variant, args, ..
        } => {
            let head = format!(
                "{}.{}",
                sym_name(interner, *ty),
                sym_name(interner, *variant)
            );
            if args.is_empty() {
                head
            } else {
                let subs = args
                    .iter()
                    .map(|p| pat_name_at(interner, p, depth))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{head} {subs}")
            }
        }
        Pat::Record(fields) => {
            let inner = fields
                .iter()
                .map(|(sym, p)| {
                    format!(
                        "{} = {}",
                        sym_name(interner, *sym),
                        pat_name_at(interner, p, depth)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {inner} }}")
        }
        Pat::Slice { prefix, rest } => {
            let parts = prefix
                .iter()
                .map(|p| pat_name_at(interner, p, depth))
                .collect::<Vec<_>>()
                .join(", ");
            rest.as_ref().map_or_else(
                || format!("[{parts}]"),
                |r| format!("[{parts}, {} @ ..]", pat_name_at(interner, r, depth)),
            )
        }
    }
}

fn write_module(out: &mut String, module: &Module, interner: &Interner) {
    line(
        out,
        1,
        &format!("module {}", mod_path_name(interner, &module.name)),
    );
    for ty in &module.types {
        write_type(out, ty, interner);
    }
    for func in &module.funcs {
        write_func(out, func, interner);
    }
    if let Some(entry) = module.entry {
        let name = module.funcs.iter().find(|f| f.id == entry).map_or_else(
            || format!("fn#{}", entry.as_raw()),
            |f| sym_name(interner, f.name),
        );
        line(out, 2, &format!("entry {name}"));
    }
}

fn write_type(out: &mut String, ty: &TypeDef, interner: &Interner) {
    match ty {
        TypeDef::Enum(EnumDef {
            name,
            type_params,
            variants,
            ..
        }) => {
            let rendered = variants
                .iter()
                .map(|v| variant_name(interner, v))
                .collect::<Vec<_>>()
                .join(" | ");
            // A generic enum shows its quantified type variables after the name
            // (`Maybe a`); a non-generic enum shows nothing, so existing output
            // is unchanged.
            let gens = if type_params.is_empty() {
                String::new()
            } else {
                let vars = type_params
                    .iter()
                    .map(|sym| sym_name(interner, *sym))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(" {vars}")
            };
            line(
                out,
                2,
                &format!("type {}{gens} = {rendered}", sym_name(interner, *name)),
            );
        }
    }
}

/// Render one enum variant in source-like form: `Increment`, `Just a`,
/// `Rect Float Float`.
fn variant_name(interner: &Interner, v: &Variant) -> String {
    let head = sym_name(interner, v.name);
    if v.fields.is_empty() {
        head
    } else {
        let fields = v
            .fields
            .iter()
            .map(|t| ir_type_name(interner, t))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{head} {fields}")
    }
}

/// The debug-text suffix for one type parameter's bounds: empty for an
/// unbounded variable, or `: Add+Sub+…` listing each set flag in a
/// fixed order. This is the IR's human-readable dump, not the Rust emission —
/// the backend renders the real `::core::ops::*` / `PartialOrd` spellings.
fn bound_suffix(bounds: BoundSet) -> String {
    if bounds.is_unbounded() {
        return String::new();
    }
    let mut parts = Vec::new();
    if bounds.has_static() {
        parts.push("'static");
    }
    if bounds.has_add() {
        parts.push("Add");
    }
    if bounds.has_sub() {
        parts.push("Sub");
    }
    if bounds.has_mul() {
        parts.push("Mul");
    }
    if bounds.has_ord() {
        parts.push("Ord");
    }
    if bounds.has_eq() {
        parts.push("Eq");
    }
    if bounds.has_ord_total() {
        parts.push("OrdTotal");
    }
    if bounds.has_hash() {
        parts.push("Hash");
    }
    if bounds.has_copy() {
        parts.push("Copy");
    }
    if bounds.has_clone() {
        parts.push("Clone");
    }
    if bounds.has_show() {
        parts.push("Stringify");
    }
    format!(": {}", parts.join("+"))
}

fn write_func(out: &mut String, func: &Func, interner: &Interner) {
    let params = func
        .params
        .iter()
        .map(|(sym, ty)| {
            format!(
                "{} : {}",
                sym_name(interner, *sym),
                ir_type_name(interner, ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    // A fully-parametric function shows its quantified type variables as
    // `<a, b>` after the name; a monomorphic function shows nothing, so
    // existing (empty `type_params`) output is unchanged.
    let generics = if func.type_params.is_empty() {
        String::new()
    } else {
        let vars = func
            .type_params
            .iter()
            .map(|(sym, bounds)| {
                let name = sym_name(interner, *sym);
                let suffix = bound_suffix(*bounds);
                format!("{name}{suffix}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{vars}>")
    };
    line(
        out,
        2,
        &format!(
            "fn#{} {}{generics}({params}) -> {}",
            func.id.as_raw(),
            sym_name(interner, func.name),
            ir_type_name(interner, &func.ret)
        ),
    );
    write_expr(out, &func.body, interner, 3);
}

/// Render a `let`-like binding node (`Let` / `Destructure`): a `header` line,
/// then the `value` and `body` sub-trees under labelled children. Shared by both
/// binding forms so each match arm stays a single call.
#[allow(clippy::too_many_arguments)] // depth threads alongside level for the stack-safety guard
fn write_binding(
    out: &mut String,
    header: &str,
    value: &Expr,
    body: &Expr,
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, header);
    line(out, level + 1, "value");
    write_expr_at(out, value, interner, level + 2, depth);
    line(out, level + 1, "body");
    write_expr_at(out, body, interner, level + 2, depth);
}

/// Render an [`Expr`] as an indented tree.
///
/// Depth-bounded via [`write_expr_at`] (starting at depth 0) — the single
/// external call site (`write_func`) stays the plain 4-argument form; only
/// the mutually recursive family below (`write_expr_at` and its dispatch
/// siblings) threads the counter.
fn write_expr(out: &mut String, expr: &Expr, interner: &Interner, level: usize) {
    write_expr_at(out, expr, interner, level, 0);
}

/// [`write_expr`]'s depth-tracked recursion, mutually recursive with
/// `write_binding` / `write_tail_loop` / `write_task_seq` /
/// `write_task_seq_sync` / `write_fields` / `write_list` / `write_cons` /
/// `write_record` / `write_update` / `write_lambda` / `write_apply` /
/// `write_match` — every one of those is reached ONLY from within this
/// match, so `depth` incremented once here and threaded through covers the
/// whole family. Past [`MAX_IR_RENDER_DEPTH`] this renders
/// [`DEPTH_LIMIT_PLACEHOLDER`] instead of recursing further — total and
/// stack-safe on a pathologically nested expression, matching the real Rust
/// backend emitter's own bound (IPE-L0200) rather than trusting the caller
/// never to hand `--emit-ir` one.
#[allow(clippy::too_many_lines)] // an exhaustive per-variant walker; splitting it obscures the 1:1 map
fn write_expr_at(out: &mut String, expr: &Expr, interner: &Interner, level: usize, depth: u16) {
    if depth > MAX_IR_RENDER_DEPTH {
        line(out, level, DEPTH_LIMIT_PLACEHOLDER);
        return;
    }
    let depth = depth + 1;
    match expr {
        Expr::Int(n) => line(out, level, &format!("Int {n}")),
        Expr::Bool(b) => line(out, level, &format!("Bool {b}")),
        Expr::Float(f) => line(out, level, &format!("Float {f}")),
        Expr::Str(s) => line(out, level, &format!("Str {s:?}")),
        Expr::Char(c) => line(out, level, &format!("Char '{c}'")),
        Expr::Unit => line(out, level, "Unit"),
        Expr::Var(sym) => line(out, level, &format!("Var {}", sym_name(interner, *sym))),
        Expr::CloneVar(sym) => line(
            out,
            level,
            &format!("CloneVar {}", sym_name(interner, *sym)),
        ),
        Expr::Ctor {
            ty, variant, args, ..
        } => {
            line(
                out,
                level,
                &format!(
                    "Ctor {}.{}",
                    sym_name(interner, *ty),
                    sym_name(interner, *variant)
                ),
            );
            for arg in args {
                write_expr_at(out, arg, interner, level + 1, depth);
            }
        }
        Expr::BinOp { op, lhs, rhs } => {
            line(out, level, &format!("BinOp {}", binop_token(*op)));
            write_expr_at(out, lhs, interner, level + 1, depth);
            write_expr_at(out, rhs, interner, level + 1, depth);
        }
        Expr::Let { name, value, body } => {
            write_binding(
                out,
                &format!("Let {}", sym_name(interner, *name)),
                value,
                body,
                interner,
                level,
                depth,
            );
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            write_binding(
                out,
                &format!("Destructure {}", pat_name(interner, binder)),
                value,
                body,
                interner,
                level,
                depth,
            );
        }
        Expr::If { cond, then_, else_ } => {
            line(out, level, "If");
            line(out, level + 1, "cond");
            write_expr_at(out, cond, interner, level + 2, depth);
            line(out, level + 1, "then");
            write_expr_at(out, then_, interner, level + 2, depth);
            line(out, level + 1, "else");
            write_expr_at(out, else_, interner, level + 2, depth);
        }
        Expr::Match(m) => write_match(out, m, interner, level, depth),
        Expr::Call { callee, args, .. } => {
            line(out, level, &format!("Call {}", callee_name(callee)));
            for arg in args {
                write_expr_at(out, arg, interner, level + 1, depth);
            }
        }
        Expr::Tuple(elems) => {
            line(out, level, "Tuple");
            for elem in elems {
                write_expr_at(out, elem, interner, level + 1, depth);
            }
        }
        Expr::List { elem, items } => write_list(out, elem, items, interner, level, depth),
        Expr::Cons { head, tail } => write_cons(out, head, tail, interner, level, depth),
        Expr::ListIndexClone { list, index } => {
            line(out, level, &format!("ListIndexClone [{index}]"));
            write_expr_at(out, list, interner, level + 1, depth);
        }
        Expr::ListLenCheck { list, len, exact } => {
            let op = if *exact { "==" } else { ">=" };
            line(out, level, &format!("ListLenCheck len {op} {len}"));
            write_expr_at(out, list, interner, level + 1, depth);
        }
        Expr::Record(fields) => write_record(out, fields, interner, level, depth),
        Expr::Access {
            record,
            field,
            field_ty: _,
        } => {
            line(
                out,
                level,
                &format!("Access .{}", sym_name(interner, *field)),
            );
            write_expr_at(out, record, interner, level + 1, depth);
        }
        Expr::Update { record, fields } => {
            write_update(out, record, fields, interner, level, depth);
        }
        Expr::Lambda { params, ret, body } => {
            write_lambda(out, "Lambda", params, ret, body, interner, level, depth);
        }
        Expr::SharedLambda { params, ret, body } => {
            write_lambda(
                out,
                "SharedLambda",
                params,
                ret,
                body,
                interner,
                level,
                depth,
            );
        }
        Expr::Apply { func, args } => write_apply(out, func, args, interner, level, depth),
        Expr::FuncValue { callee, ty } => line(
            out,
            level,
            &format!(
                "FuncValue {} : {}",
                callee_name(callee),
                ir_type_name(interner, ty)
            ),
        ),
        Expr::TaskSeq { effect, rest } => {
            write_task_seq(out, effect, rest, interner, level, depth);
        }
        Expr::TaskSeqSync { effect, rest } => {
            write_task_seq_sync(out, effect, rest, interner, level, depth);
        }
        Expr::TailLoop { params, body } => {
            write_tail_loop(out, params, body, interner, level, depth);
        }
        Expr::TailRecur { args } => {
            line(out, level, "TailRecur");
            for arg in args {
                write_expr_at(out, arg, interner, level + 1, depth);
            }
        }
    }
}

/// Render a [`Expr::TailLoop`] node: a header line, one `param name : ty` line
/// per loop parameter, then the `body:` sub-tree. Extracted from [`write_expr`]
/// to stay within the 100-line limit clippy enforces (sibling of
/// [`write_task_seq`]).
fn write_tail_loop(
    out: &mut String,
    params: &[(Symbol, IrType)],
    body: &Expr,
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, "TailLoop");
    for (name, ty) in params {
        line(
            out,
            level + 1,
            &format!(
                "param {} : {}",
                sym_name(interner, *name),
                ir_type_name(interner, ty)
            ),
        );
    }
    line(out, level + 1, "body");
    write_expr_at(out, body, interner, level + 2, depth);
}

/// Render a [`Expr::TaskSeq`] node: a header line followed by `effect:` and
/// `rest:` child sub-trees. Extracted from [`write_expr`] to stay within the
/// 100-line limit clippy enforces.
fn write_task_seq(
    out: &mut String,
    effect: &Expr,
    rest: &Expr,
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, "TaskSeq");
    line(out, level + 1, "effect:");
    write_expr_at(out, effect, interner, level + 2, depth);
    line(out, level + 1, "rest:");
    write_expr_at(out, rest, interner, level + 2, depth);
}

/// Render a [`Expr::TaskSeqSync`] node: identical layout to [`write_task_seq`]
/// but labelled `TaskSeqSync` to distinguish it in IR dumps.
fn write_task_seq_sync(
    out: &mut String,
    effect: &Expr,
    rest: &Expr,
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, "TaskSeqSync");
    line(out, level + 1, "effect:");
    write_expr_at(out, effect, interner, level + 2, depth);
    line(out, level + 1, "rest:");
    write_expr_at(out, rest, interner, level + 2, depth);
}

/// Render the labelled `field <name>` / value child lines of a record literal /
/// update. Shared by [`write_record`] and [`write_update`].
fn write_fields(
    out: &mut String,
    fields: &[(Symbol, Expr)],
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    for (name, value) in fields {
        line(out, level, &format!("field {}", sym_name(interner, *name)));
        write_expr_at(out, value, interner, level + 1, depth);
    }
}

/// Render a `Record` literal node. Split from [`write_expr`] to keep that match
/// small.
/// Render a list literal node: a `List : <elem>` header line followed by each
/// element expression one level deeper.
fn write_list(
    out: &mut String,
    elem: &IrType,
    items: &[Expr],
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(
        out,
        level,
        &format!("List : {}", ir_type_name(interner, elem)),
    );
    for item in items {
        write_expr_at(out, item, interner, level + 1, depth);
    }
}

/// Render a cons node: a `Cons` header line followed by the head then the tail,
/// each one level deeper.
fn write_cons(
    out: &mut String,
    head: &Expr,
    tail: &Expr,
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, "Cons");
    write_expr_at(out, head, interner, level + 1, depth);
    write_expr_at(out, tail, interner, level + 1, depth);
}

fn write_record(
    out: &mut String,
    fields: &[(Symbol, Expr)],
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, "Record");
    write_fields(out, fields, interner, level + 1, depth);
}

/// Render an `Update` node: the copied `record` then the changed fields. Split
/// from [`write_expr`] to keep that match small.
fn write_update(
    out: &mut String,
    record: &Expr,
    fields: &[(Symbol, Expr)],
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, "Update");
    line(out, level + 1, "record");
    write_expr_at(out, record, interner, level + 2, depth);
    write_fields(out, fields, interner, level + 1, depth);
}

/// Render a `Lambda` node: a header `Lambda (p0 : T0, ...) -> R` followed by its
/// body one level deeper. Split from [`write_expr`] to keep that match small.
#[allow(clippy::too_many_arguments)] // depth threads alongside level for the stack-safety guard
fn write_lambda(
    out: &mut String,
    label: &str,
    params: &[(Symbol, IrType)],
    ret: &IrType,
    body: &Expr,
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    let rendered = params
        .iter()
        .map(|(sym, ty)| {
            format!(
                "{} : {}",
                sym_name(interner, *sym),
                ir_type_name(interner, ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    line(
        out,
        level,
        &format!("{label} ({rendered}) -> {}", ir_type_name(interner, ret)),
    );
    write_expr_at(out, body, interner, level + 1, depth);
}

/// Render an `Apply` node: a `func` sub-tree then one `arg` sub-tree per
/// argument. Split from [`write_expr`] to keep that match small.
fn write_apply(
    out: &mut String,
    func: &Expr,
    args: &[Expr],
    interner: &Interner,
    level: usize,
    depth: u16,
) {
    line(out, level, "Apply");
    line(out, level + 1, "func");
    write_expr_at(out, func, interner, level + 2, depth);
    for arg in args {
        line(out, level + 1, "arg");
        write_expr_at(out, arg, interner, level + 2, depth);
    }
}

fn write_match(out: &mut String, m: &Match, interner: &Interner, level: usize, depth: u16) {
    line(out, level, "Match");
    line(out, level + 1, "scrutinee");
    write_expr_at(out, m.scrutinee(), interner, level + 2, depth);
    for Arm { pat, body, guard } in m.arms() {
        line(out, level + 1, &format!("arm {}", pat_name(interner, pat)));
        if let Some(g) = guard {
            line(out, level + 2, "guard");
            write_expr_at(out, g, interner, level + 3, depth);
        }
        write_expr_at(out, body, interner, level + 2, depth);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CallPin, FuncId, OnFormKind};
    use ipe_diagnostics::DResult;

    /// Build the canonical program: a `Main` module with a `Msg` enum and a
    /// `main` function whose body is `Log.println (String.fromInt 1)`, plus a
    /// `tick` function with a `Match` over `Msg`.
    #[allow(clippy::too_many_lines)]
    fn m0_program(i: &mut Interner) -> DResult<Program> {
        let main_mod = i.intern("Main")?;
        let msg = i.intern("Msg")?;
        let inc = i.intern("Increment")?;
        let dec = i.intern("Decrement")?;
        let main_sym = i.intern("main")?;
        let tick = i.intern("tick")?;
        let count = i.intern("count")?;
        let m = i.intern("m")?;

        let main_func = Func {
            id: FuncId::from_raw(0),
            name: main_sym,
            home: ModPath(vec![]),
            type_params: vec![],
            params: vec![],
            ret: IrType::Task(Box::new(IrType::Unit)),
            body: Expr::Call {
                callee: Callee::Kernel(KernelFn::LogPrintln),
                args: vec![Expr::Call {
                    callee: Callee::Kernel(KernelFn::StringFromInt),
                    args: vec![Expr::Int(1)],
                    pin: CallPin::None,
                    on_form: OnFormKind::NotForm,
                }],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            },
        };

        let tick_arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: msg,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: msg,
                    variant: dec,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
                guard: None,
            },
        ];
        let tick_func = Func {
            id: FuncId::from_raw(1),
            name: tick,
            home: ModPath(vec![]),
            type_params: vec![],
            params: vec![
                (
                    m,
                    IrType::Enum {
                        home: ModPath(vec![]),
                        name: msg,
                        args: vec![],
                    },
                ),
                (count, IrType::Int),
            ],
            ret: IrType::Int,
            body: Expr::Match(Match::new(Expr::Var(m), tick_arms, &[inc, dec])?),
        };

        Ok(Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    home: ModPath(vec![]),
                    name: msg,
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: inc,
                            fields: vec![],
                        },
                        Variant {
                            name: dec,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![main_func, tick_func],
                entry: Some(FuncId::from_raw(0)),
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        })
    }

    #[test]
    fn pretty_renders_m0_program() -> DResult<()> {
        let mut i = Interner::new();
        let program = m0_program(&mut i)?;
        let rendered = pretty(&program, &i);

        let expected = "\
program
  module Main
    type Msg = Increment | Decrement
    fn#0 main() -> Task Error ()
      Call kernel Log.println
        Call kernel String.fromInt
          Int 1
    fn#1 tick(m : Msg, count : Int) -> Int
      Match
        scrutinee
          Var m
        arm Msg.Increment
          BinOp +
            Var count
            Int 1
        arm Msg.Decrement
          BinOp -
            Var count
            Int 1
    entry main
";
        assert_eq!(rendered, expected);
        Ok(())
    }

    #[test]
    fn pretty_is_deterministic() -> DResult<()> {
        let mut i = Interner::new();
        let program = m0_program(&mut i)?;
        assert_eq!(pretty(&program, &i), pretty(&program, &i));
        Ok(())
    }

    #[test]
    fn pretty_renders_let_if_and_extended_binops() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("f")?;
        let n = i.intern("n")?;
        let x = i.intern("x")?;

        // f(n : Int) -> Int = let x = n * 2 in if x >= 10 then x / 2 else x + 1
        let body = Expr::Let {
            name: x,
            value: Box::new(Expr::BinOp {
                op: BinOp::Mul,
                lhs: Box::new(Expr::Var(n)),
                rhs: Box::new(Expr::Int(2)),
            }),
            body: Box::new(Expr::If {
                cond: Box::new(Expr::BinOp {
                    op: BinOp::Ge,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(10)),
                }),
                then_: Box::new(Expr::BinOp {
                    op: BinOp::Div,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(2)),
                }),
                else_: Box::new(Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(1)),
                }),
            }),
        };
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![(n, IrType::Int)],
                    ret: IrType::Int,
                    body,
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    fn#0 f(n : Int) -> Int
      Let x
        value
          BinOp *
            Var n
            Int 2
        body
          If
            cond
              BinOp >=
                Var x
                Int 10
            then
              BinOp /
                Var x
                Int 2
            else
              BinOp +
                Var x
                Int 1
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_tuple_expr_and_type() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("pair")?;
        let n = i.intern("n")?;

        // pair(n : Int) -> (Int, Bool) = (n, n)  (shape only; types illustrative)
        let body = Expr::Tuple(vec![Expr::Var(n), Expr::Int(1)]);
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![(n, IrType::Int)],
                    ret: IrType::Tuple(vec![IrType::Int, IrType::Bool]),
                    body,
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    fn#0 pair(n : Int) -> (Int, Bool)
      Tuple
        Var n
        Int 1
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_record_expr_access_update_and_type() -> DResult<()> {
        use std::collections::BTreeMap;

        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let func = i.intern("move_")?;
        let param = i.intern("p")?;
        let x = i.intern("x")?;
        let y = i.intern("y")?;

        // move_(p : { x : Int, y : Int }) -> { x : Int, y : Int }
        //   = { p | x = p.x }   (shape only; values illustrative)
        let body = Expr::Update {
            record: Box::new(Expr::Var(param)),
            fields: vec![(
                x,
                Expr::Access {
                    record: Box::new(Expr::Var(param)),
                    field: x,
                    field_ty: IrType::Int,
                },
            )],
        };
        let mut rec_fields = BTreeMap::new();
        rec_fields.insert(x, IrType::Int);
        rec_fields.insert(y, IrType::Int);
        let rec_ty = IrType::Record(rec_fields);
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: func,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![(param, rec_ty.clone())],
                    ret: rec_ty,
                    body,
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    fn#0 move_(p : { x : Int, y : Int }) -> { x : Int, y : Int }
      Update
        record
          Var p
        field x
          Access .x
            Var p
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_record_literal() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("origin")?;
        let x = i.intern("x")?;
        let y = i.intern("y")?;

        // origin() -> ... = { x = 1, y = 2 }
        let body = Expr::Record(vec![(x, Expr::Int(1)), (y, Expr::Int(2))]);
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![],
                    ret: IrType::Int,
                    body,
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    fn#0 origin() -> Int
      Record
        field x
          Int 1
        field y
          Int 2
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_lambda_apply_and_fun_type() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("apply2")?;
        let g = i.intern("g")?;
        let x = i.intern("x")?;

        // apply2(g : Int -> Int) -> Int = (\x -> g x) 2
        let body = Expr::Apply {
            func: Box::new(Expr::Lambda {
                params: vec![(x, IrType::Int)],
                ret: IrType::Int,
                body: Box::new(Expr::Apply {
                    func: Box::new(Expr::Var(g)),
                    args: vec![Expr::Var(x)],
                }),
            }),
            args: vec![Expr::Int(2)],
        };
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![(g, IrType::Fun(vec![IrType::Int], Box::new(IrType::Int)))],
                    ret: IrType::Int,
                    body,
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    fn#0 apply2(g : Int -> Int) -> Int
      Apply
        func
          Lambda (x : Int) -> Int
            Apply
              func
                Var g
              arg
                Var x
        arg
          Int 2
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_nullary_fun_type() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("thunk")?;

        // thunk(k : () -> Bool) -> Bool = ...  (body shape illustrative)
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![(i.intern("k")?, IrType::Fun(vec![], Box::new(IrType::Bool)))],
                    ret: IrType::Bool,
                    body: Expr::Int(0),
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    fn#0 thunk(k : () -> Bool) -> Bool
      Int 0
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_generic_adt_decl_ctor_and_pattern() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let a = i.intern("a")?;
        let maybe = i.intern("Maybe")?;
        let just = i.intern("Just")?;
        let nothing = i.intern("Nothing")?;
        let unwrap = i.intern("unwrap")?;
        let m = i.intern("m")?;
        let x = i.intern("x")?;

        // type Maybe a = Just a | Nothing
        // unwrap(m : Maybe Int) -> Int =
        //   case m of Just x -> x ; Nothing -> 0
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: maybe,
                    variant: just,
                    args: vec![Pat::Var(x)],
                },
                body: Expr::Var(x),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: maybe,
                    variant: nothing,
                    args: vec![],
                },
                body: Expr::Int(0),
                guard: None,
            },
        ];
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    home: ModPath(vec![]),
                    name: maybe,
                    type_params: vec![a],
                    variants: vec![
                        Variant {
                            name: just,
                            fields: vec![IrType::Generic(a)],
                        },
                        Variant {
                            name: nothing,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: unwrap,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![(
                        m,
                        IrType::Enum {
                            home: ModPath(vec![]),
                            name: maybe,
                            args: vec![IrType::Int],
                        },
                    )],
                    ret: IrType::Int,
                    body: Expr::Match(Match::new(Expr::Var(m), arms, &[just, nothing])?),
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    type Maybe a = Just a | Nothing
    fn#0 unwrap(m : Maybe Int) -> Int
      Match
        scrutinee
          Var m
        arm Maybe.Just x
          Var x
        arm Maybe.Nothing
          Int 0
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_tuple_pattern_and_unit() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let wrap = i.intern("Wrap")?;
        let mk_wrap = i.intern("MkWrap")?;
        let fst_of = i.intern("fstOf")?;
        let w = i.intern("w")?;
        let a = i.intern("a")?;
        let b = i.intern("b")?;
        let nop = i.intern("nop")?;

        // type Wrap = MkWrap (Int, Int)
        // fstOf(w : Wrap) -> Int = case w of MkWrap (a, b) -> a
        // nop() -> () = ()
        let arms = vec![Arm {
            pat: Pat::Ctor {
                home: ModPath(vec![]),
                ty: wrap,
                variant: mk_wrap,
                args: vec![Pat::Tuple(vec![Pat::Var(a), Pat::Var(b)])],
            },
            body: Expr::Var(a),
            guard: None,
        }];
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    home: ModPath(vec![]),
                    name: wrap,
                    type_params: vec![],
                    variants: vec![Variant {
                        name: mk_wrap,
                        fields: vec![IrType::Tuple(vec![IrType::Int, IrType::Int])],
                    }],
                })],
                funcs: vec![
                    Func {
                        id: FuncId::from_raw(0),
                        name: fst_of,
                        home: ModPath(vec![]),
                        type_params: vec![],
                        params: vec![(
                            w,
                            IrType::Enum {
                                home: ModPath(vec![]),
                                name: wrap,
                                args: vec![],
                            },
                        )],
                        ret: IrType::Int,
                        body: Expr::Match(Match::new(Expr::Var(w), arms, &[mk_wrap])?),
                    },
                    Func {
                        id: FuncId::from_raw(1),
                        name: nop,
                        home: ModPath(vec![]),
                        type_params: vec![],
                        params: vec![],
                        ret: IrType::Unit,
                        body: Expr::Unit,
                    },
                ],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };

        let expected = "\
program
  module Main
    type Wrap = MkWrap (Int, Int)
    fn#0 fstOf(w : Wrap) -> Int
      Match
        scrutinee
          Var w
        arm Wrap.MkWrap (a, b)
          Var a
    fn#1 nop() -> ()
      Unit
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_resolves_forged_symbol_to_placeholder() {
        let i = Interner::new();
        // A program referencing a symbol this interner never handed out.
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![Symbol::from_raw(999)]),
                types: vec![],
                funcs: vec![],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };
        let rendered = pretty(&program, &i);
        assert!(rendered.contains("module <sym#999>"));
    }

    /// A single-func program whose body is a `BinOp` chain nested `levels`
    /// deep: `(((...(0 + 1) + 1)...) + 1)`.
    fn deeply_nested_binop_program(i: &mut Interner, levels: u32) -> DResult<Program> {
        let main_mod = i.intern("Main")?;
        let deep = i.intern("deep")?;
        let mut body = Expr::Int(0);
        for _ in 0..levels {
            body = Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(body),
                rhs: Box::new(Expr::Int(1)),
            };
        }
        Ok(Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: deep,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![],
                    ret: IrType::Int,
                    body,
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_ffi: false,
                uses_env_public: false,
            }],
        })
    }

    /// CO-BACKEND-006: `--emit-ir` on a program nested well past the real
    /// emitter's bound (`ipe_backend_rust::MAX_EMIT_DEPTH`, IPE-L0200) must
    /// render a `<depth limit>` placeholder and return normally — never
    /// overflow the native stack — even though this dev-flag path has no
    /// other gate rejecting the program first.
    #[test]
    fn deeply_nested_expr_renders_depth_limit_placeholder_not_stack_overflow() -> DResult<()> {
        let mut i = Interner::new();
        // Far past MAX_IR_RENDER_DEPTH (96) — the same order of magnitude
        // the emitter-side hardening test uses to prove its own guard.
        let program = deeply_nested_binop_program(&mut i, 4096)?;
        let rendered = pretty(&program, &i);
        assert!(
            rendered.contains(DEPTH_LIMIT_PLACEHOLDER),
            "deep nesting must render the depth-limit placeholder:\n{rendered}"
        );
        Ok(())
    }

    /// An expression at the depth bound still renders in full — the guard is
    /// a ceiling, not an off-by-one rejection of legitimate programs.
    #[test]
    fn nesting_at_the_bound_still_renders_in_full() -> DResult<()> {
        let mut i = Interner::new();
        let program = deeply_nested_binop_program(&mut i, u32::from(MAX_IR_RENDER_DEPTH) / 2)?;
        let rendered = pretty(&program, &i);
        assert!(
            !rendered.contains(DEPTH_LIMIT_PLACEHOLDER),
            "in-bound nesting must render fully, not hit the placeholder:\n{rendered}"
        );
        // Every level renders its own `Int 0` / `Int 1` leaf.
        assert!(rendered.contains("Int 0"));
        assert!(rendered.contains("BinOp +"));
        Ok(())
    }
}
