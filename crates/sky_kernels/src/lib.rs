//! Kernel-function registry — the single closed enum covering every Sky
//! stdlib kernel.
//!
//! # DAG constraint
//!
//! `sky_kernels` is a **leaf crate**.  Its only permitted dependencies are
//! `sky_intern` and `sky_diagnostics`.  No edge to `sky_ir`, `sky_types`, or
//! `sky_backend_rust` is ever allowed; those crates import `sky_kernels` and a
//! reverse edge would create a DAG cycle.
//!
//! # Phase A
//!
//! Phase A establishes the registry infrastructure while keeping every
//! downstream crate on the legacy dual-backed path.  `sky_ir` re-exports
//! `type KernelFn = sky_kernels::StdlibKernel` so existing call-sites compile
//! unchanged.  Phase B threads `VarHome::Kernel(KernelId)` through the
//! canonicaliser, retiring the parallel `(Symbol, Symbol)` table.

#![allow(clippy::module_name_repetitions)] // KernelId / KernelClass / FfiKernelId all contain "Kernel"
#![forbid(unsafe_code)]

/// Classification of a kernel variant by which compiler / runtime subsystem
/// owns its emission.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelClass {
    /// String, Char, Math, List, Maybe, Result, Dict, Set, Bytes, Encoding,
    /// Json*, Crypto, Uuid, Jwt, Task combinators, Io, Time (non-TEA),
    /// System, Random, File, Http — everything that does not belong to a
    /// specialised subsystem.
    Pure,
    /// `Std.Db` / `Db.Decode` kernels (M5b-db / M6).
    Db,
    /// `Sky.Http.Server` / Middleware / `RateLimit` kernels (M6).
    Server,
    /// `Cmd` / `Sub` / `Time.every` TEA wiring kernels (M5c, including M6
    /// reserved pub/sub variants).
    Tea,
    /// `Std.Ui` / `Std.Html` element and attribute builders (M7).
    Ui,
    /// `Std.Live` app-entry kernels (M7).
    Live,
    /// `Std.Tui` app-entry kernels (M7).
    Tui,
    /// `Std.Webview` app-entry kernel (M7).
    Webview,
    /// Reserved for the FFI kernel tier (Phase B+).
    Ffi,
}

/// Per-variant metadata returned by [`StdlibKernel::decl`].
///
/// All fields are `'static` — the struct is `Copy` and can be embedded in
/// `const` contexts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StdlibDecl {
    /// The canonical qualifier used in the canon `QUALIFIERS` table
    /// (e.g. `"String"`, `"Math"`).
    ///
    /// Qualifiers starting with `'_'` are internal or not-yet-registered and
    /// are excluded from the canon-equality tripwire test.
    pub qualifier: &'static str,
    /// The canonical function name (e.g. `"fromInt"`, `"pi"`).
    pub name: &'static str,
    /// Sky-level arity: number of arguments before the result.
    pub arity: u8,
    /// Which subsystem owns emission of this kernel.
    pub class: KernelClass,
    /// Name of the Rust runtime symbol that implements this kernel (from
    /// `sky_backend_rust::naming::kernel_name`).
    pub emit: &'static str,
}

/// Every stdlib kernel function known to the Sky compiler.
///
/// Variant order matches `lower.rs` `lower_callee` declaration order so that
/// the discriminant values are stable across a rename cycle.
///
/// # Registry invariant
///
/// [`StdlibKernel::ALL`] is the canonical wired-variant slice.  Every variant
/// in `ALL` has a matching entry in the canon `QUALIFIERS` table (verified by
/// the `canon_equals_registry` tripwire test in `sky_canon`).  Variants
/// intentionally absent from `ALL` have their qualifier noted in the `decl()`
/// doc section below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StdlibKernel {
    // ── Log ─────────────────────────────────────────────────────────────────
    LogPrintln,
    // ── String ──────────────────────────────────────────────────────────────
    StringFromInt,
    StringFromFloat,
    StringLength,
    StringIsEmpty,
    StringReverse,
    StringToUpper,
    StringToLower,
    StringCasefold,
    StringTrim,
    StringTrimStart,
    StringTrimEnd,
    StringToInt,
    StringToFloat,
    StringFromChar,
    StringFromList,
    StringConcat,
    StringWords,
    StringLines,
    StringToList,
    StringIsEmail,
    StringIsUrl,
    StringAppend,
    StringContains,
    StringStartsWith,
    StringEndsWith,
    StringEqualFold,
    StringJoin,
    StringSplit,
    StringRepeat,
    StringDropLeft,
    StringDropRight,
    StringReplace,
    StringSlice,
    StringPadLeft,
    StringPadRight,
    // ── Char ────────────────────────────────────────────────────────────────
    CharIsAlpha,
    CharIsDigit,
    CharIsLower,
    CharIsUpper,
    CharToLower,
    CharToUpper,
    CharToCode,
    CharFromCode,
    // ── List ────────────────────────────────────────────────────────────────
    ListMap,
    ListFilter,
    ListFoldl,
    ListFoldr,
    ListLength,
    ListHead,
    ListTail,
    ListMember,
    ListRange,
    ListReverse,
    ListAppend,
    ListConcat,
    ListTake,
    ListDrop,
    ListZip,
    ListCons,
    ListIsEmpty,
    ListConcatMap,
    ListIndexedMap,
    // ── Maybe ───────────────────────────────────────────────────────────────
    MaybeWithDefault,
    MaybeMap,
    MaybeAndThen,
    // ── Result ──────────────────────────────────────────────────────────────
    ResultWithDefault,
    ResultMap,
    /// Internal: `Result.withDefault`-style defaulting used during lowering.
    /// Qualifier `"_internal_"` — not registered in the canon `QUALIFIERS`
    /// table and excluded from the tripwire test.
    ResultOkDefault,
    // ── Math ────────────────────────────────────────────────────────────────
    MathMin,
    MathMax,
    MathPi,
    MathE,
    MathPhi,
    MathSqrt2,
    MathInf,
    MathNan,
    MathAbs,
    MathSqrt,
    MathCbrt,
    MathExp,
    MathExp2,
    MathLog,
    MathLog2,
    MathLog10,
    MathSin,
    MathCos,
    MathTan,
    MathAsin,
    MathAcos,
    MathAtan,
    MathSinh,
    MathCosh,
    MathTanh,
    MathAsinh,
    MathAcosh,
    MathAtanh,
    MathFloor,
    MathCeil,
    MathRound,
    MathTrunc,
    MathPow,
    MathHypot,
    MathAtan2,
    MathMod,
    MathRemainder,
    // ── Dict ────────────────────────────────────────────────────────────────
    DictEmpty,
    DictIsEmpty,
    DictSize,
    DictKeys,
    DictValues,
    DictToList,
    DictFromList,
    DictGet,
    DictMember,
    DictRemove,
    DictUnion,
    DictMap,
    DictInsert,
    DictFoldl,
    // ── Set ─────────────────────────────────────────────────────────────────
    SetEmpty,
    SetSize,
    SetToList,
    SetFromList,
    SetMember,
    SetInsert,
    SetRemove,
    SetUnion,
    SetIntersect,
    SetDiff,
    // ── Bytes ───────────────────────────────────────────────────────────────
    BytesEmpty,
    BytesLength,
    BytesIsEmpty,
    BytesFromString,
    BytesToString,
    BytesFromHex,
    BytesToHex,
    BytesFromBase64,
    BytesToBase64,
    BytesAppend,
    BytesSlice,
    // ── Encoding ────────────────────────────────────────────────────────────
    EncodingBase64Encode,
    EncodingBase64Decode,
    EncodingUrlEncode,
    EncodingUrlDecode,
    EncodingHexEncode,
    EncodingHexDecode,
    // ── Json.Encode ─────────────────────────────────────────────────────────
    JsonEncString,
    JsonEncInt,
    JsonEncFloat,
    JsonEncBool,
    JsonEncNull,
    JsonEncList,
    JsonEncObject,
    JsonEncEncode,
    // ── Json.Decode ─────────────────────────────────────────────────────────
    JsonDecString,
    JsonDecInt,
    JsonDecFloat,
    JsonDecBool,
    JsonDecDecodeString,
    JsonDecField,
    JsonDecAt,
    JsonDecIndex,
    JsonDecList,
    JsonDecMap,
    JsonDecAndThen,
    JsonDecSucceed,
    JsonDecFail,
    JsonDecOneOf,
    JsonDecMap2,
    JsonDecMap3,
    JsonDecMap4,
    // ── Json.Decode.Pipeline ────────────────────────────────────────────────
    JsonDecPRequired,
    JsonDecPOptional,
    JsonDecPCustom,
    JsonDecPRequiredAt,
    // ── Crypto ──────────────────────────────────────────────────────────────
    CryptoSha256,
    CryptoSha512,
    CryptoSha1,
    CryptoMd5,
    CryptoHmacSha256,
    CryptoHmacSha512,
    CryptoRsaSha256Sign,
    CryptoRsaSha256Verify,
    CryptoConstantTimeEqual,
    CryptoAesGcmEncrypt,
    CryptoAesGcmDecrypt,
    CryptoChacha20Encrypt,
    CryptoChacha20Decrypt,
    CryptoAesKeyFromPassword,
    CryptoChachaKeyFromPassword,
    CryptoRandomBytes,
    CryptoRandomToken,
    // ── Uuid ────────────────────────────────────────────────────────────────
    UuidV4,
    UuidV7,
    UuidParse,
    // ── Jwt ─────────────────────────────────────────────────────────────────
    JwtEncodeHs256,
    JwtDecodeHs256,
    JwtEncodeRs256,
    JwtDecodeRs256,
    // ── Task combinators ────────────────────────────────────────────────────
    TaskSucceed,
    TaskFail,
    TaskMap,
    TaskAndThen,
    TaskMapError,
    TaskOnError,
    TaskFromResult,
    TaskAndThenResult,
    TaskSequence,
    TaskParallel,
    TaskRun,
    // ── Io ──────────────────────────────────────────────────────────────────
    IoReadLine,
    IoWriteStdout,
    IoWriteStderr,
    // ── Time (non-TEA) ──────────────────────────────────────────────────────
    TimeNow,
    TimeSleep,
    TimeUnixMillis,
    // ── System ──────────────────────────────────────────────────────────────
    SystemArgs,
    SystemGetenv,
    SystemGetenvOr,
    SystemGetArg,
    SystemGetenvInt,
    SystemGetenvBool,
    SystemSetenv,
    SystemUnsetenv,
    SystemCwd,
    SystemLoadEnv,
    SystemExit,
    // ── Random ──────────────────────────────────────────────────────────────
    RandomInt,
    RandomFloat,
    RandomChoice,
    // ── File ────────────────────────────────────────────────────────────────
    FileReadFile,
    FileWriteFile,
    FileExists,
    FileRemove,
    FileMkdirAll,
    FileReadFileLimit,
    FileReadFileBytes,
    FileAppend,
    FileReadDir,
    FileIsDir,
    FileTempFile,
    FileTempDir,
    FileCopy,
    FileRename,
    FileDelete,
    // ── Http ────────────────────────────────────────────────────────────────
    HttpGet,
    HttpPost,
    HttpRequest,
    HttpParseQuery,
    HttpDefaultRequest,
    HttpWithMethod,
    HttpWithTimeout,
    HttpWithBody,
    HttpWithHeader,
    // ── Db ──────────────────────────────────────────────────────────────────
    DbConnect,
    DbOpen,
    DbClose,
    DbExecRaw,
    DbExec,
    DbQuery,
    DbQueryDecode,
    DbGetString,
    DbGetInt,
    DbGetBool,
    DbGetField,
    DbInsertRow,
    DbGetById,
    DbUpdateById,
    DbDeleteById,
    DbFindOneByField,
    DbFindManyByField,
    DbFindByConditions,
    DbUnsafeFindWhere,
    DbInsertFields,
    DbUpdateFields,
    DbInsertFieldsReturning,
    DbWithTransaction,
    DbMigrate,
    // ── Db.Decode ───────────────────────────────────────────────────────────
    DbDecString,
    DbDecInt,
    DbDecFloat,
    DbDecBool,
    DbDecNullable,
    DbDecMap,
    DbDecAndThen,
    DbDecSucceed,
    DbDecFail,
    DbDecMap2,
    DbDecMap3,
    DbDecMap4,
    DbDecRequired,
    DbDecOptional,
    // ── TEA: Cmd / Sub / Time.every (wired M5c) ─────────────────────────────
    CmdNone,
    CmdBatch,
    CmdPerform,
    SubNone,
    SubBatch,
    SubEvery,
    TimeEvery,
    // ── TEA: pub/sub M6 reserved ────────────────────────────────────────────
    /// `Cmd.publish` — reserved for M6; absent from [`Self::ALL`] until wired
    /// in the canon `QUALIFIERS` table.
    CmdPublish,
    /// `Cmd.publishNoEcho` — reserved for M6; absent from [`Self::ALL`].
    CmdPublishNoEcho,
    /// `Sub.subscribeTopic` — reserved for M6; absent from [`Self::ALL`].
    SubSubscribeTopic,
    /// `PubSub.publish` — reserved for M6; absent from [`Self::ALL`] until the
    /// `"PubSub"` qualifier is added to the canon `QUALIFIERS` table.
    PubSubPublish,
    /// `PubSub.publishNoEcho` — reserved for M6; absent from [`Self::ALL`].
    PubSubPublishNoEcho,
    // ── Sky.Http.Server / Middleware / RateLimit ─────────────────────────────
    ServerGet,
    ServerPost,
    ServerPut,
    ServerDelete,
    ServerAny,
    ServerApi,
    ServerStatic,
    ServerListen,
    ServerText,
    ServerJson,
    ServerHtml,
    ServerWithStatus,
    ServerWithHeader,
    ServerRedirect,
    ServerParam,
    ServerQueryParam,
    ServerHeader,
    ServerGetCookie,
    ServerBody,
    ServerPath,
    ServerMethod,
    ServerCookieNew,
    ServerWithCookie,
    MiddlewareWithCors,
    MiddlewareWithLogging,
    MiddlewareWithBasicAuth,
    MiddlewareWithRateLimit,
    RateLimitAllow,
    // ── M7: Std.Ui / Std.Html render kernels ─────────────────────────────────
    UiLayout,
    UiLayoutWith,
    HtmlRender,
    HtmlEscapeText,
    HtmlEscapeAttr,
    HtmlAttrToString,
    // ── M7: Std.Ui element builders ──────────────────────────────────────────
    UiNone,
    UiText,
    UiHtml,
    UiEl,
    UiRow,
    UiColumn,
    UiWrappedRow,
    UiGrid,
    // ── M7: Std.Ui attribute builders ────────────────────────────────────────
    UiSpacing,
    UiPadding,
    UiPaddingXY,
    UiWidth,
    UiHeight,
    UiCenterX,
    UiCenterY,
    UiAlignLeft,
    UiAlignRight,
    UiAlignTop,
    UiAlignBottom,
    UiPointer,
    UiClip,
    UiScrollbars,
    UiGridColumns,
    // ── M7: Std.Ui Length builders ───────────────────────────────────────────
    UiPx,
    UiFill,
    UiContent,
    UiShrink,
    UiFillPortion,
    UiVh,
    UiVw,
    UiMinimum,
    UiMaximum,
    // ── M7: Std.Ui Color builders ────────────────────────────────────────────
    UiRgb,
    UiRgba,
    UiWhite,
    UiBlack,
    UiTransparent,
    // ── M7: Background / Border / Font sub-modules ───────────────────────────
    BackgroundColor,
    BackgroundImage,
    BorderWidth,
    BorderRounded,
    BorderColor,
    FontSize,
    FontColor,
    FontFamily,
    FontBold,
    FontItalic,
    // ── M7: Html element builders ────────────────────────────────────────────
    HtmlTextNode,
    HtmlRawNode,
    HtmlNode,
    /// `Html.styleNode : List Attr -> String -> Html msg` — arity-2, NOT the
    /// arity-3 `HtmlNode` it was previously mis-folded into. Its dedicated
    /// runtime kernel `html_style_node_` close-tag-neutralises the CSS body at
    /// construction (F7).
    HtmlStyleNode,
    HtmlDiv,
    HtmlSpan,
    HtmlA,
    HtmlButton,
    HtmlP,
    HtmlInput,
    HtmlImg,
    // ── M7: Std.Live app-entry kernels ───────────────────────────────────────
    LiveApp,
    LiveAppRouted,
    LiveRoute,
    LiveRenderStatic,
    // ── M7: Std.Tui app-entry kernels ────────────────────────────────────────
    TuiProgram,
    TuiApp,
    // ── M7: Std.Webview app-entry kernel ─────────────────────────────────────
    WebviewApp,
    // ── M7: event-attribute builders (Phase-1a) ──────────────────────────────
    UiOnClick,
    UiOnFocus,
    UiOnBlur,
    UiOnMouseOver,
    UiOnMouseOut,
    UiOnInput,
    UiOnChange,
    UiOnKeyDown,
    UiOnKeyUp,
    UiOnBool,
}

impl StdlibKernel {
    /// Canonical metadata for this kernel variant.
    ///
    /// The returned [`StdlibDecl`] is `'static` and `Copy` — safe to embed in
    /// `const` contexts.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub const fn decl(self) -> StdlibDecl {
        // Shorthand constructor to keep each arm concise.
        const fn d(
            qualifier: &'static str,
            name: &'static str,
            arity: u8,
            class: KernelClass,
            emit: &'static str,
        ) -> StdlibDecl {
            StdlibDecl {
                qualifier,
                name,
                arity,
                class,
                emit,
            }
        }
        use KernelClass::{Db, Live, Pure, Server, Tea, Tui, Ui, Webview};
        match self {
            // ── Log ─────────────────────────────────────────────────────────
            // Qualifier "Log" is installed via `install_builtin_vars` as an
            // unqualified name; it is NOT in the canon `QUALIFIERS` table.
            // The tripwire test skips it because "Log" is absent from
            // `env.qual_vars`.
            Self::LogPrintln => d("Log", "println", 1, Pure, "log_println"),
            // ── String ──────────────────────────────────────────────────────
            Self::StringFromInt => d("String", "fromInt", 1, Pure, "string_from_int"),
            Self::StringFromFloat => d("String", "fromFloat", 1, Pure, "string_from_float"),
            Self::StringLength => d("String", "length", 1, Pure, "string_length"),
            Self::StringIsEmpty => d("String", "isEmpty", 1, Pure, "string_is_empty"),
            Self::StringReverse => d("String", "reverse", 1, Pure, "string_reverse"),
            Self::StringToUpper => d("String", "toUpper", 1, Pure, "string_to_upper"),
            Self::StringToLower => d("String", "toLower", 1, Pure, "string_to_lower"),
            Self::StringCasefold => d("String", "casefold", 1, Pure, "string_casefold"),
            Self::StringTrim => d("String", "trim", 1, Pure, "string_trim"),
            Self::StringTrimStart => d("String", "trimStart", 1, Pure, "string_trim_start"),
            Self::StringTrimEnd => d("String", "trimEnd", 1, Pure, "string_trim_end"),
            Self::StringToInt => d("String", "toInt", 1, Pure, "string_to_int"),
            Self::StringToFloat => d("String", "toFloat", 1, Pure, "string_to_float"),
            Self::StringFromChar => d("String", "fromChar", 1, Pure, "string_from_char"),
            Self::StringFromList => d("String", "fromList", 1, Pure, "string_from_list"),
            Self::StringConcat => d("String", "concat", 1, Pure, "string_concat"),
            Self::StringWords => d("String", "words", 1, Pure, "string_words"),
            Self::StringLines => d("String", "lines", 1, Pure, "string_lines"),
            Self::StringToList => d("String", "toList", 1, Pure, "string_to_list"),
            Self::StringIsEmail => d("String", "isEmail", 1, Pure, "string_is_email"),
            Self::StringIsUrl => d("String", "isUrl", 1, Pure, "string_is_url"),
            Self::StringAppend => d("String", "append", 2, Pure, "string_append"),
            Self::StringContains => d("String", "contains", 2, Pure, "string_contains"),
            Self::StringStartsWith => d("String", "startsWith", 2, Pure, "string_starts_with"),
            Self::StringEndsWith => d("String", "endsWith", 2, Pure, "string_ends_with"),
            Self::StringEqualFold => d("String", "equalFold", 2, Pure, "string_equal_fold"),
            Self::StringJoin => d("String", "join", 2, Pure, "string_join"),
            Self::StringSplit => d("String", "split", 2, Pure, "string_split"),
            Self::StringRepeat => d("String", "repeat", 2, Pure, "string_repeat"),
            Self::StringDropLeft => d("String", "dropLeft", 2, Pure, "string_drop_left"),
            Self::StringDropRight => d("String", "dropRight", 2, Pure, "string_drop_right"),
            Self::StringReplace => d("String", "replace", 3, Pure, "string_replace"),
            Self::StringSlice => d("String", "slice", 3, Pure, "string_slice"),
            Self::StringPadLeft => d("String", "padLeft", 3, Pure, "string_pad_left"),
            Self::StringPadRight => d("String", "padRight", 3, Pure, "string_pad_right"),
            // ── Char ────────────────────────────────────────────────────────
            Self::CharIsAlpha => d("Char", "isAlpha", 1, Pure, "char_is_alpha"),
            Self::CharIsDigit => d("Char", "isDigit", 1, Pure, "char_is_digit"),
            Self::CharIsLower => d("Char", "isLower", 1, Pure, "char_is_lower"),
            Self::CharIsUpper => d("Char", "isUpper", 1, Pure, "char_is_upper"),
            Self::CharToLower => d("Char", "toLower", 1, Pure, "char_to_lower"),
            Self::CharToUpper => d("Char", "toUpper", 1, Pure, "char_to_upper"),
            Self::CharToCode => d("Char", "toCode", 1, Pure, "char_to_code"),
            Self::CharFromCode => d("Char", "fromCode", 1, Pure, "char_from_code"),
            // ── List ────────────────────────────────────────────────────────
            Self::ListMap => d("List", "map", 2, Pure, "list_map_consume"),
            Self::ListFilter => d("List", "filter", 2, Pure, "list_filter"),
            Self::ListFoldl => d("List", "foldl", 3, Pure, "list_foldl"),
            Self::ListFoldr => d("List", "foldr", 3, Pure, "list_foldr"),
            Self::ListLength => d("List", "length", 1, Pure, "list_length"),
            Self::ListHead => d("List", "head", 1, Pure, "list_head"),
            Self::ListTail => d("List", "tail", 1, Pure, "list_tail"),
            Self::ListMember => d("List", "member", 2, Pure, "list_member"),
            Self::ListRange => d("List", "range", 2, Pure, "list_range"),
            Self::ListReverse => d("List", "reverse", 1, Pure, "list_reverse"),
            Self::ListAppend => d("List", "append", 2, Pure, "list_append"),
            Self::ListConcat => d("List", "concat", 1, Pure, "list_concat"),
            Self::ListTake => d("List", "take", 2, Pure, "list_take"),
            Self::ListDrop => d("List", "drop", 2, Pure, "list_drop"),
            Self::ListZip => d("List", "zip", 2, Pure, "list_zip"),
            Self::ListCons => d("List", "cons", 2, Pure, "sky_list_cons"),
            Self::ListIsEmpty => d("List", "isEmpty", 1, Pure, "list_is_empty"),
            Self::ListConcatMap => d("List", "concatMap", 2, Pure, "list_concat_map"),
            Self::ListIndexedMap => d("List", "indexedMap", 2, Pure, "list_indexed_map"),
            // ── Maybe ───────────────────────────────────────────────────────
            Self::MaybeWithDefault => d("Maybe", "withDefault", 2, Pure, "maybe_with_default"),
            Self::MaybeMap => d("Maybe", "map", 2, Pure, "sky_maybe_map"),
            Self::MaybeAndThen => d("Maybe", "andThen", 2, Pure, "sky_maybe_and_then"),
            // ── Result ──────────────────────────────────────────────────────
            Self::ResultWithDefault => d("Result", "withDefault", 2, Pure, "result_with_default"),
            Self::ResultMap => d("Result", "map", 2, Pure, "sky_result_map"),
            // Internal: qualifier starts with '_' → skipped by tripwire test.
            Self::ResultOkDefault => d("_internal_", "okDefault", 1, Pure, "ok_res"),
            // ── Math ────────────────────────────────────────────────────────
            Self::MathMin => d("Math", "min", 2, Pure, "math_min"),
            Self::MathMax => d("Math", "max", 2, Pure, "math_max"),
            Self::MathPi => d("Math", "pi", 0, Pure, "math_pi"),
            Self::MathE => d("Math", "e", 0, Pure, "math_e"),
            Self::MathPhi => d("Math", "phi", 0, Pure, "math_phi"),
            Self::MathSqrt2 => d("Math", "sqrt2", 0, Pure, "math_sqrt2"),
            Self::MathInf => d("Math", "inf", 0, Pure, "math_inf"),
            Self::MathNan => d("Math", "nan", 0, Pure, "math_nan"),
            Self::MathAbs => d("Math", "abs", 1, Pure, "math_abs"),
            Self::MathSqrt => d("Math", "sqrt", 1, Pure, "math_sqrt"),
            Self::MathCbrt => d("Math", "cbrt", 1, Pure, "math_cbrt"),
            Self::MathExp => d("Math", "exp", 1, Pure, "math_exp"),
            Self::MathExp2 => d("Math", "exp2", 1, Pure, "math_exp2"),
            Self::MathLog => d("Math", "log", 1, Pure, "math_log"),
            Self::MathLog2 => d("Math", "log2", 1, Pure, "math_log2"),
            Self::MathLog10 => d("Math", "log10", 1, Pure, "math_log10"),
            Self::MathSin => d("Math", "sin", 1, Pure, "math_sin"),
            Self::MathCos => d("Math", "cos", 1, Pure, "math_cos"),
            Self::MathTan => d("Math", "tan", 1, Pure, "math_tan"),
            Self::MathAsin => d("Math", "asin", 1, Pure, "math_asin"),
            Self::MathAcos => d("Math", "acos", 1, Pure, "math_acos"),
            Self::MathAtan => d("Math", "atan", 1, Pure, "math_atan"),
            Self::MathSinh => d("Math", "sinh", 1, Pure, "math_sinh"),
            Self::MathCosh => d("Math", "cosh", 1, Pure, "math_cosh"),
            Self::MathTanh => d("Math", "tanh", 1, Pure, "math_tanh"),
            Self::MathAsinh => d("Math", "asinh", 1, Pure, "math_asinh"),
            Self::MathAcosh => d("Math", "acosh", 1, Pure, "math_acosh"),
            Self::MathAtanh => d("Math", "atanh", 1, Pure, "math_atanh"),
            Self::MathFloor => d("Math", "floor", 1, Pure, "math_floor"),
            Self::MathCeil => d("Math", "ceil", 1, Pure, "math_ceil"),
            Self::MathRound => d("Math", "round", 1, Pure, "math_round"),
            Self::MathTrunc => d("Math", "trunc", 1, Pure, "math_trunc"),
            Self::MathPow => d("Math", "pow", 2, Pure, "math_pow"),
            Self::MathHypot => d("Math", "hypot", 2, Pure, "math_hypot"),
            Self::MathAtan2 => d("Math", "atan2", 2, Pure, "math_atan2"),
            Self::MathMod => d("Math", "mod", 2, Pure, "math_mod"),
            Self::MathRemainder => d("Math", "remainder", 2, Pure, "math_remainder"),
            // ── Dict ────────────────────────────────────────────────────────
            Self::DictEmpty => d("Dict", "empty", 0, Pure, "dict_empty"),
            Self::DictIsEmpty => d("Dict", "isEmpty", 1, Pure, "dict_is_empty"),
            Self::DictSize => d("Dict", "size", 1, Pure, "dict_size"),
            Self::DictKeys => d("Dict", "keys", 1, Pure, "dict_keys"),
            Self::DictValues => d("Dict", "values", 1, Pure, "dict_values"),
            Self::DictToList => d("Dict", "toList", 1, Pure, "dict_to_list"),
            Self::DictFromList => d("Dict", "fromList", 1, Pure, "dict_from_list"),
            Self::DictGet => d("Dict", "get", 2, Pure, "dict_get"),
            Self::DictMember => d("Dict", "member", 2, Pure, "dict_member"),
            Self::DictRemove => d("Dict", "remove", 2, Pure, "dict_remove"),
            Self::DictUnion => d("Dict", "union", 2, Pure, "dict_union"),
            Self::DictMap => d("Dict", "map", 2, Pure, "dict_map"),
            Self::DictInsert => d("Dict", "insert", 3, Pure, "dict_insert"),
            Self::DictFoldl => d("Dict", "foldl", 3, Pure, "dict_foldl"),
            // ── Set ─────────────────────────────────────────────────────────
            Self::SetEmpty => d("Set", "empty", 0, Pure, "set_empty"),
            Self::SetSize => d("Set", "size", 1, Pure, "set_size"),
            Self::SetToList => d("Set", "toList", 1, Pure, "set_to_list"),
            Self::SetFromList => d("Set", "fromList", 1, Pure, "set_from_list"),
            Self::SetMember => d("Set", "member", 2, Pure, "set_member"),
            Self::SetInsert => d("Set", "insert", 2, Pure, "set_insert"),
            Self::SetRemove => d("Set", "remove", 2, Pure, "set_remove"),
            Self::SetUnion => d("Set", "union", 2, Pure, "set_union"),
            Self::SetIntersect => d("Set", "intersect", 2, Pure, "set_intersect"),
            Self::SetDiff => d("Set", "diff", 2, Pure, "set_diff"),
            // ── Bytes ───────────────────────────────────────────────────────
            Self::BytesEmpty => d("Bytes", "empty", 0, Pure, "bytes_empty"),
            Self::BytesLength => d("Bytes", "length", 1, Pure, "bytes_length"),
            Self::BytesIsEmpty => d("Bytes", "isEmpty", 1, Pure, "bytes_is_empty"),
            Self::BytesFromString => d("Bytes", "fromString", 1, Pure, "bytes_from_string"),
            Self::BytesToString => d("Bytes", "toString", 1, Pure, "bytes_to_string"),
            Self::BytesFromHex => d("Bytes", "fromHex", 1, Pure, "bytes_from_hex"),
            Self::BytesToHex => d("Bytes", "toHex", 1, Pure, "bytes_to_hex"),
            Self::BytesFromBase64 => d("Bytes", "fromBase64", 1, Pure, "bytes_from_base64"),
            Self::BytesToBase64 => d("Bytes", "toBase64", 1, Pure, "bytes_to_base64"),
            Self::BytesAppend => d("Bytes", "append", 2, Pure, "bytes_append"),
            Self::BytesSlice => d("Bytes", "slice", 3, Pure, "bytes_slice"),
            // ── Encoding ────────────────────────────────────────────────────
            Self::EncodingBase64Encode => d("Encoding", "base64Encode", 1, Pure, "base64_encode"),
            Self::EncodingBase64Decode => {
                d("Encoding", "base64Decode", 1, Pure, "sky_base64_decode")
            }
            Self::EncodingUrlEncode => d("Encoding", "urlEncode", 1, Pure, "url_encode"),
            Self::EncodingUrlDecode => d("Encoding", "urlDecode", 1, Pure, "sky_url_decode"),
            Self::EncodingHexEncode => d("Encoding", "hexEncode", 1, Pure, "encoding_hex_encode"),
            Self::EncodingHexDecode => {
                d("Encoding", "hexDecode", 1, Pure, "sky_encoding_hex_decode")
            }
            // ── Json.Encode ─────────────────────────────────────────────────
            Self::JsonEncString => d("JsonEnc", "string", 1, Pure, "json_enc_string"),
            Self::JsonEncInt => d("JsonEnc", "int", 1, Pure, "json_enc_int"),
            Self::JsonEncFloat => d("JsonEnc", "float", 1, Pure, "json_enc_float"),
            Self::JsonEncBool => d("JsonEnc", "bool", 1, Pure, "json_enc_bool"),
            Self::JsonEncNull => d("JsonEnc", "null", 0, Pure, "json_enc_null"),
            Self::JsonEncList => d("JsonEnc", "list", 2, Pure, "json_enc_list"),
            Self::JsonEncObject => d("JsonEnc", "object", 1, Pure, "json_enc_object"),
            Self::JsonEncEncode => d("JsonEnc", "encode", 2, Pure, "json_enc_encode"),
            // ── Json.Decode ─────────────────────────────────────────────────
            Self::JsonDecString => d("JsonDec", "string", 0, Pure, "json_decode_string"),
            Self::JsonDecInt => d("JsonDec", "int", 0, Pure, "json_decode_int"),
            Self::JsonDecFloat => d("JsonDec", "float", 0, Pure, "json_decode_float"),
            Self::JsonDecBool => d("JsonDec", "bool", 0, Pure, "json_decode_bool"),
            Self::JsonDecDecodeString => d(
                "JsonDec",
                "decodeString",
                2,
                Pure,
                "decode_from_json_string",
            ),
            Self::JsonDecField => d("JsonDec", "field", 2, Pure, "decode_field"),
            Self::JsonDecAt => d("JsonDec", "at", 2, Pure, "decode_at"),
            Self::JsonDecIndex => d("JsonDec", "index", 2, Pure, "decode_index"),
            Self::JsonDecList => d("JsonDec", "list", 1, Pure, "decode_list"),
            Self::JsonDecMap => d("JsonDec", "map", 2, Pure, "decode_map"),
            Self::JsonDecAndThen => d("JsonDec", "andThen", 2, Pure, "decode_and_then"),
            Self::JsonDecSucceed => d("JsonDec", "succeed", 1, Pure, "decode_succeed"),
            Self::JsonDecFail => d("JsonDec", "fail", 1, Pure, "decode_fail"),
            Self::JsonDecOneOf => d("JsonDec", "oneOf", 1, Pure, "decode_one_of"),
            Self::JsonDecMap2 => d("JsonDec", "map2", 3, Pure, "decode_map2"),
            Self::JsonDecMap3 => d("JsonDec", "map3", 4, Pure, "decode_map3"),
            Self::JsonDecMap4 => d("JsonDec", "map4", 5, Pure, "decode_map4"),
            // ── Json.Decode.Pipeline ────────────────────────────────────────
            Self::JsonDecPRequired => {
                d("JsonDecP", "required", 3, Pure, "decode_pipeline_required")
            }
            Self::JsonDecPOptional => {
                d("JsonDecP", "optional", 4, Pure, "decode_pipeline_optional")
            }
            Self::JsonDecPCustom => d("JsonDecP", "custom", 2, Pure, "decode_pipeline_custom"),
            Self::JsonDecPRequiredAt => d(
                "JsonDecP",
                "requiredAt",
                3,
                Pure,
                "decode_pipeline_required_at",
            ),
            // ── Crypto ──────────────────────────────────────────────────────
            Self::CryptoSha256 => d("Crypto", "sha256", 1, Pure, "crypto_sha256"),
            Self::CryptoSha512 => d("Crypto", "sha512", 1, Pure, "crypto_sha512"),
            Self::CryptoSha1 => d("Crypto", "sha1", 1, Pure, "crypto_sha1"),
            Self::CryptoMd5 => d("Crypto", "md5", 1, Pure, "crypto_md5"),
            Self::CryptoHmacSha256 => d("Crypto", "hmacSha256", 2, Pure, "crypto_hmac_sha256"),
            Self::CryptoHmacSha512 => d("Crypto", "hmacSha512", 2, Pure, "crypto_hmac_sha512"),
            Self::CryptoRsaSha256Sign => d(
                "Crypto",
                "rsaSha256Sign",
                2,
                Pure,
                "sky_crypto_rsa_sha256_sign",
            ),
            Self::CryptoRsaSha256Verify => d(
                "Crypto",
                "rsaSha256Verify",
                3,
                Pure,
                "crypto_rsa_sha256_verify",
            ),
            Self::CryptoConstantTimeEqual => d(
                "Crypto",
                "constantTimeEqual",
                2,
                Pure,
                "crypto_constant_time_equal",
            ),
            // AEAD arity is 2 (key, plaintext/ciphertext): the Rust runtime
            // (`sky_aes_gcm_encrypt(key, plaintext)` etc.) prepends/strips a
            // fresh random nonce internally, so — unlike the Go backend which
            // took an explicit nonce/AAD arg — there is no third argument.
            Self::CryptoAesGcmEncrypt => {
                d("Crypto", "aesGcmEncrypt", 2, Pure, "sky_aes_gcm_encrypt")
            }
            Self::CryptoAesGcmDecrypt => {
                d("Crypto", "aesGcmDecrypt", 2, Pure, "sky_aes_gcm_decrypt")
            }
            Self::CryptoChacha20Encrypt => {
                d("Crypto", "chacha20Encrypt", 2, Pure, "sky_chacha20_encrypt")
            }
            Self::CryptoChacha20Decrypt => {
                d("Crypto", "chacha20Decrypt", 2, Pure, "sky_chacha20_decrypt")
            }
            Self::CryptoAesKeyFromPassword => d(
                "Crypto",
                "aesKeyFromPassword",
                2,
                Pure,
                "crypto_aes_key_from_password",
            ),
            Self::CryptoChachaKeyFromPassword => d(
                "Crypto",
                "chachaKeyFromPassword",
                2,
                Pure,
                "crypto_chacha_key_from_password",
            ),
            Self::CryptoRandomBytes => d("Crypto", "randomBytes", 1, Pure, "crypto_random_bytes"),
            Self::CryptoRandomToken => d("Crypto", "randomToken", 1, Pure, "crypto_random_token"),
            // ── Uuid ────────────────────────────────────────────────────────
            // `v4`/`v7` are EFFECT-tier (`() -> Task Error String`, task #54):
            // entropy is not a memoizable pure `String`. Arity is 1 (the unit
            // argument) so the FIRST_SCHEMED `arrow-count == decl().arity`
            // invariant holds against the `fun(Unit, task(string))` scheme.
            // Runtime `uuid_v4::<E>(_: ())` / `uuid_v7::<E>(_: ())` take that unit.
            Self::UuidV4 => d("Uuid", "v4", 1, Pure, "uuid_v4"),
            Self::UuidV7 => d("Uuid", "v7", 1, Pure, "uuid_v7"),
            Self::UuidParse => d("Uuid", "parse", 1, Pure, "uuid_parse"),
            // ── Jwt ─────────────────────────────────────────────────────────
            // Encode arity is 2 (secret/key, claims_json): the Rust runtime
            // `sky_jwt_encode_hs256(secret, claims_json)` / `_rs256(key_pem,
            // claims_json)` take exactly two args.
            Self::JwtEncodeHs256 => d("Jwt", "encodeHs256", 2, Pure, "sky_jwt_encode_hs256"),
            Self::JwtDecodeHs256 => d("Jwt", "decodeHs256", 2, Pure, "sky_jwt_decode_hs256"),
            Self::JwtEncodeRs256 => d("Jwt", "encodeRs256", 2, Pure, "sky_jwt_encode_rs256"),
            Self::JwtDecodeRs256 => d("Jwt", "decodeRs256", 2, Pure, "sky_jwt_decode_rs256"),
            // ── Task combinators ────────────────────────────────────────────
            Self::TaskSucceed => d("Task", "succeed", 1, Pure, "task_succeed"),
            Self::TaskFail => d("Task", "fail", 1, Pure, "task_fail"),
            Self::TaskMap => d("Task", "map", 2, Pure, "task_map"),
            Self::TaskAndThen => d("Task", "andThen", 2, Pure, "task_and_then"),
            Self::TaskMapError => d("Task", "mapError", 2, Pure, "task_map_error"),
            Self::TaskOnError => d("Task", "onError", 2, Pure, "task_on_error"),
            Self::TaskFromResult => d("Task", "fromResult", 1, Pure, "task_from_result"),
            Self::TaskAndThenResult => d("Task", "andThenResult", 2, Pure, "task_and_then_result"),
            Self::TaskSequence => d("Task", "sequence", 1, Pure, "task_sequence"),
            Self::TaskParallel => d("Task", "parallel", 1, Pure, "task_parallel"),
            Self::TaskRun => d("Task", "run", 1, Pure, "task_run"),
            // ── Io ──────────────────────────────────────────────────────────
            Self::IoReadLine => d("Io", "readLine", 0, Pure, "io_read_line"),
            Self::IoWriteStdout => d("Io", "writeStdout", 1, Pure, "io_write_stdout"),
            Self::IoWriteStderr => d("Io", "writeStderr", 1, Pure, "io_write_stderr"),
            // ── Time (non-TEA) ──────────────────────────────────────────────
            Self::TimeNow => d("Time", "now", 0, Pure, "time_now"),
            Self::TimeSleep => d("Time", "sleep", 1, Pure, "time_sleep"),
            Self::TimeUnixMillis => d("Time", "unixMillis", 0, Pure, "time_unix_millis"),
            // ── System ──────────────────────────────────────────────────────
            Self::SystemArgs => d("System", "args", 0, Pure, "system_args"),
            Self::SystemGetenv => d("System", "getenv", 1, Pure, "system_getenv"),
            Self::SystemGetenvOr => d("System", "getenvOr", 2, Pure, "system_getenv_or"),
            Self::SystemGetArg => d("System", "getArg", 1, Pure, "system_get_arg"),
            Self::SystemGetenvInt => d("System", "getenvInt", 2, Pure, "system_getenv_int"),
            Self::SystemGetenvBool => d("System", "getenvBool", 2, Pure, "system_getenv_bool"),
            Self::SystemSetenv => d("System", "setenv", 2, Pure, "system_setenv"),
            Self::SystemUnsetenv => d("System", "unsetenv", 1, Pure, "system_unsetenv"),
            Self::SystemCwd => d("System", "cwd", 0, Pure, "system_cwd"),
            Self::SystemLoadEnv => d("System", "loadEnv", 0, Pure, "system_load_env"),
            Self::SystemExit => d("System", "exit", 1, Pure, "system_exit"),
            // ── Random ──────────────────────────────────────────────────────
            Self::RandomInt => d("Random", "int", 2, Pure, "random_int"),
            Self::RandomFloat => d("Random", "float", 0, Pure, "random_float"),
            Self::RandomChoice => d("Random", "choice", 1, Pure, "random_choice"),
            // ── File ────────────────────────────────────────────────────────
            Self::FileReadFile => d("File", "readFile", 1, Pure, "file_read_file"),
            Self::FileWriteFile => d("File", "writeFile", 2, Pure, "file_write_file"),
            Self::FileExists => d("File", "exists", 1, Pure, "file_exists"),
            Self::FileRemove => d("File", "remove", 1, Pure, "file_remove"),
            Self::FileMkdirAll => d("File", "mkdirAll", 1, Pure, "file_mkdir_all"),
            Self::FileReadFileLimit => d("File", "readFileLimit", 2, Pure, "file_read_file_limit"),
            Self::FileReadFileBytes => d("File", "readFileBytes", 1, Pure, "file_read_file_bytes"),
            Self::FileAppend => d("File", "append", 2, Pure, "file_append"),
            Self::FileReadDir => d("File", "readDir", 1, Pure, "file_read_dir"),
            Self::FileIsDir => d("File", "isDir", 1, Pure, "file_is_dir"),
            Self::FileTempFile => d("File", "tempFile", 1, Pure, "file_temp_file"),
            Self::FileTempDir => d("File", "tempDir", 1, Pure, "file_temp_dir"),
            Self::FileCopy => d("File", "copy", 2, Pure, "file_copy"),
            Self::FileRename => d("File", "rename", 2, Pure, "file_rename"),
            Self::FileDelete => d("File", "delete", 1, Pure, "file_delete"),
            // ── Http ────────────────────────────────────────────────────────
            Self::HttpGet => d("Http", "get", 1, Pure, "http_get"),
            Self::HttpPost => d("Http", "post", 2, Pure, "http_post"),
            Self::HttpRequest => d("Http", "request", 1, Pure, "http_request"),
            Self::HttpParseQuery => d("Http", "parseQuery", 1, Pure, "http_parse_query"),
            Self::HttpDefaultRequest => {
                d("Http", "defaultRequest", 0, Pure, "http_default_request")
            }
            Self::HttpWithMethod => d("Http", "withMethod", 2, Pure, "http_with_method"),
            Self::HttpWithTimeout => d("Http", "withTimeout", 2, Pure, "http_with_timeout"),
            Self::HttpWithBody => d("Http", "withBody", 2, Pure, "http_with_body"),
            Self::HttpWithHeader => d("Http", "withHeader", 3, Pure, "http_with_header"),
            // ── Db ──────────────────────────────────────────────────────────
            Self::DbConnect => d("Db", "connect", 1, Db, "db_connect"),
            Self::DbOpen => d("Db", "open", 1, Db, "db_open"),
            Self::DbClose => d("Db", "close", 1, Db, "db_close"),
            Self::DbExecRaw => d("Db", "execRaw", 3, Db, "db_exec_raw"),
            Self::DbExec => d("Db", "exec", 3, Db, "db_exec_params"),
            Self::DbQuery => d("Db", "query", 3, Db, "db_query_params"),
            Self::DbQueryDecode => d("Db", "queryDecode", 4, Db, "db_query_decode_params"),
            Self::DbGetString => d("Db", "getString", 2, Db, "db_get_string"),
            Self::DbGetInt => d("Db", "getInt", 2, Db, "db_get_int"),
            Self::DbGetBool => d("Db", "getBool", 2, Db, "db_get_bool"),
            Self::DbGetField => d("Db", "getField", 2, Db, "db_get_field"),
            Self::DbInsertRow => d("Db", "insertRow", 3, Db, "db_insert_row"),
            Self::DbGetById => d("Db", "getById", 2, Db, "db_get_by_id"),
            Self::DbUpdateById => d("Db", "updateById", 3, Db, "db_update_by_id"),
            Self::DbDeleteById => d("Db", "deleteById", 2, Db, "db_delete_by_id"),
            Self::DbFindOneByField => d("Db", "findOneByField", 3, Db, "db_find_one_by_field"),
            Self::DbFindManyByField => d("Db", "findManyByField", 3, Db, "db_find_many_by_field"),
            Self::DbFindByConditions => d("Db", "findByConditions", 3, Db, "db_find_by_conditions"),
            Self::DbUnsafeFindWhere => d("Db", "unsafeFindWhere", 3, Db, "db_unsafe_find_where"),
            Self::DbInsertFields => d("Db", "insertFields", 3, Db, "db_insert_fields"),
            Self::DbUpdateFields => d("Db", "updateFields", 4, Db, "db_update_fields"),
            Self::DbInsertFieldsReturning => d(
                "Db",
                "insertFieldsReturning",
                5,
                Db,
                "db_insert_fields_returning",
            ),
            Self::DbWithTransaction => d("Db", "withTransaction", 2, Db, "db_with_transaction"),
            Self::DbMigrate => d("Db", "migrate", 2, Db, "db_migrate_apply"),
            // ── Db.Decode ───────────────────────────────────────────────────
            Self::DbDecString => d("Db.Decode", "string", 1, Db, "db_decode_string"),
            Self::DbDecInt => d("Db.Decode", "int", 1, Db, "db_decode_int"),
            Self::DbDecFloat => d("Db.Decode", "float", 1, Db, "db_decode_float"),
            Self::DbDecBool => d("Db.Decode", "bool", 1, Db, "db_decode_bool"),
            Self::DbDecNullable => d("Db.Decode", "nullable", 1, Db, "db_decode_nullable"),
            Self::DbDecMap => d("Db.Decode", "map", 2, Db, "db_decode_map"),
            Self::DbDecAndThen => d("Db.Decode", "andThen", 2, Db, "db_decode_and_then"),
            Self::DbDecSucceed => d("Db.Decode", "succeed", 1, Db, "db_decode_succeed"),
            Self::DbDecFail => d("Db.Decode", "fail", 1, Db, "db_decode_fail"),
            Self::DbDecMap2 => d("Db.Decode", "map2", 3, Db, "db_decode_map2"),
            Self::DbDecMap3 => d("Db.Decode", "map3", 4, Db, "db_decode_map3"),
            Self::DbDecMap4 => d("Db.Decode", "map4", 5, Db, "db_decode_map4"),
            Self::DbDecRequired => d("Db.Decode", "required", 3, Db, "db_decode_required"),
            Self::DbDecOptional => d("Db.Decode", "optional", 4, Db, "db_decode_optional"),
            // ── TEA: Cmd / Sub / Time.every (wired M5c) ─────────────────────
            Self::CmdNone => d("Cmd", "none", 0, Tea, "cmd_none"),
            Self::CmdBatch => d("Cmd", "batch", 1, Tea, "cmd_batch"),
            Self::CmdPerform => d("Cmd", "perform", 2, Tea, "cmd_perform"),
            Self::SubNone => d("Sub", "none", 0, Tea, "sub_none"),
            Self::SubBatch => d("Sub", "batch", 1, Tea, "sub_batch"),
            Self::SubEvery => d("Sub", "every", 2, Tea, "sub_every"),
            Self::TimeEvery => d("Time", "every", 2, Tea, "time_every"),
            // ── TEA: M6 reserved pub/sub ─────────────────────────────────────
            // Qualifier "Cmd" IS in qual_vars but "publish"/"publishNoEcho" are
            // NOT yet. Absent from ALL until wired; decl() is still exhaustive.
            Self::CmdPublish => d("Cmd", "publish", 1, Tea, "cmd_publish"),
            Self::CmdPublishNoEcho => d("Cmd", "publishNoEcho", 1, Tea, "cmd_publish_no_echo"),
            // Qualifier "Sub" IS in qual_vars but "subscribeTopic" is NOT yet.
            Self::SubSubscribeTopic => d("Sub", "subscribeTopic", 2, Tea, "sub_subscribe_topic"),
            // Qualifier "PubSub" is NOT yet in qual_vars — safe to put in ALL
            // (tripwire skips unknown qualifiers), but kept out for clarity.
            Self::PubSubPublish => d("PubSub", "publish", 1, Tea, "pubsub_publish"),
            Self::PubSubPublishNoEcho => {
                d("PubSub", "publishNoEcho", 1, Tea, "pubsub_publish_no_echo")
            }
            // ── Sky.Http.Server / Middleware / RateLimit ─────────────────────
            Self::ServerGet => d("Server", "get", 2, Server, "server_get"),
            Self::ServerPost => d("Server", "post", 2, Server, "server_post"),
            Self::ServerPut => d("Server", "put", 2, Server, "server_put"),
            Self::ServerDelete => d("Server", "delete", 2, Server, "server_delete"),
            Self::ServerAny => d("Server", "any", 2, Server, "server_any"),
            Self::ServerApi => d("Server", "api", 2, Server, "server_api"),
            Self::ServerStatic => d("Server", "static", 2, Server, "server_static"),
            Self::ServerListen => d("Server", "listen", 2, Server, "server_listen"),
            Self::ServerText => d("Server", "text", 1, Server, "server_text"),
            Self::ServerJson => d("Server", "json", 1, Server, "server_json"),
            Self::ServerHtml => d("Server", "html", 1, Server, "server_html"),
            Self::ServerWithStatus => d("Server", "withStatus", 2, Server, "server_with_status"),
            Self::ServerWithHeader => d("Server", "withHeader", 3, Server, "server_with_header"),
            Self::ServerRedirect => d("Server", "redirect", 1, Server, "server_redirect"),
            Self::ServerParam => d("Server", "param", 2, Server, "server_param"),
            Self::ServerQueryParam => d("Server", "queryParam", 2, Server, "server_query_param"),
            Self::ServerHeader => d("Server", "header", 2, Server, "server_header"),
            Self::ServerGetCookie => d("Server", "getCookie", 2, Server, "server_get_cookie"),
            Self::ServerBody => d("Server", "body", 1, Server, "server_body"),
            Self::ServerPath => d("Server", "path", 1, Server, "server_path"),
            Self::ServerMethod => d("Server", "method", 1, Server, "server_method"),
            Self::ServerCookieNew => d("Server", "cookie", 1, Server, "server_cookie"),
            Self::ServerWithCookie => d("Server", "withCookie", 2, Server, "server_with_cookie"),
            Self::MiddlewareWithCors => {
                d("Middleware", "withCors", 1, Server, "middleware_with_cors")
            }
            Self::MiddlewareWithLogging => d(
                "Middleware",
                "withLogging",
                1,
                Server,
                "middleware_with_logging",
            ),
            Self::MiddlewareWithBasicAuth => d(
                "Middleware",
                "withBasicAuth",
                2,
                Server,
                "middleware_with_basic_auth",
            ),
            Self::MiddlewareWithRateLimit => d(
                "Middleware",
                "withRateLimit",
                2,
                Server,
                "middleware_with_rate_limit",
            ),
            Self::RateLimitAllow => d("RateLimit", "allow", 2, Server, "rate_limit_allow"),
            // ── M7: Std.Ui / Std.Html render kernels ─────────────────────────
            Self::UiLayout => d("Ui", "layout", 2, Ui, "ui_layout"),
            Self::UiLayoutWith => d("Ui", "layoutWith", 2, Ui, "ui_layout_with"),
            Self::HtmlRender => d("Html", "render", 1, Ui, "html_render_"),
            Self::HtmlEscapeText => d("Html", "escapeHtml", 1, Ui, "html_escape_text_"),
            Self::HtmlEscapeAttr => d("Html", "escapeAttr", 1, Ui, "html_escape_attr_"),
            Self::HtmlAttrToString => d("Html", "attrToString", 1, Ui, "html_attr_to_string_"),
            // ── M7: Std.Ui element builders ──────────────────────────────────
            Self::UiNone => d("Ui", "none", 0, Ui, "ui_none_"),
            Self::UiText => d("Ui", "text", 1, Ui, "ui_text_"),
            Self::UiHtml => d("Ui", "html", 1, Ui, "ui_html_"),
            Self::UiEl => d("Ui", "el", 2, Ui, "ui_el_"),
            Self::UiRow => d("Ui", "row", 2, Ui, "ui_row_"),
            Self::UiColumn => d("Ui", "column", 2, Ui, "ui_column_"),
            Self::UiWrappedRow => d("Ui", "wrappedRow", 2, Ui, "ui_wrapped_row_"),
            Self::UiGrid => d("Ui", "grid", 2, Ui, "ui_grid_"),
            // ── M7: Std.Ui attribute builders ────────────────────────────────
            Self::UiSpacing => d("Ui", "spacing", 1, Ui, "ui_spacing_"),
            Self::UiPadding => d("Ui", "padding", 1, Ui, "ui_padding_"),
            Self::UiPaddingXY => d("Ui", "paddingXY", 2, Ui, "ui_padding_xy_"),
            Self::UiWidth => d("Ui", "width", 1, Ui, "ui_width_"),
            Self::UiHeight => d("Ui", "height", 1, Ui, "ui_height_"),
            Self::UiCenterX => d("Ui", "centerX", 0, Ui, "ui_center_x_"),
            Self::UiCenterY => d("Ui", "centerY", 0, Ui, "ui_center_y_"),
            Self::UiAlignLeft => d("Ui", "alignLeft", 0, Ui, "ui_align_left_"),
            Self::UiAlignRight => d("Ui", "alignRight", 0, Ui, "ui_align_right_"),
            Self::UiAlignTop => d("Ui", "alignTop", 0, Ui, "ui_align_top_"),
            Self::UiAlignBottom => d("Ui", "alignBottom", 0, Ui, "ui_align_bottom_"),
            Self::UiPointer => d("Ui", "pointer", 0, Ui, "ui_pointer_"),
            Self::UiClip => d("Ui", "clip", 0, Ui, "ui_clip_"),
            Self::UiScrollbars => d("Ui", "scrollbars", 0, Ui, "ui_scrollbars_"),
            Self::UiGridColumns => d("Ui", "gridColumns", 1, Ui, "ui_grid_columns_"),
            // ── M7: Std.Ui Length builders ───────────────────────────────────
            Self::UiPx => d("Ui", "px", 1, Ui, "ui_px_"),
            Self::UiFill => d("Ui", "fill", 0, Ui, "ui_fill_"),
            Self::UiContent => d("Ui", "content", 0, Ui, "ui_content_"),
            Self::UiShrink => d("Ui", "shrink", 0, Ui, "ui_shrink_"),
            Self::UiFillPortion => d("Ui", "fillPortion", 1, Ui, "ui_fill_portion_"),
            Self::UiVh => d("Ui", "vh", 1, Ui, "ui_vh_"),
            Self::UiVw => d("Ui", "vw", 1, Ui, "ui_vw_"),
            Self::UiMinimum => d("Ui", "minimum", 2, Ui, "ui_minimum_"),
            Self::UiMaximum => d("Ui", "maximum", 2, Ui, "ui_maximum_"),
            // ── M7: Std.Ui Color builders ────────────────────────────────────
            Self::UiRgb => d("Ui", "rgb", 3, Ui, "ui_rgb_"),
            Self::UiRgba => d("Ui", "rgba", 4, Ui, "ui_rgba_"),
            Self::UiWhite => d("Ui", "white", 0, Ui, "ui_white_"),
            Self::UiBlack => d("Ui", "black", 0, Ui, "ui_black_"),
            Self::UiTransparent => d("Ui", "transparent", 0, Ui, "ui_transparent_"),
            // ── M7: Background / Border / Font sub-modules ───────────────────
            Self::BackgroundColor => d("Background", "color", 1, Ui, "ui_background_color_"),
            Self::BackgroundImage => d("Background", "image", 1, Ui, "ui_background_image_"),
            Self::BorderWidth => d("Border", "width", 1, Ui, "ui_border_width_"),
            Self::BorderRounded => d("Border", "rounded", 1, Ui, "ui_border_rounded_"),
            Self::BorderColor => d("Border", "color", 1, Ui, "ui_border_color_"),
            Self::FontSize => d("Font", "size", 1, Ui, "ui_font_size_"),
            Self::FontColor => d("Font", "color", 1, Ui, "ui_font_color_"),
            Self::FontFamily => d("Font", "family", 1, Ui, "ui_font_family_"),
            Self::FontBold => d("Font", "bold", 0, Ui, "ui_font_bold_"),
            Self::FontItalic => d("Font", "italic", 0, Ui, "ui_font_italic_"),
            // ── M7: Html element builders ────────────────────────────────────
            Self::HtmlTextNode => d("Html", "text", 1, Ui, "html_text_node_"),
            Self::HtmlRawNode => d("Html", "raw", 1, Ui, "html_raw_node_"),
            Self::HtmlNode => d("Html", "node", 3, Ui, "html_node_"),
            Self::HtmlStyleNode => d("Html", "styleNode", 2, Ui, "html_style_node_"),
            // Arity corrected 3→2 / 2→1 (task #74): the tag is a baked literal,
            // not a parameter — `html_div_` etc. take (attrs, children) = 2, the
            // void `html_input_`/`html_img_` take (attrs) = 1. Runtime fn params
            // AND lower `callee_arity` (2/1) are the authorities; the old decl
            // arity was an off-by-one (same class as the AEAD/Jwt 3→2 fix, #58).
            Self::HtmlDiv => d("Html", "div", 2, Ui, "html_div_"),
            Self::HtmlSpan => d("Html", "span", 2, Ui, "html_span_"),
            Self::HtmlA => d("Html", "a", 2, Ui, "html_a_"),
            Self::HtmlButton => d("Html", "button", 2, Ui, "html_button_"),
            Self::HtmlP => d("Html", "p", 2, Ui, "html_p_"),
            Self::HtmlInput => d("Html", "input", 1, Ui, "html_input_"),
            Self::HtmlImg => d("Html", "img", 1, Ui, "html_img_"),
            // ── M7: Std.Live app-entry kernels ───────────────────────────────
            Self::LiveApp => d("Live", "app", 1, Live, "live_app"),
            Self::LiveAppRouted => d("Live", "appRouted", 1, Live, "live_app_routed"),
            Self::LiveRoute => d("Live", "route", 2, Live, "live_route"),
            Self::LiveRenderStatic => d("Live", "renderStatic", 1, Live, "live_render_static"),
            // ── M7: Std.Tui app-entry kernels ────────────────────────────────
            Self::TuiProgram => d("Tui", "program", 1, Tui, "tui_app"),
            Self::TuiApp => d("Tui", "app", 1, Tui, "tui_app_ui"),
            // ── M7: Std.Webview app-entry kernel ─────────────────────────────
            Self::WebviewApp => d("Webview", "app", 1, Webview, "webview_app"),
            // ── M7: event-attribute builders ─────────────────────────────────
            Self::UiOnClick => d("Ui", "onClick", 1, Ui, "ui_on_click_"),
            Self::UiOnFocus => d("Ui", "onFocus", 1, Ui, "ui_on_focus_"),
            Self::UiOnBlur => d("Ui", "onBlur", 1, Ui, "ui_on_blur_"),
            Self::UiOnMouseOver => d("Ui", "onMouseOver", 1, Ui, "ui_on_mouse_over_"),
            Self::UiOnMouseOut => d("Ui", "onMouseOut", 1, Ui, "ui_on_mouse_out_"),
            Self::UiOnInput => d("Ui", "onInput", 1, Ui, "ui_on_input_"),
            Self::UiOnChange => d("Ui", "onChange", 1, Ui, "ui_on_change_"),
            Self::UiOnKeyDown => d("Ui", "onKeyDown", 1, Ui, "ui_on_key_down_"),
            Self::UiOnKeyUp => d("Ui", "onKeyUp", 1, Ui, "ui_on_key_up_"),
            Self::UiOnBool => d("Ui", "onBool", 1, Ui, "ui_on_bool_"),
        }
    }

    /// All **wired** stdlib kernel variants.
    ///
    /// This slice is the single source of truth used by the canon-equality
    /// tripwire test (`canon_equals_registry` in `sky_canon`) to verify that
    /// every registry entry has a matching entry in the canon `QUALIFIERS`
    /// table.
    ///
    /// # Exclusions
    ///
    /// The following variants are intentionally absent until they are registered
    /// in the canon `QUALIFIERS` table (Phase B / M6):
    ///
    /// - [`Self::CmdPublish`] — `"Cmd"` qualifier is in `qual_vars` but
    ///   `"publish"` is not yet; adding prematurely would break the tripwire.
    /// - [`Self::CmdPublishNoEcho`] — same reason.
    /// - [`Self::SubSubscribeTopic`] — `"Sub"` qualifier is in `qual_vars`
    ///   but `"subscribeTopic"` is not yet.
    ///
    /// When those entries are added to `QUALIFIERS`, add the variant here in
    /// the same commit to keep the tripwire green.
    pub const ALL: &'static [Self] = &[
        // Log
        Self::LogPrintln,
        // String
        Self::StringFromInt,
        Self::StringFromFloat,
        Self::StringLength,
        Self::StringIsEmpty,
        Self::StringReverse,
        Self::StringToUpper,
        Self::StringToLower,
        Self::StringCasefold,
        Self::StringTrim,
        Self::StringTrimStart,
        Self::StringTrimEnd,
        Self::StringToInt,
        Self::StringToFloat,
        Self::StringFromChar,
        Self::StringFromList,
        Self::StringConcat,
        Self::StringWords,
        Self::StringLines,
        Self::StringToList,
        Self::StringIsEmail,
        Self::StringIsUrl,
        Self::StringAppend,
        Self::StringContains,
        Self::StringStartsWith,
        Self::StringEndsWith,
        Self::StringEqualFold,
        Self::StringJoin,
        Self::StringSplit,
        Self::StringRepeat,
        Self::StringDropLeft,
        Self::StringDropRight,
        Self::StringReplace,
        Self::StringSlice,
        Self::StringPadLeft,
        Self::StringPadRight,
        // Char
        Self::CharIsAlpha,
        Self::CharIsDigit,
        Self::CharIsLower,
        Self::CharIsUpper,
        Self::CharToLower,
        Self::CharToUpper,
        Self::CharToCode,
        Self::CharFromCode,
        // List
        Self::ListMap,
        Self::ListFilter,
        Self::ListFoldl,
        Self::ListFoldr,
        Self::ListLength,
        Self::ListHead,
        Self::ListTail,
        Self::ListMember,
        Self::ListRange,
        Self::ListReverse,
        Self::ListAppend,
        Self::ListConcat,
        Self::ListTake,
        Self::ListDrop,
        Self::ListZip,
        Self::ListCons,
        Self::ListIsEmpty,
        Self::ListConcatMap,
        Self::ListIndexedMap,
        // Maybe
        Self::MaybeWithDefault,
        Self::MaybeMap,
        Self::MaybeAndThen,
        // Result
        Self::ResultWithDefault,
        Self::ResultMap,
        Self::ResultOkDefault, // qualifier "_internal_" → tripwire skips
        // Math
        Self::MathMin,
        Self::MathMax,
        Self::MathPi,
        Self::MathE,
        Self::MathPhi,
        Self::MathSqrt2,
        Self::MathInf,
        Self::MathNan,
        Self::MathAbs,
        Self::MathSqrt,
        Self::MathCbrt,
        Self::MathExp,
        Self::MathExp2,
        Self::MathLog,
        Self::MathLog2,
        Self::MathLog10,
        Self::MathSin,
        Self::MathCos,
        Self::MathTan,
        Self::MathAsin,
        Self::MathAcos,
        Self::MathAtan,
        Self::MathSinh,
        Self::MathCosh,
        Self::MathTanh,
        Self::MathAsinh,
        Self::MathAcosh,
        Self::MathAtanh,
        Self::MathFloor,
        Self::MathCeil,
        Self::MathRound,
        Self::MathTrunc,
        Self::MathPow,
        Self::MathHypot,
        Self::MathAtan2,
        Self::MathMod,
        Self::MathRemainder,
        // Dict
        Self::DictEmpty,
        Self::DictIsEmpty,
        Self::DictSize,
        Self::DictKeys,
        Self::DictValues,
        Self::DictToList,
        Self::DictFromList,
        Self::DictGet,
        Self::DictMember,
        Self::DictRemove,
        Self::DictUnion,
        Self::DictMap,
        Self::DictInsert,
        Self::DictFoldl,
        // Set
        Self::SetEmpty,
        Self::SetSize,
        Self::SetToList,
        Self::SetFromList,
        Self::SetMember,
        Self::SetInsert,
        Self::SetRemove,
        Self::SetUnion,
        Self::SetIntersect,
        Self::SetDiff,
        // Bytes
        Self::BytesEmpty,
        Self::BytesLength,
        Self::BytesIsEmpty,
        Self::BytesFromString,
        Self::BytesToString,
        Self::BytesFromHex,
        Self::BytesToHex,
        Self::BytesFromBase64,
        Self::BytesToBase64,
        Self::BytesAppend,
        Self::BytesSlice,
        // Encoding
        Self::EncodingBase64Encode,
        Self::EncodingBase64Decode,
        Self::EncodingUrlEncode,
        Self::EncodingUrlDecode,
        Self::EncodingHexEncode,
        Self::EncodingHexDecode,
        // Json.Encode
        Self::JsonEncString,
        Self::JsonEncInt,
        Self::JsonEncFloat,
        Self::JsonEncBool,
        Self::JsonEncNull,
        Self::JsonEncList,
        Self::JsonEncObject,
        Self::JsonEncEncode,
        // Json.Decode
        Self::JsonDecString,
        Self::JsonDecInt,
        Self::JsonDecFloat,
        Self::JsonDecBool,
        Self::JsonDecDecodeString,
        Self::JsonDecField,
        Self::JsonDecAt,
        Self::JsonDecIndex,
        Self::JsonDecList,
        Self::JsonDecMap,
        Self::JsonDecAndThen,
        Self::JsonDecSucceed,
        Self::JsonDecFail,
        Self::JsonDecOneOf,
        Self::JsonDecMap2,
        Self::JsonDecMap3,
        Self::JsonDecMap4,
        // Json.Decode.Pipeline
        Self::JsonDecPRequired,
        Self::JsonDecPOptional,
        Self::JsonDecPCustom,
        Self::JsonDecPRequiredAt,
        // Crypto
        Self::CryptoSha256,
        Self::CryptoSha512,
        Self::CryptoSha1,
        Self::CryptoMd5,
        Self::CryptoHmacSha256,
        Self::CryptoHmacSha512,
        Self::CryptoRsaSha256Sign,
        Self::CryptoRsaSha256Verify,
        Self::CryptoConstantTimeEqual,
        Self::CryptoAesGcmEncrypt,
        Self::CryptoAesGcmDecrypt,
        Self::CryptoChacha20Encrypt,
        Self::CryptoChacha20Decrypt,
        Self::CryptoAesKeyFromPassword,
        Self::CryptoChachaKeyFromPassword,
        Self::CryptoRandomBytes,
        Self::CryptoRandomToken,
        // Uuid
        Self::UuidV4,
        Self::UuidV7,
        Self::UuidParse,
        // Jwt
        Self::JwtEncodeHs256,
        Self::JwtDecodeHs256,
        Self::JwtEncodeRs256,
        Self::JwtDecodeRs256,
        // Task
        Self::TaskSucceed,
        Self::TaskFail,
        Self::TaskMap,
        Self::TaskAndThen,
        Self::TaskMapError,
        Self::TaskOnError,
        Self::TaskFromResult,
        Self::TaskAndThenResult,
        Self::TaskSequence,
        Self::TaskParallel,
        Self::TaskRun,
        // Io
        Self::IoReadLine,
        Self::IoWriteStdout,
        Self::IoWriteStderr,
        // Time (non-TEA)
        Self::TimeNow,
        Self::TimeSleep,
        Self::TimeUnixMillis,
        // System
        Self::SystemArgs,
        Self::SystemGetenv,
        Self::SystemGetenvOr,
        Self::SystemGetArg,
        Self::SystemGetenvInt,
        Self::SystemGetenvBool,
        Self::SystemSetenv,
        Self::SystemUnsetenv,
        Self::SystemCwd,
        Self::SystemLoadEnv,
        Self::SystemExit,
        // Random
        Self::RandomInt,
        Self::RandomFloat,
        Self::RandomChoice,
        // File
        Self::FileReadFile,
        Self::FileWriteFile,
        Self::FileExists,
        Self::FileRemove,
        Self::FileMkdirAll,
        Self::FileReadFileLimit,
        Self::FileReadFileBytes,
        Self::FileAppend,
        Self::FileReadDir,
        Self::FileIsDir,
        Self::FileTempFile,
        Self::FileTempDir,
        Self::FileCopy,
        Self::FileRename,
        Self::FileDelete,
        // Http
        Self::HttpGet,
        Self::HttpPost,
        Self::HttpRequest,
        Self::HttpParseQuery,
        Self::HttpDefaultRequest,
        Self::HttpWithMethod,
        Self::HttpWithTimeout,
        Self::HttpWithBody,
        Self::HttpWithHeader,
        // Db
        Self::DbConnect,
        Self::DbOpen,
        Self::DbClose,
        Self::DbExecRaw,
        Self::DbExec,
        Self::DbQuery,
        Self::DbQueryDecode,
        Self::DbGetString,
        Self::DbGetInt,
        Self::DbGetBool,
        Self::DbGetField,
        Self::DbInsertRow,
        Self::DbGetById,
        Self::DbUpdateById,
        Self::DbDeleteById,
        Self::DbFindOneByField,
        Self::DbFindManyByField,
        Self::DbFindByConditions,
        Self::DbUnsafeFindWhere,
        Self::DbInsertFields,
        Self::DbUpdateFields,
        Self::DbInsertFieldsReturning,
        Self::DbWithTransaction,
        Self::DbMigrate,
        // Db.Decode
        Self::DbDecString,
        Self::DbDecInt,
        Self::DbDecFloat,
        Self::DbDecBool,
        Self::DbDecNullable,
        Self::DbDecMap,
        Self::DbDecAndThen,
        Self::DbDecSucceed,
        Self::DbDecFail,
        Self::DbDecMap2,
        Self::DbDecMap3,
        Self::DbDecMap4,
        Self::DbDecRequired,
        Self::DbDecOptional,
        // TEA: Cmd / Sub / Time.every
        Self::CmdNone,
        Self::CmdBatch,
        Self::CmdPerform,
        Self::SubNone,
        Self::SubBatch,
        Self::SubEvery,
        Self::TimeEvery,
        // TEA: PubSub M6 reserved (qualifier "PubSub" not yet in qual_vars →
        // tripwire skips; kept here so they appear in the registry index)
        Self::PubSubPublish,
        Self::PubSubPublishNoEcho,
        // Sky.Http.Server / Middleware / RateLimit
        Self::ServerGet,
        Self::ServerPost,
        Self::ServerPut,
        Self::ServerDelete,
        Self::ServerAny,
        Self::ServerApi,
        Self::ServerStatic,
        Self::ServerListen,
        Self::ServerText,
        Self::ServerJson,
        Self::ServerHtml,
        Self::ServerWithStatus,
        Self::ServerWithHeader,
        Self::ServerRedirect,
        Self::ServerParam,
        Self::ServerQueryParam,
        Self::ServerHeader,
        Self::ServerGetCookie,
        Self::ServerBody,
        Self::ServerPath,
        Self::ServerMethod,
        Self::ServerCookieNew,
        Self::ServerWithCookie,
        Self::MiddlewareWithCors,
        Self::MiddlewareWithLogging,
        Self::MiddlewareWithBasicAuth,
        Self::MiddlewareWithRateLimit,
        Self::RateLimitAllow,
        // M7: Ui / Html render kernels
        Self::UiLayout,
        Self::UiLayoutWith,
        Self::HtmlRender,
        Self::HtmlEscapeText,
        Self::HtmlEscapeAttr,
        Self::HtmlAttrToString,
        // M7: Ui element builders
        Self::UiNone,
        Self::UiText,
        Self::UiHtml,
        Self::UiEl,
        Self::UiRow,
        Self::UiColumn,
        Self::UiWrappedRow,
        Self::UiGrid,
        // M7: Ui attribute builders
        Self::UiSpacing,
        Self::UiPadding,
        Self::UiPaddingXY,
        Self::UiWidth,
        Self::UiHeight,
        Self::UiCenterX,
        Self::UiCenterY,
        Self::UiAlignLeft,
        Self::UiAlignRight,
        Self::UiAlignTop,
        Self::UiAlignBottom,
        Self::UiPointer,
        Self::UiClip,
        Self::UiScrollbars,
        Self::UiGridColumns,
        // M7: Ui Length builders
        Self::UiPx,
        Self::UiFill,
        Self::UiContent,
        Self::UiShrink,
        Self::UiFillPortion,
        Self::UiVh,
        Self::UiVw,
        Self::UiMinimum,
        Self::UiMaximum,
        // M7: Ui Color builders
        Self::UiRgb,
        Self::UiRgba,
        Self::UiWhite,
        Self::UiBlack,
        Self::UiTransparent,
        // M7: Background / Border / Font
        Self::BackgroundColor,
        Self::BackgroundImage,
        Self::BorderWidth,
        Self::BorderRounded,
        Self::BorderColor,
        Self::FontSize,
        Self::FontColor,
        Self::FontFamily,
        Self::FontBold,
        Self::FontItalic,
        // M7: Html element builders
        Self::HtmlTextNode,
        Self::HtmlRawNode,
        Self::HtmlNode,
        Self::HtmlDiv,
        Self::HtmlSpan,
        Self::HtmlA,
        Self::HtmlButton,
        Self::HtmlP,
        Self::HtmlInput,
        Self::HtmlImg,
        // M7: Live
        Self::LiveApp,
        Self::LiveAppRouted,
        Self::LiveRoute,
        Self::LiveRenderStatic,
        // M7: Tui
        Self::TuiProgram,
        Self::TuiApp,
        // M7: Webview
        Self::WebviewApp,
        // M7: event-attribute builders
        Self::UiOnClick,
        Self::UiOnFocus,
        Self::UiOnBlur,
        Self::UiOnMouseOver,
        Self::UiOnMouseOut,
        Self::UiOnInput,
        Self::UiOnChange,
        Self::UiOnKeyDown,
        Self::UiOnKeyUp,
        Self::UiOnBool,
    ];

    // ── Classification predicates (moved from sky_ir::KernelFn) ─────────────
    // These are the single authoritative classification lists.  `sky_ir`
    // re-exports them through the `type KernelFn = StdlibKernel` alias.

    /// `true` when this variant belongs to the `Db` / `Db.Decode` subsystem.
    #[must_use]
    pub const fn is_db(self) -> bool {
        matches!(
            self,
            Self::DbConnect
                | Self::DbOpen
                | Self::DbClose
                | Self::DbExecRaw
                | Self::DbExec
                | Self::DbQuery
                | Self::DbQueryDecode
                | Self::DbGetString
                | Self::DbGetInt
                | Self::DbGetBool
                | Self::DbGetField
                | Self::DbInsertRow
                | Self::DbGetById
                | Self::DbUpdateById
                | Self::DbDeleteById
                | Self::DbFindOneByField
                | Self::DbFindManyByField
                | Self::DbFindByConditions
                | Self::DbUnsafeFindWhere
                | Self::DbInsertFields
                | Self::DbUpdateFields
                | Self::DbInsertFieldsReturning
                | Self::DbWithTransaction
                | Self::DbMigrate
                | Self::DbDecString
                | Self::DbDecInt
                | Self::DbDecFloat
                | Self::DbDecBool
                | Self::DbDecNullable
                | Self::DbDecMap
                | Self::DbDecAndThen
                | Self::DbDecSucceed
                | Self::DbDecFail
                | Self::DbDecMap2
                | Self::DbDecMap3
                | Self::DbDecMap4
                | Self::DbDecRequired
                | Self::DbDecOptional
        )
    }

    /// `true` when this variant belongs to the TEA (`Cmd` / `Sub` /
    /// `Time.every`) subsystem, including M6-reserved pub/sub variants.
    #[must_use]
    pub const fn is_tea(self) -> bool {
        matches!(
            self,
            Self::CmdNone
                | Self::CmdBatch
                | Self::CmdPerform
                | Self::SubNone
                | Self::SubBatch
                | Self::SubEvery
                | Self::TimeEvery
                | Self::CmdPublish
                | Self::CmdPublishNoEcho
                | Self::SubSubscribeTopic
                | Self::PubSubPublish
                | Self::PubSubPublishNoEcho
        )
    }

    /// `true` when this variant belongs to the `Sky.Http.Server` / Middleware
    /// / `RateLimit` subsystem.
    #[must_use]
    pub const fn is_server(self) -> bool {
        matches!(
            self,
            Self::ServerGet
                | Self::ServerPost
                | Self::ServerPut
                | Self::ServerDelete
                | Self::ServerAny
                | Self::ServerApi
                | Self::ServerStatic
                | Self::ServerListen
                | Self::ServerText
                | Self::ServerJson
                | Self::ServerHtml
                | Self::ServerWithStatus
                | Self::ServerWithHeader
                | Self::ServerRedirect
                | Self::ServerParam
                | Self::ServerQueryParam
                | Self::ServerHeader
                | Self::ServerGetCookie
                | Self::ServerBody
                | Self::ServerPath
                | Self::ServerMethod
                | Self::ServerCookieNew
                | Self::ServerWithCookie
                | Self::MiddlewareWithCors
                | Self::MiddlewareWithLogging
                | Self::MiddlewareWithBasicAuth
                | Self::MiddlewareWithRateLimit
                | Self::RateLimitAllow
        )
    }

    /// `true` when this variant belongs to the `Std.Ui` / `Std.Html` M7
    /// subsystem.
    #[must_use]
    pub const fn is_ui(self) -> bool {
        matches!(
            self,
            Self::UiLayout
                | Self::UiLayoutWith
                | Self::HtmlRender
                | Self::HtmlEscapeText
                | Self::HtmlEscapeAttr
                | Self::HtmlAttrToString
                | Self::UiNone
                | Self::UiText
                | Self::UiHtml
                | Self::UiEl
                | Self::UiRow
                | Self::UiColumn
                | Self::UiWrappedRow
                | Self::UiGrid
                | Self::UiSpacing
                | Self::UiPadding
                | Self::UiPaddingXY
                | Self::UiWidth
                | Self::UiHeight
                | Self::UiCenterX
                | Self::UiCenterY
                | Self::UiAlignLeft
                | Self::UiAlignRight
                | Self::UiAlignTop
                | Self::UiAlignBottom
                | Self::UiPointer
                | Self::UiClip
                | Self::UiScrollbars
                | Self::UiGridColumns
                | Self::UiPx
                | Self::UiFill
                | Self::UiContent
                | Self::UiShrink
                | Self::UiFillPortion
                | Self::UiVh
                | Self::UiVw
                | Self::UiMinimum
                | Self::UiMaximum
                | Self::UiRgb
                | Self::UiRgba
                | Self::UiWhite
                | Self::UiBlack
                | Self::UiTransparent
                | Self::BackgroundColor
                | Self::BackgroundImage
                | Self::BorderWidth
                | Self::BorderRounded
                | Self::BorderColor
                | Self::FontSize
                | Self::FontColor
                | Self::FontFamily
                | Self::FontBold
                | Self::FontItalic
                | Self::HtmlTextNode
                | Self::HtmlRawNode
                | Self::HtmlNode
                | Self::HtmlDiv
                | Self::HtmlSpan
                | Self::HtmlA
                | Self::HtmlButton
                | Self::HtmlP
                | Self::HtmlInput
                | Self::HtmlImg
                | Self::UiOnClick
                | Self::UiOnFocus
                | Self::UiOnBlur
                | Self::UiOnMouseOver
                | Self::UiOnMouseOut
                | Self::UiOnInput
                | Self::UiOnChange
                | Self::UiOnKeyDown
                | Self::UiOnKeyUp
                | Self::UiOnBool
        )
    }

    /// `true` when this variant belongs to the `Std.Live` app-entry subsystem.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(
            self,
            Self::LiveApp | Self::LiveAppRouted | Self::LiveRoute | Self::LiveRenderStatic
        )
    }

    /// `true` when this variant belongs to the `Std.Tui` app-entry subsystem.
    #[must_use]
    pub const fn is_tui(self) -> bool {
        matches!(self, Self::TuiProgram | Self::TuiApp)
    }

    /// `true` when this variant is the `Std.Webview` app-entry kernel.
    #[must_use]
    pub const fn is_webview(self) -> bool {
        matches!(self, Self::WebviewApp)
    }
}

// ── Two-tier kernel identity ─────────────────────────────────────────────────

/// Opaque identifier for a user-provided FFI binding.
///
/// Phase A stub — the inner `u32` has no public API.  Phase B will expose
/// constructors tied to the FFI introspection pipeline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FfiKernelId(u32);

/// A fully-resolved kernel function: either a known stdlib kernel (resolved
/// at canonicalisation time) or a user-provided FFI binding (resolved during
/// the FFI phase; Phase A stub).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelId {
    /// A known stdlib kernel.
    Stdlib(StdlibKernel),
    /// A user-provided FFI binding (Phase A stub).
    Ffi(FfiKernelId),
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::StdlibKernel;

    /// Verifies that no two non-internal variants in [`StdlibKernel::ALL`] share
    /// the same `(qualifier, name)` pair.
    ///
    /// A collision in `decl()` would let `stdlib_index`'s silent last-wins insert
    /// silently alias one variant onto another, making `id = Some(k)` ambiguous:
    /// the variant stored in the index would not necessarily be the one `decl()`
    /// names, and the Phase B fast path would fire with the wrong variant.
    ///
    /// MECHANICAL: built from `ALL` + `decl()` only — no read of `stdlib_index`
    /// or any runtime state.  Fails deterministically on any transposition in
    /// `decl()` that creates a duplicate `(qualifier, name)` pair, regardless of
    /// whether the compiler is ever invoked.
    #[test]
    fn no_colliding_qualifier_name_pairs() {
        let mut seen: HashMap<(&'static str, &'static str), StdlibKernel> = HashMap::new();
        let mut non_internal_count: usize = 0;

        for &sk in StdlibKernel::ALL {
            let decl = sk.decl();
            // Skip internal-only entries (qualifier starts with '_', e.g.
            // ResultOkDefault whose qualifier is "_internal_").  These are never
            // inserted into stdlib_index and need not be injective with respect
            // to the public namespace.
            if decl.qualifier.starts_with('_') {
                continue;
            }
            non_internal_count += 1;
            let prior = seen.insert((decl.qualifier, decl.name), sk);
            assert!(
                prior.is_none(),
                "COLLISION in StdlibKernel::decl(): \
                 StdlibKernel::{sk:?} and StdlibKernel::{prior:?} \
                 both declare (qualifier={:?}, name={:?}). \
                 decl() must be injective over non-internal ALL variants; \
                 stdlib_index's last-wins insert would silently drop one.",
                decl.qualifier,
                decl.name,
            );
        }

        // Sanity: the HashMap length must equal the non-internal variant count.
        assert_eq!(
            seen.len(),
            non_internal_count,
            "HashMap len ({}) != non-internal variant count ({}); loop accounting broken",
            seen.len(),
            non_internal_count,
        );
    }
}
