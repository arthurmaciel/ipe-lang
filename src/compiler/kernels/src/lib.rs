//! Kernel-function registry — the single closed enum covering every Ipê
//! stdlib kernel.
//!
//! # DAG constraint
//!
//! `ipe_kernels` is a **leaf crate**.  Its only permitted dependencies are
//! `ipe_intern` and `ipe_diagnostics`.  No edge to `ipe_ir`, `ipe_types`, or
//! `ipe_backend_rust` is ever allowed; those crates import `ipe_kernels` and a
//! reverse edge would create a DAG cycle.
//!
//! `ipe_ir` re-exports `type KernelFn = ipe_kernels::StdlibKernel` so
//! call-sites reach the enum through either crate.

#![allow(clippy::module_name_repetitions)] // KernelId / KernelClass / FfiKernelId all contain "Kernel"
#![forbid(unsafe_code)]

mod capability;
pub use capability::{Capability, UnknownCapability};

/// Classification of a kernel variant by which compiler / runtime subsystem
/// owns its emission.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum KernelClass {
    /// String, Char, Math, List, Maybe, Result, Dict, Set, Bytes, Encoding,
    /// Json*, Crypto, Uuid, Jwt, Task combinators, Io, Time (non-TEA),
    /// System, Random, File, Http — everything that does not belong to a
    /// specialised subsystem.
    Pure,
    /// `Ipe.Db` / `Db.Decode` kernels.
    Db,
    /// `Ipe.Http.Server` / Middleware / `RateLimit` kernels.
    Server,
    /// `Cmd` / `Sub` / `Time.every` TEA wiring kernels, including reserved
    /// pub/sub variants.
    Tea,
    /// `Ipe.Ui` / `Ipe.Html` element and attribute builders.
    Ui,
    /// `Ipe.Web` app-entry kernels.
    Web,
    /// `Ipe.Terminal` app-entry kernels (`appScreen`, `appLines`).
    Terminal,
    /// `Ipe.WebView` app-entry kernel.
    WebView,
    /// Reserved for the FFI kernel tier.
    Ffi,
}

/// A conditionally-vendored runtime feature-module that a kernel's emitted
/// symbol lives in but whose emit-`class` does NOT already pull in.
///
/// The backend trims the emitted `ipe_runtime/mod.rs` to a base set and appends
/// feature-modules per `uses_*` flag. A kernel's emit [`KernelClass`] drives its
/// codegen dispatch, but is NOT the same fact as "which vendored module defines
/// the symbol I emit": `Cmd.publish` is `class = Tea` yet its `cmd_publish`
/// symbol lives in `live::pubsub`; `HttpStream.chunks` is `class = Pure` yet its
/// `sub_subscribe_stream` symbol lives in `http_stream`. When those two facts
/// diverge, the module the symbol needs must be declared independently of the
/// class — otherwise `ipe` accepts the program (exit 0) but the emitted crate
/// fails `cargo build` (E0425/E0412), the module-set SEAL breach class.
///
/// This is the SINGLE source of truth for that divergence: [`KernelFn::required_runtime_module`]
/// returns it, and the lowerer's per-program kernel scan sets the matching
/// `uses_*` flag from it. A kernel whose symbol lives in the module its class
/// already pulls in returns `None` — no second table to keep in sync.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuntimeModule {
    /// The `web` feature-module (`ipe_runtime::web::*`, incl. `pubsub`).
    /// Declared by the `uses_web` `mod.rs` append.
    Web,
    /// The `server` feature-module set (`ipe_runtime::server` +
    /// `server_stream` + `http_stream`). Declared by the `uses_server` append.
    Server,
}

/// The event-payload shape of a `Ipe.Html.Events` builder.
///
/// Drives both the constrain scheme (the argument type) and the backend emit
/// arm (which `html::Event` variant to construct). Making the shape an ADT —
/// rather than re-deriving it from the kernel name at each site — keeps the
/// scheme and the emit in lockstep and makes an unhandled shape a
/// non-exhaustive-match error.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum HtmlEventShape {
    /// Zero wire args — the `Msg` dispatches as-is. `msg -> Attribute msg`.
    /// Constructs `Event::OnMsg(name, msg)`.
    Msg,
    /// Value-carrying — the handler receives the input string.
    /// `(String -> msg) -> Attribute msg`. Constructs `Event::OnString`.
    String,
    /// Checkbox state — the handler receives the checked bool.
    /// `(Bool -> msg) -> Attribute msg`. Constructs `Event::OnBool`.
    Bool,
    /// Heterogeneous payload whose handler type is DECOUPLED from `msg`
    /// (`onSubmit`: `a -> Attribute msg`). `msg`/the payload type stay free at
    /// the Ipê/HM level only; the codegen-side runtime constructor
    /// (`html_on_raw_`) now builds `Event::OnForm` with the concrete payload
    /// type recovered via Rust generic inference — never `Arc<dyn Any>` at
    /// runtime.
    Raw,
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
    /// Ipê-level arity: number of arguments before the result.
    pub arity: u8,
    /// Which subsystem owns emission of this kernel.
    pub class: KernelClass,
    /// Name of the Rust runtime symbol that implements this kernel (from
    /// `ipe_backend_rust::naming::kernel_name`).
    pub emit: &'static str,
}

/// Every stdlib kernel function known to the Ipê compiler.
///
/// Variant order matches `lower.rs` `lower_callee` declaration order so that
/// the discriminant values are stable across a rename cycle.
///
/// # Registry invariant
///
/// [`StdlibKernel::ALL`] is the canonical wired-variant slice.  Every variant
/// in `ALL` has a matching entry in the canon `QUALIFIERS` table (verified by
/// the `canon_equals_registry` tripwire test in `ipe_canon`).  Variants
/// intentionally absent from `ALL` have their qualifier noted in the `decl()`
/// doc section below.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum StdlibKernel {
    // ── Log ─────────────────────────────────────────────────────────────────
    LogInfo,
    LogDebug,
    LogWarn,
    LogError,
    LogInfoWith,
    LogDebugWith,
    LogWarnWith,
    LogErrorWith,
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
    // Haystack-first companions (`containsIn`/`startsWithIn`/`endsWithIn`).
    StringContainsIn,
    StringStartsWithIn,
    StringEndsWithIn,
    // Char-level navigation + fold family.
    StringLeft,
    StringRight,
    StringCons,
    StringUncons,
    StringPad,
    StringIndexes,
    StringMap,
    StringFilter,
    StringFoldl,
    StringFoldr,
    StringAny,
    StringAll,
    // ── Char ────────────────────────────────────────────────────────────────
    CharIsAlpha,
    CharIsDigit,
    CharIsLower,
    CharIsUpper,
    CharToLower,
    CharToUpper,
    CharToCode,
    CharFromCode,
    CharIsAlphaNum,
    CharIsHexDigit,
    CharIsOctDigit,
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
    ListAny,
    ListAll,
    ListFind,
    // ── List batch ───────────────────────────────────────────────────
    ListFilterMap,
    ListSortBy,
    ListSort,
    ListSortWith,
    ListSingleton,
    ListRepeat,
    ListSum,
    ListProduct,
    ListMaximum,
    ListMinimum,
    ListIntersperse,
    ListPartition,
    ListUnzip,
    ListMap2,
    ListMap3,
    ListMap4,
    ListMap5,
    // ── Basics (core Prelude) ────────────────────────────────────────────────
    BasicsNot,
    BasicsIdentity,
    BasicsAlways,
    BasicsFst,
    BasicsSnd,
    BasicsModBy,
    BasicsToString,
    /// `clamp : comparable -> comparable -> comparable -> comparable`. Carries
    /// the `Comparable a` (Ord) obligation via `constrain_var_kernel`, exactly
    /// like `Math.min` / `Math.max`.
    BasicsClamp,
    // ── Basics numerics ──────────────────────────────────────────────
    /// `negate : number -> number` — unary negation on Int or Float.
    /// Also the runtime target for the `-x` desugar (`negate x`).
    BasicsNegate,
    /// `abs : number -> number` — absolute value on Int or Float.
    BasicsAbs,
    /// `sqrt : Float -> Float` — square root (Float-only, matches Elm).
    BasicsSqrt,
    /// `min : comparable -> comparable -> comparable` — Basics.min.
    BasicsMin,
    /// `max : comparable -> comparable -> comparable` — Basics.max.
    BasicsMax,
    /// `compare : comparable -> comparable -> Order` — three-way comparison.
    ///
    /// Returns `LT` / `EQ` / `GT` (a typed Rust enum on the Rust backend;
    /// `-1 / 0 / 1` int on the Go/Ipê backend — sanctioned divergence).
    /// The `comparable` (`Ord`) constraint is enforced via `constrain_var_kernel`.
    BasicsCompare,
    // ── end Basics numerics ──────────────────────────────────────────
    // ── Error (Ipe.Error — minimal `Error = String` slice) ─────────
    // Message-carrying constructors: `String -> Error`. With `IpeError = String`
    // the message IS the error value, so all eight collapse to one identity
    // runtime symbol (`ipe_error_from_message`); the distinct Ipê-level names are
    // preserved for the rich-ADT upgrade.
    ErrorUnexpected,
    ErrorInvalidInput,
    ErrorIo,
    ErrorNetwork,
    ErrorFfi,
    ErrorDecode,
    ErrorConflict,
    ErrorUnavailable,
    // Nullary constructors: `Error` (a canonical message string).
    ErrorTimeout,
    ErrorNotFound,
    ErrorPermissionDenied,
    // Render: `Error -> String` (reuses the `errorToString` runtime).
    ErrorToString,
    // Modifier: `String -> Error -> Error` (replace the message).
    ErrorWithMessage,
    // Classification: `Error -> Bool` (kind ∈ {Timeout, Network, Unavailable}).
    ErrorIsRetryable,
    // Modifier: `ErrorDetails -> Error -> Error`
    // (attaches the `ErrorDetails` union to `ErrorInfo.details`).
    ErrorWithDetails,
    // Inspectors: extract the kind (`Error -> ErrorKind`), the bare message
    // (`Error -> String`), and a kind's stable label (`ErrorKind -> String`).
    ErrorKind,
    ErrorMessage,
    ErrorKindName,
    // ── CssSafety (Ipe.CssSafety — Ipe.Css leaf security kernels) ───
    // The FOUR primitive leaf shims over the audited `css_safety` policy that the
    // compiled-source `Ipe.Css` funnels every free-string entry through (PARSE,
    // DON'T VALIDATE). `safeValue`/`safePropName`/`safeSelector` are the
    // `String -> Maybe String` parsers (`None` => the Ipê side drops the
    // declaration/rule); `stripStyleClose` is the `String -> String` breakout
    // floor for a raw `<style>` body.
    CssSafetySafeValue,
    CssSafetySafePropName,
    CssSafetySafeSelector,
    CssSafetyStripStyleClose,
    // ── Maybe ───────────────────────────────────────────────────────────────
    MaybeWithDefault,
    MaybeMap,
    MaybeAndThen,
    /// `Maybe.map2` .. `Maybe.map5` — apply an N-ary function across N `Maybe`s;
    /// the first `Nothing` short-circuits.
    MaybeMap2,
    MaybeMap3,
    MaybeMap4,
    MaybeMap5,
    /// `Maybe.andMap : Maybe a -> Maybe (a -> b) -> Maybe b`.
    MaybeAndMap,
    /// `Maybe.combine : List (Maybe a) -> Maybe (List a)`.
    MaybeCombine,
    // ── Result ──────────────────────────────────────────────────────────────
    ResultWithDefault,
    ResultMap,
    ResultAndThen,
    ResultMapError,
    /// `Result.map2` .. `Result.map5` — apply an N-ary function across N
    /// `Result`s over a shared error channel; the first `Err` short-circuits.
    ResultMap2,
    ResultMap3,
    ResultMap4,
    ResultMap5,
    /// `Result.andMap : Result e a -> Result e (a -> b) -> Result e b`.
    ResultAndMap,
    /// `Result.combine : List (Result e a) -> Result e (List a)`.
    ResultCombine,
    /// `Result.traverse : (a -> Result e b) -> List a -> Result e (List b)`
    /// — one-pass map+collect; first `Err` short-circuits.
    ResultTraverse,
    /// `Result.toMaybe : Result e a -> Maybe a` — `Ok`→`Just`, `Err`→`Nothing`.
    ResultToMaybe,
    /// `Result.fromMaybe : e -> Maybe a -> Result e a` — `Just`→`Ok`,
    /// `Nothing`→`Err err`.
    ResultFromMaybe,
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
    MathIsNaN,
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
    DictSingleton,
    DictFoldr,
    DictFilter,
    DictPartition,
    DictIntersect,
    DictDiff,
    DictUpdate,
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
    SetIsEmpty,
    SetSingleton,
    SetFoldl,
    SetFoldr,
    SetMap,
    SetFilter,
    SetPartition,
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
    // ── Jwt builder API ─────────────────────────────────────────
    /// `Jwt.claims` — arity 0; returns an empty `Claims` accumulator.
    JwtClaims,
    /// `Jwt.hs256 : String -> Algorithm` — builds an HS256 algorithm descriptor.
    JwtHs256,
    /// `Jwt.rs256 : String -> Algorithm` — builds an RS256 algorithm descriptor.
    JwtRs256,
    /// `Jwt.subject : String -> Claims -> Claims` — sets the `sub` claim.
    JwtSubject,
    /// `Jwt.issuer : String -> Claims -> Claims` — sets the `iss` claim.
    JwtIssuer,
    /// `Jwt.audience : String -> Claims -> Claims` — sets the `aud` claim.
    JwtAudience,
    /// `Jwt.expiresAt : Int -> Claims -> Claims` — sets the `exp` claim (Unix ms).
    JwtExpiresAt,
    /// `Jwt.notBefore : Int -> Claims -> Claims` — sets the `nbf` claim (Unix ms).
    JwtNotBefore,
    /// `Jwt.issuedAt : Int -> Claims -> Claims` — sets the `iat` claim (Unix ms).
    JwtIssuedAt,
    /// `Jwt.jwtId : String -> Claims -> Claims` — sets the `jti` claim.
    JwtJwtId,
    /// `Jwt.withClaim : String -> JsonEnc.Value -> Claims -> Claims` — adds an arbitrary claim.
    JwtWithClaim,
    /// `Jwt.encode : Algorithm -> Claims -> Result Error String` — signs the claims.
    JwtEncode,
    /// `Jwt.decode : Algorithm -> String -> Result Error Claims` — verifies and decodes.
    JwtDecode,
    // ── Task combinators ────────────────────────────────────────────────────
    TaskSucceed,
    TaskFail,
    TaskMap,
    /// `Task.map2`..`Task.map5` — combine 2..5 independent tasks with an N-ary
    /// function; effects run in argument order, first `Err` short-circuits.
    TaskMap2,
    TaskMap3,
    TaskMap4,
    TaskMap5,
    /// `Task.attempt : (Result Error a -> msg) -> Task Error a -> Cmd msg` —
    /// bridge a `Task` into a `Cmd`, mapping the settled `Result` to a message.
    /// Emits the runtime `cmd_perform` (arg order swapped from `Cmd.perform`).
    TaskAttempt,
    TaskAndThen,
    TaskMapError,
    TaskOnError,
    TaskFromResult,
    TaskAndThenResult,
    TaskSequence,
    TaskParallel,
    TaskRun,
    /// `Task.perform` — 1-arg legacy alias of `Task.run`; both map to
    /// `task_run` at the runtime boundary.
    TaskPerform,
    /// `Task.lazy : (() -> Task e a) -> Task e a` — deferred task creation.
    TaskLazy,
    // ── Task retry surface (retryWith) ──────────────────────────────────────
    /// `Task.retryWith : RetryPolicy Error -> Task Error a -> Task Error a`
    /// Runs the task retrying per policy on failure.
    TaskRetryWith,
    /// `Task.linearBackoff : Int -> Int -> RetryPolicy e`
    /// Constant-delay policy; kind=0.
    TaskLinearBackoff,
    /// `Task.exponentialBackoff : Int -> Int -> RetryPolicy e`
    /// Exponential back-off policy; kind=1.
    TaskExponentialBackoff,
    /// `Task.withJitter : RetryPolicy e -> RetryPolicy e`
    /// Enables random jitter on the policy.
    TaskWithJitter,
    /// `Task.retryOn : (e -> Bool) -> RetryPolicy e -> RetryPolicy e`
    /// Sets the shouldRetry predicate.
    TaskRetryOn,
    /// `Task.withRetryOn : (e -> Bool) -> RetryPolicy e -> RetryPolicy e`
    /// Alias for retryOn.
    TaskWithRetryOn,
    /// `Task.defaultRetryPolicy : RetryPolicy e`
    /// 3 attempts, 500 ms exponential, no jitter, retry-all.
    TaskDefaultRetryPolicy,
    /// `Task.withMaxAttempts : Int -> RetryPolicy e -> RetryPolicy e`
    TaskWithMaxAttempts,
    /// `Task.withBaseMs : Int -> RetryPolicy e -> RetryPolicy e`
    TaskWithBaseMs,
    /// `Task.withKind : Int -> RetryPolicy e -> RetryPolicy e`
    /// 0 = linear, 1 = exponential.
    TaskWithKind,
    // ── Io ──────────────────────────────────────────────────────────────────
    IoReadLine,
    IoWriteStdout,
    IoWriteStderr,
    /// `Io.println : String -> Task Error ()` — write message + newline to stdout.
    IoPrintln,
    /// `Io.eprintln : String -> Task Error ()` — write message + newline to stderr.
    IoEprintln,
    // ── Debug (development-only) ──────────────────────────────────────────────
    /// `Debug.log : String -> a -> a` — print `"label: value"` to stderr, return
    /// the value unchanged. The one deliberate impure escape hatch; a production
    /// build (`ipe build --optimize`) rejects any use (IPE-L0140).
    DebugLog,
    // ── Time (non-TEA) ──────────────────────────────────────────────────────
    TimeNow,
    TimeSleep,
    TimeUnixMillis,
    TimeTimeString,
    // `Ipe.Time` pure calendar helpers (no I/O). Reference:
    // `Ffi.callPure "Time_isLeapYear"` / `"Time_daysInMonth"`.
    TimeIsLeapYear,
    TimeDaysInMonth,
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
    // ── Process ───────────────────────────────────────────────────────────────
    ProcessRun,
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
    /// `Http.withUrl : String -> HttpRequest -> HttpRequest` — pure builder
    /// (Go parity).
    HttpWithUrl,
    /// `Http.withFollowRedirects : Bool -> HttpRequest -> HttpRequest` —
    /// pure builder (Go parity).
    HttpWithFollowRedirects,
    /// `Http.withMaxRedirects : Int -> HttpRequest -> HttpRequest` — pure
    /// builder (Go parity).
    HttpWithMaxRedirects,
    /// `Http.methodFromString : String -> Maybe HttpMethod` — typed parse
    /// boundary; `Nothing` for unrecognised verbs.
    HttpMethodFromString,
    /// `Http.methodToString : HttpMethod -> String` — canonical uppercase string.
    HttpMethodToString,
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
    DbInsertFields,
    DbUpdateFields,
    DbInsertFieldsReturning,
    DbWithTransaction,
    DbMigrate,
    /// `Db.defaultMigration : String -> Migration` — a Migration named with an
    /// empty SQL body (reference `Std/Db.ipe:246`).
    DbDefaultMigration,
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
    DbDecMoney,
    /// `Db.Decode.bytes : String -> Decoder (List Int)` — hex-decodes a
    /// BYTEA/BLOB column written by `SqlBytes` back to raw bytes.
    DbDecBytes,
    // ── TEA: Cmd / Sub / Time.every ─────────────────────────────────────────
    CmdNone,
    CmdBatch,
    CmdPerform,
    /// `Cmd.map` — `(a -> msg) -> Cmd a -> Cmd msg`; retags a sub-component's
    /// commands into the parent's message type.
    CmdMap,
    SubNone,
    SubBatch,
    SubEvery,
    TimeEvery,
    /// `Sub.map` — `(a -> msg) -> Sub a -> Sub msg`; the `Sub` twin of
    /// [`Self::CmdMap`].
    SubMap,
    // ── TEA: pub/sub ────────────────────────────────────────────────────────
    /// `Cmd.publish` — `"publish"` registered in canon `QUALIFIERS`.
    CmdPublish,
    /// `Cmd.publishNoEcho` — alongside `CmdPublish`.
    CmdPublishNoEcho,
    /// `Sub.subscribeTopic`.
    SubSubscribeTopic,
    /// `PubSub.publish` — reserved; absent from [`Self::ALL`] until the
    /// `"PubSub"` qualifier is added to the canon `QUALIFIERS` table.
    PubSubPublish,
    /// `PubSub.publishNoEcho` — reserved; absent from [`Self::ALL`].
    PubSubPublishNoEcho,
    // ── Ipe.Http.Server / Middleware / RateLimit ─────────────────────────────
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
    MiddlewareWithCsrf,
    RateLimitAllow,
    // ── Ipe.Ui / Ipe.Html render kernels ─────────────────────────────────
    UiLayout,
    UiLayoutWith,
    HtmlRender,
    HtmlEscapeText,
    HtmlEscapeAttr,
    HtmlAttrToString,
    // ── Ipe.Ui element builders ──────────────────────────────────────────
    UiNone,
    UiText,
    UiHtml,
    /// `Ui.cells : List (List Char) -> Element msg` — a raw terminal cell grid
    /// embedded as an island inside an `Ipe.Ui` view under `Terminal.appScreen`.
    UiCells,
    UiEl,
    UiRow,
    UiColumn,
    UiWrappedRow,
    UiGrid,
    UiParagraph,
    UiTextColumn,
    UiButton, // (List Attr, { onPress : Maybe msg, label : Element msg }) → Element msg
    UiLink,   // (List Attr, { url : String, label : Element msg }) → Element msg
    /// `Ui.form : List (Attribute msg) -> List (Element msg) -> Element msg`
    UiForm,
    /// `Ui.image : List Attr -> { src : String, description : String } -> Element msg`
    /// — renders `<img src=… alt=…>` (a void `TaggedNode`, no children).
    UiImage,
    // ── Ipe.Ui nearby attribute builders (absolute-positioned overlays) ──
    /// `Ui.above : Element msg -> Attribute msg`
    UiAbove,
    /// `Ui.below : Element msg -> Attribute msg`
    UiBelow,
    /// `Ui.onLeft : Element msg -> Attribute msg`
    UiOnLeft,
    /// `Ui.onRight : Element msg -> Attribute msg`
    UiOnRight,
    /// `Ui.inFront : Element msg -> Attribute msg`
    UiInFront,
    /// `Ui.behind : Element msg -> Attribute msg`
    UiBehind,
    // ── Ipe.Ui attribute builders ────────────────────────────────────────
    UiSpacing,
    UiPadding,
    UiPaddingXY,
    /// `Ui.paddingEach : { top : Int, right : Int, bottom : Int, left : Int } -> Attribute msg`
    UiPaddingEach,
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
    /// `Ui.clipX : Attribute msg` — `AttrOverflow "clip" "visible"` (single-axis
    /// clip; Y stays truly visible, no `auto`-scrollbar promotion).
    UiClipX,
    /// `Ui.clipY : Attribute msg` — `AttrOverflow "visible" "clip"`.
    UiClipY,
    UiScrollbars,
    /// `Ui.scrollbarX : Attribute msg` — `AttrOverflow "auto" "hidden"`.
    UiScrollbarX,
    /// `Ui.scrollbarY : Attribute msg` — `AttrOverflow "hidden" "auto"`.
    UiScrollbarY,
    UiGridColumns,
    // ── Ipe.Ui Length builders ───────────────────────────────────────────
    UiPx,
    UiFill,
    UiContent,
    UiShrink,
    UiFillPortion,
    UiVh,
    UiVw,
    UiMinimum,
    UiMaximum,
    // ── Ipe.Ui Color builders ────────────────────────────────────────────
    UiRgb,
    UiRgba,
    UiWhite,
    UiBlack,
    UiTransparent,
    /// `Ui.colorCss color` — convert a `Color` to its CSS string representation.
    UiColorCss,
    // ── Background / Border / Font sub-modules ───────────────────────────
    BackgroundColor,
    BackgroundImage,
    /// `Background.linearGradient : Float -> List (Float, Color) -> Attribute msg`
    /// — renders `background-image: linear-gradient(<angle>deg, <c1> <p1>%, …);`
    /// via the existing `AttrBgGradient` runtime variant.
    BackgroundLinearGradient,
    BorderWidth,
    BorderRounded,
    BorderColor,
    BorderWidthEach, // { top : Int, right : Int, bottom : Int, left : Int } → Attribute msg
    BorderShadow, // { offsetX : Int, offsetY : Int, blur : Int, spread : Int, color : Color } → Attribute msg
    BorderGlow,   // Int → Color → Attribute msg (box-shadow, 0,0 offset + 0 spread; blur + colour)
    BorderInnerShadow, // same record as BorderShadow but INSET → Attribute msg
    FontSize,
    FontColor,
    FontFamily,
    FontBold,
    FontItalic,
    // ── Html element builders ────────────────────────────────────────────
    HtmlTextNode,
    HtmlRawNode,
    HtmlNode,
    /// `Html.voidNode : String -> List Attr -> Html msg` — a void element of an
    /// arbitrary (runtime) tag; the generic counterpart of the fixed-tag void
    /// builders below. Routes through the same `html_node_(tag, attrs, [])`
    /// sink as `Html.node`, just with an empty children vec baked at emit.
    HtmlVoidNode,
    /// `Html.doctype : List Html -> Html msg` — wraps children in the
    /// `!doctype-wrapper` pseudo-tag; `html::render_into_ctx` already
    /// special-cases that tag to emit a literal `<!DOCTYPE html>` prefix then
    /// the children directly (renderer support pre-existed this kernel wiring).
    HtmlDoctype,
    /// `Html.titleNode : String -> Html msg` — wraps a raw string directly in
    /// `<title>` (`HElement "title" [] [HText s]`).
    HtmlTitleNode,
    /// `Html.toString : Html msg -> String` — alias of `Html.render` (same
    /// runtime kernel `html_render_`), kept for API familiarity.
    HtmlToString,
    /// `Html.styleNode : List Attr -> String -> Html msg` — arity-2, distinct
    /// from the arity-3 `HtmlNode`. Its dedicated runtime kernel
    /// `html_style_node_` close-tag-neutralises the CSS body at construction
    /// (F7).
    HtmlStyleNode,
    HtmlDiv,
    HtmlSpan,
    HtmlA,
    HtmlButton,
    HtmlP,
    HtmlInput,
    HtmlImg,
    // ── Ipe.Html ELEMENT builders (tag-as-data) ──────────
    // Container elements (arity-2 attrs->children) and void elements (arity-1
    // attrs) that all route through the generic `html_node_` runtime sink with
    // their wire tag from `html_element_tag`. See `is_html_container`/`is_html_void`.
    HtmlH1,
    HtmlH2,
    HtmlH3,
    HtmlH4,
    HtmlH5,
    HtmlH6,
    HtmlNav,
    HtmlSection,
    HtmlArticle,
    HtmlHeader,
    /// Legacy compat alias for `Html.header` — same `<header>` tag, arity 2.
    HtmlHeaderNode,
    /// Legacy compat alias for `Html.code` — same `<code>` tag, arity 2.
    HtmlCodeNode,
    /// Legacy compat alias for `Html.main` — same `<main>` tag, arity 2.
    HtmlMainNode,
    /// Legacy compat alias for `Html.footer` — same `<footer>` tag, arity 2.
    HtmlFooterNode,
    /// Legacy compat alias for `Html.link` — same `<link>` tag, arity 1 (void).
    HtmlLinkNode,
    HtmlFooter,
    HtmlMain,
    HtmlAside,
    HtmlUl,
    HtmlOl,
    HtmlLi,
    HtmlTable,
    HtmlThead,
    HtmlTbody,
    HtmlTfoot,
    HtmlTr,
    HtmlTh,
    HtmlTd,
    HtmlTextarea,
    HtmlSelect,
    HtmlOption,
    HtmlLabel,
    HtmlForm,
    HtmlFieldset,
    HtmlLegend,
    HtmlPre,
    HtmlCode,
    HtmlStrong,
    HtmlEm,
    HtmlSmall,
    HtmlBlockquote,
    HtmlFigure,
    HtmlFigcaption,
    HtmlDetails,
    HtmlSummary,
    HtmlDialog,
    HtmlVideo,
    HtmlAudio,
    HtmlCanvas,
    HtmlIframe,
    HtmlProgress,
    HtmlMeter,
    HtmlScript,
    HtmlBody,
    HtmlTitle,
    /// `Html.htmlNode : List Attr -> List Html -> Html msg` — `<html>` document
    /// root container. Tag-as-data, same generic `html_node_` sink as `h1`/`nav`.
    HtmlHtmlNode,
    /// `Html.headNode : List Attr -> List Html -> Html msg` — `<head>` container.
    HtmlHeadNode,
    HtmlBr,
    HtmlHr,
    HtmlMeta,
    HtmlLink,
    HtmlArea,
    HtmlBase,
    HtmlCol,
    HtmlEmbed,
    HtmlSource,
    HtmlTrack,
    HtmlWbr,
    // ── Ipe.Html.Attributes builders (corpus-used direct-backing) ───────
    // String fixed-key attributes (`String -> Attribute msg`). The wire key is
    // the member name except `type_`→`type` / `for_`→`for` (see `html_attr_key`).
    HtmlAttrClass,
    HtmlAttrId,
    HtmlAttrHref,
    HtmlAttrSrc,
    HtmlAttrAlt,
    HtmlAttrValue,
    HtmlAttrName,
    HtmlAttrPlaceholder,
    HtmlAttrType,
    HtmlAttrFor,
    HtmlAttrStyle,
    HtmlAttrTitle,
    // Bool fixed-key attributes (`Bool -> Attribute msg`).
    HtmlAttrChecked,
    HtmlAttrDisabled,
    HtmlAttrReadonly,
    HtmlAttrRequired,
    HtmlAttrMultiple,
    HtmlAttrSelected,
    HtmlAttrAutofocus,
    HtmlAttrAutocomplete, // `autocomplete : String -> Attribute msg`
    // Generic attribute builders + identity.
    HtmlAttribute,     // `attribute : String -> String -> Attribute msg`
    HtmlBoolAttribute, // `boolAttribute : String -> Bool -> Attribute msg`
    HtmlNoAttr,        // `noAttr : Attribute msg`
    // ── Ipe.Web app-entry kernels ───────────────────────────────────────
    WebApp,
    WebAppRouted,
    WebRoute,
    WebRenderStatic,
    // ── Ipe.Terminal app-entry kernels ───────────────────────────────────
    /// `Terminal.appScreen` — full-screen TEA entry, `view : Model -> Element
    /// Msg`, driven by `onKey`.
    TerminalAppScreen,
    // ── Ipe.WebView app-entry kernel ─────────────────────────────────────
    WebViewApp,
    // ── event-attribute builders ─────────────────────────────────────────
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
    UiOnSubmit, // (a -> msg) -> Attribute msg  — form submit
    /// `Ui.onFile : (String -> msg) -> Attribute msg` — wire event name
    /// `"ipe-file"`; the browser-side driver reads the chosen file, base64
    /// data-URL-encodes it, and dispatches the URL string to the handler.
    UiOnFile,
    // ── Ipe.Html.Events builders — produce `Ipe.Html.Attribute msg`
    // (`html_attr`), so they unify with `Ipe.Html.Attributes` builders and the
    // element builders' `List (Ipe.Html.Attribute msg)` slot. Distinct from the
    // `UiOn*` kernels above, which produce the `Ipe.Ui.Attribute` variant for
    // the Ipe.Ui element family. Emit constructs `html::Attribute::EventAttr`.
    HtmlOnClick,
    HtmlOnFocus,
    HtmlOnBlur,
    HtmlOnMouseOver,
    HtmlOnMouseOut,
    HtmlOnSubmit,
    HtmlOnInput,
    HtmlOnChange,
    HtmlOnKeyDown,
    HtmlOnKeyUp,
    HtmlOnBool,
    // ── Ipe.Ui extended attribute builders ───────────────────────
    // Ui namespace — aspect-ratio + htmlAttribute + name/style/cinemascope
    UiSquare,        // nullary Attr: "1 / 1"
    UiWidescreen,    // nullary Attr: "16 / 9"
    UiCinemascope,   // nullary Attr: "2.35 / 1"
    UiAspectRatio,   // Float → Attr
    UiAspectRatioWH, // Int → Int → Attr
    UiHtmlAttribute, // String → String → Attr (AttrAttribute escape-hatch)
    UiName,          // String → Attr (HTML name= attribute)
    UiStyle,         // String → String → Attr (raw CSS property + value)
    UiTransitionRaw, // String → Bool → Attr (CSS transition shorthand + respect-reduced-motion flag)
    UiGridTracksRaw, // String → String → Attr (grid-template-columns + grid-template-rows)
    UiAnimateRaw, // String → String → String → Bool → Attr (name + shorthand-tail + @keyframes body + respect flag)
    // ── Breakpoint opaque constants + Ui.breakpoint wrapper ────────────
    /// `Ui.breakpoint : Breakpoint -> List (Attribute msg) -> Element msg -> Element msg`
    ///
    /// Delegates to `Ui.mediaQuery` at runtime (`ui_breakpoint_` →
    /// `ui_media_query_`), mirroring upstream's `breakpoint bp attrs child =
    /// mediaQuery (breakpointToQuery bp) attrs child` — `breakpointToQuery`
    /// is the identity here because `Breakpoint` is typed as `String` in the
    /// Rust port (see sanctioned divergence note in
    /// `constrain.rs::stdlib_scheme`, `UiBreakpoint` arm).
    UiBreakpoint,
    /// `Ui.mediaQuery : String -> List (Attribute msg) -> Element msg -> Element msg`
    ///
    /// Raw-CSS-media-query escape hatch (the typed `Breakpoint` constants
    /// cover the common cases via `Ui.breakpoint`). Wraps `child` in a
    /// marker-carrying `<div>` (`data-ipe-mq-q` = the query, gated through
    /// `SafeCssMediaQuery`; `data-ipe-mq-rules` = the attrs folded through
    /// the shared `build_style_string` collector). The Live / Webview render
    /// pipelines consume the markers via
    /// `live::style_inject::apply_style_injections` (`build_mq`) into a
    /// ipe-id-scoped `<style data-ipe-mq="<sid>">@media <q> {
    /// [ipe-id="<sid>"] { <rules> } }</style>` block. See
    /// `docs/adr/0019-ui-mediaquery-safe-boundary.md`.
    UiMediaQuery,
    UiMobile,        // Breakpoint constant: "(max-width: 767px)"
    UiTablet,        // Breakpoint constant: "(min-width: 768px) and (max-width: 1023px)"
    UiDesktop,       // Breakpoint constant: "(min-width: 1024px)"
    UiDarkMode,      // Breakpoint constant: "(prefers-color-scheme: dark)"
    UiLightMode,     // Breakpoint constant: "(prefers-color-scheme: light)"
    UiReducedMotion, // Breakpoint constant: "(prefers-reduced-motion: reduce)"
    // ── PseudoClass opaque constants + Ui.onPseudo generic escape hatch ──
    // `PseudoClass` is a genuine 5-constructor opaque runtime type (mirrors
    // `ipe_runtime::ui::element::PseudoClass` byte-for-byte — the SAME enum
    // `Background.hoverColor` / `Border.hoverColor` / `Font.hoverColor` already
    // construct internally via `AttrPseudoRule`). Unlike `Breakpoint` (typed as
    // a bare CSS-query `String`), `PseudoClass` carries no CSS text itself — it
    // is a closed 5-value tag consumed by `onPseudo`/the pseudo-class-colour
    // helpers — so it is registered as a real opaque nullary-constant type
    // rather than a String divergence.
    /// `Ui.onPseudo : PseudoClass -> List (Attribute msg) -> Attribute msg`
    /// — generic escape hatch: folds `attrs` into one CSS rules-string (the
    /// same style-collection logic as `Ui.layout`'s `style=""` attr) and
    /// attaches it as `AttrPseudoRule(pc, css)`. Sub-module helpers
    /// (`Background.hoverColor` etc.) already build on this exact primitive on
    /// the `../ipe` reference; the Rust port backs them the same way.
    UiOnPseudo,
    /// `Ui.hover : PseudoClass` — `PseudoClass::Hover`.
    UiHover,
    /// `Ui.focus : PseudoClass` — `PseudoClass::Focus`.
    UiFocus,
    /// `Ui.focusVisible : PseudoClass` — `PseudoClass::FocusVisible`.
    UiFocusVisible,
    /// `Ui.active : PseudoClass` — `PseudoClass::Active`.
    UiActive,
    /// `Ui.disabled : PseudoClass` — `PseudoClass::Disabled`. Distinct from the
    /// unrelated `Attr.disabled : Bool -> Attribute msg` (HTML boolean attr).
    UiDisabled,
    // Background namespace — pseudo-class colour tints
    BackgroundHoverColor,
    BackgroundFocusColor,
    BackgroundActiveColor,
    BackgroundDisabledColor,
    // Border namespace — style keywords (nullary)
    BorderSolid,
    BorderDashed,
    BorderDotted,
    // Border namespace — pseudo-class
    BorderHoverColor,
    BorderFocusColor,
    BorderActiveColor,
    BorderHoverWidth,   // Int → Attr
    BorderHoverRounded, // Int → Attr
    // Font namespace — weight variants (nullary)
    FontWeight,    // Int → Attr
    FontSemiBold,  // nullary (600)
    FontRegular,   // nullary (400)
    FontLight,     // nullary (300)
    FontExtraBold, // nullary (800)
    FontBlack,     // nullary (900)
    // Font namespace — decoration
    FontUnderline,    // nullary (AttrFontUnderline)
    FontNoDecoration, // nullary (AttrFontDecoration("none"))
    FontLineThrough,  // nullary (AttrFontDecoration("line-through"))
    // Font namespace — spacing (Float → Attr)
    FontLetterSpacing, // Float → Attr (AttrFontLetterSpacing)
    FontWordSpacing,   // Float → Attr (AttrFontWordSpacing)
    // Font namespace — text alignment (nullary)
    FontAlignLeft,   // nullary (AttrFontAlign("left"))
    FontAlignRight,  // nullary (AttrFontAlign("right"))
    FontAlignCenter, // nullary (AttrFontAlign("center")) — distinct from FontCenter
    FontCenter,      // nullary (AttrFontAlign("center"))
    FontJustify,     // nullary (AttrFontAlign("justify"))
    // Font namespace — string constants (nullary → String, NOT Attribute)
    FontSansSerif, // String constant "sans-serif"
    FontSerif,     // String constant "serif"
    FontMonospace, // String constant "monospace"
    // Font namespace — pseudo-class
    FontHoverColor,
    FontFocusColor,
    FontActiveColor,
    FontDisabledColor,
    FontHoverSize, // Int → Attr pseudo
    // Html.Attributes — tabindex, rows
    HtmlAttrTabindex, // Int → HtmlAttr
    HtmlAttrRows,     // Int → HtmlAttr  (<textarea rows="N">)
    // ── Effect stdlib modules ────────────────────────────────────────
    // `Terminal.appLines` — line-oriented TEA app-entry, `view : Model ->
    // String`, driven by `onLine`.
    TerminalAppLines,
    // Ipe.Auth / Ipe.Auth — authentication helpers (fail-closed: no lower arm
    // yet → IPE-L0108 at lower time; qualified registration removes N0004).
    AuthHashPassword,
    AuthHashPasswordCost,
    AuthVerifyPassword,
    AuthPasswordStrength,
    AuthSignToken,
    AuthVerifyToken,
    AuthRegister,
    AuthLogin,
    AuthSetRole,
    // Ipe.Http.Server.Stream — server-side streaming HTTP (fail-closed).
    StreamStream,
    StreamEmit,
    StreamFinish,
    StreamWithContentType,
    // Ipe.Http.Stream — client-side HTTP streaming (fail-closed).
    HttpStreamOpen,
    HttpStreamForEachChunk,
    HttpStreamClose,
    /// `Http.Stream.chunks sid toMsg` — subscribes to stream chunks; returns `Sub msg`.
    /// Classified as TEA (not server) because it returns `IpeSub<M>`.
    HttpStreamChunks,
    // ── Ipe.Http.Server.WebSocket (12 kernels) ─────────────────────
    WsDefaultCfg,          // WebSocketServerCfg (arity 0)
    WsWithOnConnect, // (WebSocketServer -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg (arity 2)
    WsWithOnMessage, // (WebSocketServer -> String -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg (arity 2)
    WsWithOnClose, // (WebSocketServer -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg (arity 2)
    WsWithOnError, // (WebSocketServer -> Error -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg (arity 2)
    WsWithMaxMessageBytes, // Int -> WebSocketServerCfg -> WebSocketServerCfg (arity 2)
    WsWithOriginPatterns, // List String -> WebSocketServerCfg -> WebSocketServerCfg (arity 2)
    WsUpgrade,     // Request -> WebSocketServerCfg -> Task Error Response (arity 2)
    WsSendToClient, // WebSocketServer -> String -> Task Error () (arity 2)
    WsSendBinaryToClient, // WebSocketServer -> Bytes -> Task Error () (arity 2)
    WsBroadcast,   // List WebSocketServer -> String -> Task Error () (arity 2)
    WsCloseClient, // WebSocketServer -> Task Error () (arity 1)
    // ── Ipe.WebSocket — outbound WebSocket client (7 kernels) ──
    // The 6 Task-tier kernels take/return a raw `Int` socket id (the stdlib
    // wraps it in the `WebSocket` ADT). `Sub_subscribeWebSocket` is the single
    // `any`-typed Sub-tier kernel the stdlib routes onOpen/onMessage/onClose/
    // onError through; the backend peephole splits it on the compile-time literal
    // `kind` string into the four typed runtime fns (sub_subscribe_ws_*).
    WebSocketConnect,       // String -> Task Error Int (arity 1)
    WebSocketConnectWith,   // WebSocketCfg -> Task Error Int (arity 1)
    WebSocketSend,          // Int -> String -> Task Error () (arity 2)
    WebSocketSendBinary,    // Int -> Bytes -> Task Error () (arity 2)
    WebSocketClose,         // Int -> Task Error () (arity 1)
    WebSocketCloseWithCode, // Int -> String -> Int -> Task Error () (arity 3)
    SubSubscribeWebSocket,  // Int -> String -> (any -> msg) -> Sub msg (arity 3)
    // ── Ipe.Env — build-time-embedded public config (wasm M5 residual) ──
    // `Env.public "KEY"` resolves ONLY for names in the project's `[wasm]
    // publicEnv` allowlist (`ipe.toml`, validated against the secret-name
    // denylist at PARSE time — `ipe_cli::project::is_denylisted_public_env_name`).
    // Any other key returns `Nothing`, by construction (the generated match
    // has no arm for it) — never a live lookup against the raw process/host
    // environment, on EITHER target.
    EnvPublic, // String -> Maybe String (arity 1)
    // ── Ipe.Ui.Region ──────────────────────────────────────────────
    RegionMainContent,      // Attribute msg (arity 0)
    RegionNavigation,       // Attribute msg (arity 0)
    RegionFooter,           // Attribute msg (arity 0)
    RegionAside,            // Attribute msg (arity 0)
    RegionHeading,          // Int → Attribute msg (arity 1)
    RegionLabel,            // String → Attribute msg (arity 1)
    RegionAnnounce,         // Attribute msg (arity 0)
    RegionAnnounceUrgently, // Attribute msg (arity 0)
    // ── Ui.input + Ui.describe + desc* constructors ───────────────────────
    UiInput,             // List (Attribute msg) -> Element msg (arity 1)
    UiDescribe,          // Description -> Attribute msg (arity 1)
    UiDescMain,          // Description (arity 0)
    UiDescNavigation,    // Description (arity 0)
    UiDescContentInfo,   // Description (arity 0)
    UiDescComplementary, // Description (arity 0)
    UiDescLivePolite,    // Description (arity 0)
    UiDescLiveAssertive, // Description (arity 0)
    UiDescHeading,       // Int -> Description (arity 1)
    UiDescLabel,         // String -> Description (arity 1)
    // ── Ipe.Ui.Input ──────────────────────────────────────────────────
    /// `Input.labelAbove : List (Attribute msg) -> Element msg -> Label msg`
    InputLabelAbove,
    /// `Input.labelBelow : List (Attribute msg) -> Element msg -> Label msg`
    InputLabelBelow,
    /// `Input.labelLeft : List (Attribute msg) -> Element msg -> Label msg`
    InputLabelLeft,
    /// `Input.labelRight : List (Attribute msg) -> Element msg -> Label msg`
    InputLabelRight,
    /// `Input.labelHidden : String -> Label msg`
    InputLabelHidden,
    /// `Input.placeholder : List (Attribute msg) -> Element msg -> Placeholder msg`
    InputPlaceholder,
    /// `Input.text : List (Attribute msg) -> { onChange, text, placeholder, label } -> Element msg`
    InputText,
    /// `Input.multiline : List (Attribute msg) -> { onChange, text, placeholder, label, spellcheck } -> Element msg`
    InputMultiline,
    /// `Input.email : List (Attribute msg) -> { onChange, text, placeholder, label } -> Element msg`
    InputEmail,
    /// `Input.username : List (Attribute msg) -> { onChange, text, placeholder, label } -> Element msg`
    InputUsername,
    /// `Input.search : List (Attribute msg) -> { onChange, text, placeholder, label } -> Element msg`
    InputSearch,
    /// `Input.currentPassword : List (Attribute msg) -> { onChange, text, placeholder, label } -> Element msg`
    InputCurrentPassword,
    /// `Input.newPassword : List (Attribute msg) -> { onChange, text, placeholder, label } -> Element msg`
    InputNewPassword,
    /// `Input.checkbox : List (Attribute msg) -> { onChange, icon, checked, label } -> Element msg`
    InputCheckbox,
    /// `Input.slider : List (Attribute msg) -> { onChange, value, min, max, step, label } -> Element msg`
    InputSlider,
    /// `Input.option : String -> Element msg -> RadioOption msg`
    InputOption,
    /// `Input.radio : List (Attribute msg) -> { onChange, options, selected, label } -> Element msg`
    InputRadio,
    /// `Input.radioRow : List (Attribute msg) -> { onChange, options, selected, label } -> Element msg`
    InputRadioRow,
    // ── Ipe.Ui.Lazy ────────────────────────────────────────────────────
    /// `Lazy.lazy : (a -> Element msg) -> a -> Element msg`
    ///
    /// **Eager in v1.** Ipê's Go runtime memoises the subtree; Ipê evaluates
    /// immediately (no keyed LRU available before the TEA diff layer).  The
    /// divergence is recorded in `docs/divergences-from-sky.md` §B-Lazy.
    LazyLazy,
    /// `Lazy.lazy2 : (a -> b -> Element msg) -> a -> b -> Element msg` (eager)
    LazyLazy2,
    /// `Lazy.lazy3 : (a -> b -> c -> Element msg) -> a -> b -> c -> Element msg` (eager)
    LazyLazy3,
    /// `Lazy.lazy4 : (a -> b -> c -> d -> Element msg) -> a -> b -> c -> d -> Element msg` (eager)
    LazyLazy4,
    /// `Lazy.lazy5 : (a -> b -> c -> d -> e -> Element msg) -> a -> b -> c -> d -> e -> Element msg` (eager)
    LazyLazy5,
    // ── Ipe.Ui.Keyed — ipe-key for diff identity ─────────────────────────
    /// `Keyed.column : List (Attribute msg) -> List (String, Element msg) -> Element msg`
    KeyedColumn,
    /// `Keyed.row : List (Attribute msg) -> List (String, Element msg) -> Element msg`
    KeyedRow,

    // ── Ipe.Decimal — arbitrary-precision decimal arithmetic ──────────────
    /// `Decimal.zero : Decimal`
    DecZero,
    /// `Decimal.one : Decimal`
    DecOne,
    /// `Decimal.oneHundred : Decimal`
    DecOneHundred,
    /// `Decimal.fromString : String -> Result Error Decimal`
    DecFromString,
    /// `Decimal.fromInt : Int -> Decimal`
    DecFromInt,
    /// `Decimal.fromFloat : Float -> Decimal`
    DecFromFloat,
    /// `Decimal.fromMinor : Int -> Int -> Decimal`
    DecFromMinor,
    /// `Decimal.toString : Decimal -> String`
    DecToString,
    /// `Decimal.toStringFixed : Int -> Decimal -> String`
    DecToStringFixed,
    /// `Decimal.toFloat : Decimal -> Float`
    DecToFloat,
    /// `Decimal.toInt : Decimal -> Int`
    DecToInt,
    /// `Decimal.toMinor : Int -> Decimal -> Int`
    DecToMinor,
    /// `Decimal.add : Decimal -> Decimal -> Decimal`
    DecAdd,
    /// `Decimal.sub : Decimal -> Decimal -> Decimal`
    DecSub,
    /// `Decimal.mul : Decimal -> Decimal -> Decimal`
    DecMul,
    /// `Decimal.div : Decimal -> Decimal -> Result Error Decimal`
    DecDiv,
    /// `Decimal.mod : Decimal -> Decimal -> Result Error Decimal`
    DecMod,
    /// `Decimal.neg : Decimal -> Decimal`
    DecNeg,
    /// `Decimal.abs : Decimal -> Decimal`
    DecAbs,
    /// `Decimal.floor : Decimal -> Decimal`
    DecFloor,
    /// `Decimal.ceil : Decimal -> Decimal`
    DecCeil,
    /// `Decimal.round : Int -> Decimal -> Decimal`
    DecRound,
    /// `Decimal.roundHalfUp : Int -> Decimal -> Decimal`
    DecRoundHalfUp,
    /// `Decimal.truncate : Int -> Decimal -> Decimal`
    DecTruncate,
    /// `Decimal.compare : Decimal -> Decimal -> Int`
    DecCompare,
    /// `Decimal.eq : Decimal -> Decimal -> Bool`
    DecEq,
    /// `Decimal.neq : Decimal -> Decimal -> Bool`
    DecNeq,
    /// `Decimal.lt : Decimal -> Decimal -> Bool`
    DecLt,
    /// `Decimal.lte : Decimal -> Decimal -> Bool`
    DecLte,
    /// `Decimal.gt : Decimal -> Decimal -> Bool`
    DecGt,
    /// `Decimal.gte : Decimal -> Decimal -> Bool`
    DecGte,
    /// `Decimal.min : Decimal -> Decimal -> Decimal`
    DecMin,
    /// `Decimal.max : Decimal -> Decimal -> Decimal`
    DecMax,
    /// `Decimal.isZero : Decimal -> Bool`
    DecIsZero,
    /// `Decimal.isPositive : Decimal -> Bool`
    DecIsPositive,
    /// `Decimal.isNegative : Decimal -> Bool`
    DecIsNegative,
    /// `Decimal.percentOf : Decimal -> Decimal -> Decimal`
    DecPercentOf,
    /// `Decimal.addPercent : Decimal -> Decimal -> Decimal`
    DecAddPercent,
    /// `Decimal.subPercent : Decimal -> Decimal -> Decimal`
    DecSubPercent,
    /// `Decimal.formatWith : String -> String -> Int -> Decimal -> String`
    DecFormatWith,
    // ── Ipe.Money — currency table + FX registry + fair-split allocate ────
    // The Ipê-side `Money` ADT carries a typed `Currency` enum; the
    // compiled-source `Ipe.Money` wrappers convert `Currency` to its ISO 4217
    // code (a `String`) before invoking these kernels, so every property /
    // format / rate kernel takes the code as a plain `String`. Runtime bodies:
    // `ipe_runtime::money::*`.
    /// `Money.minorUnits : String -> Int` — decimal places for a currency's
    /// minor unit (JPY=0, USD=2, BHD=3, BTC=8; unknown → 2).
    MoneyMinorUnits,
    /// `Money.symbol : String -> String` — currency symbol ("$", "€", "₿").
    MoneySymbol,
    /// `Money.currencyName : String -> String` — human-readable name.
    MoneyCurrencyName,
    /// `Money.isKnownCurrency : String -> Bool` — is the code a recognised
    /// ISO 4217 / crypto ticker?
    MoneyIsKnownCurrency,
    /// `Money.format : String -> Decimal -> String` — symbol-prefixed, rounded
    /// half-away-from-zero to the currency's minor units ("$2.55").
    MoneyFormat,
    /// `Money.formatWithCode : String -> Decimal -> String` — ISO-code suffix
    /// form ("2.55 USD").
    MoneyFormatWithCode,
    /// `Money.allocate : Int -> Int -> Decimal -> List Decimal` — fair split of
    /// an amount across N parts (minor-unit places, parts, amount); residue
    /// distributed toward zero. Caps `parts` at 100k (memory-amplification
    /// guard) and returns `[]` on overflow / non-positive parts.
    MoneyAllocate,
    /// `Money.setRate : String -> String -> Decimal -> Result Error ()` —
    /// register an FX rate (positive-only; auto-inverse; bounded registry).
    MoneySetRate,
    /// `Money.getRate : String -> String -> Result Error Decimal` — look up a
    /// registered rate (identity for from==to; missing → Err).
    MoneyGetRate,
    /// `Money.hasRate : String -> String -> Bool`.
    MoneyHasRate,
    /// `Money.clearRates : () -> Result Error ()` — drop every registered rate.
    MoneyClearRates,
    // ── Ipe.Db.Sql — SqlFragment builder ───────────────────────
    // Typed, parameterized WHERE-fragment combinators. Replace the removed
    // `Db.unsafeFindWhere` raw-string escape hatch: a `SqlFragment` can only be
    // constructed through these kernels, so SQL injection via a hand-built
    // WHERE clause becomes a type error (String where SqlFragment is expected)
    // rather than a runtime risk.
    /// `Sql.column : String -> SqlFragment` — validated column/table reference
    /// (dot-accepting, so `users.id` is legal).
    SqlColumn,
    /// `Sql.param : SqlValue -> SqlFragment` — binds a single `?` placeholder.
    SqlParam,
    /// `Sql.int : Int -> SqlFragment` — sugar over `Sql.param`; shares the
    /// `sql_param` runtime symbol (`i64: Into<SqlParam>` already exists).
    SqlInt,
    /// `Sql.string : String -> SqlFragment` — sugar over `Sql.param`.
    SqlString,
    /// `Sql.float : Float -> SqlFragment` — sugar over `Sql.param`.
    SqlFloat,
    /// `Sql.bool : Bool -> SqlFragment` — sugar over `Sql.param`.
    SqlBool,
    /// `Sql.eq : SqlFragment -> SqlFragment -> SqlFragment`
    SqlEq,
    /// `Sql.ne : SqlFragment -> SqlFragment -> SqlFragment`
    SqlNe,
    /// `Sql.gt : SqlFragment -> SqlFragment -> SqlFragment`
    SqlGt,
    /// `Sql.lt : SqlFragment -> SqlFragment -> SqlFragment`
    SqlLt,
    /// `Sql.gte : SqlFragment -> SqlFragment -> SqlFragment`
    SqlGte,
    /// `Sql.lte : SqlFragment -> SqlFragment -> SqlFragment`
    SqlLte,
    /// `Sql.and : SqlFragment -> SqlFragment -> SqlFragment`
    SqlAnd,
    /// `Sql.or : SqlFragment -> SqlFragment -> SqlFragment`
    SqlOr,
    /// `Sql.not : SqlFragment -> SqlFragment`
    SqlNot,
    /// `Sql.isNull : SqlFragment -> SqlFragment`
    SqlIsNull,
    /// `Sql.isNotNull : SqlFragment -> SqlFragment`
    SqlIsNotNull,
    /// `Sql.inList : SqlFragment -> List SqlValue -> SqlFragment` — `[]` emits
    /// `(1 = 0)` rather than the SQL syntax error `IN ()`.
    SqlInList,
    /// `Sql.like : SqlFragment -> String -> SqlFragment` — the pattern is
    /// always a bound param, never interpolated.
    SqlLike,
    /// `Db.findWhere : Db -> String -> SqlFragment -> Task Error (List Row)` —
    /// the `SqlFragment`-typed replacement for the removed `unsafeFindWhere`.
    DbFindWhere,
    /// `Db.deleteWhere : Db -> String -> SqlFragment -> Task Error Int`
    DbDeleteWhere,
    // ── Ipe.Secret — opaque secret-string wrapper ─────────
    // The ONLY public constructor: every `Secret` value traces back to one of
    // these calls. Never derivable from a bare `String` implicitly.
    /// `Secret.fromString : String -> Secret` — the seal; construction boundary.
    SecretFromString,
    /// `Secret.reveal : Secret -> String` — the single greppable un-parse.
    SecretReveal,
    /// `Secret.redacted : Secret -> String` — explicit `"<redacted>"` (also
    /// what `toString` / interpolation gives automatically — see
    /// `ipe_runtime::secret`'s hand-written `IpeStringify` impl).
    SecretRedacted,

    // ── Ipe.Regex — RE2 helpers ──────────────────────────────────
    // Pure, total kernels routed via the compiled-source `Ipe.Regex`
    // Layer-3 surface + `Ffi.kernel "Regex_*"` aliases. Runtime fns
    // (`ipe_runtime::regex_kernel::*`) are re-exported ungated — no feature gate
    // and no `project.rs` thread needed (the emitted `mod.rs` declares
    // `regex_kernel` unconditionally, deps always present).
    /// `Regex.compile : String -> Result Error Regex` — parse a pattern ONCE
    /// into the opaque `Regex` handle; an invalid pattern is a typed `Err`,
    /// never a silent no-match.
    RegexCompile,
    /// `Regex.match : Regex -> String -> Bool` — does the pattern match anywhere?
    RegexMatch,
    /// `Regex.find : Regex -> String -> Maybe String` — first match, if any.
    RegexFind,
    /// `Regex.findAll : Regex -> String -> List String` — every match, in order.
    RegexFindAll,
    /// `Regex.replace : Regex -> String -> String -> String` — replace every match.
    RegexReplace,
    /// `Regex.split : Regex -> String -> List String` — split on every match.
    RegexSplit,

    // ── Ipe.Path — typed, validated filesystem paths ───────────────────
    // Pure, total kernels routed via the compiled-source `Ipe.Path`
    // Layer-3 surface + `Ffi.kernel "Path_*"` aliases. Runtime fns
    // (`ipe_runtime::path::*`) are re-exported ungated (same posture as Regex).
    // `Path` is an opaque, validated type: the ONLY constructor is
    // `PathFromString` (the parse-don't-validate seal that rejects NUL bytes
    // and `..` traversal escapes); the helpers take a `Path`, never a raw
    // `String`.
    /// `Path.fromString : String -> Result Error Path` — THE seal; the only
    /// constructor. Normalises the path and rejects NUL / traversal escapes.
    PathFromString,
    /// `Path.toString : Path -> String` — THE un-parse; recover the cleaned
    /// path string.
    PathToString,
    /// `Path.base : Path -> String` — final path component.
    PathBase,
    /// `Path.dir : Path -> String` — everything but the final component.
    PathDir,
    /// `Path.ext : Path -> String` — file extension (with the dot), or empty.
    PathExt,
    /// `Path.isAbsolute : Path -> Bool` — does the path start from the root?
    PathIsAbsolute,

    // ── Ipe.Trace — opt-in tracing spans ──────────────────────────────
    // Task-effectful; runtime fns `ipe_runtime::trace::*` are re-exported
    // (emitted `mod.rs` declares `trace` unconditionally). Class `Pure` (the
    // effect lives in the `Task` scheme, same as File/Io/Http).
    /// `Trace.span : String -> Task e a -> Task e a` — wrap a Task in a named span.
    TraceSpan,
    /// `Trace.event : String -> Task Error ()` — record an instantaneous event.
    TraceEvent,
    /// `Trace.attr : String -> String -> Task Error ()` — annotate the span.
    TraceAttr,

    // ── Ipe.Compression — gzip + zstd ─────────────────────────────────
    // Task-effectful; runtime `ipe_runtime::compression::*`. Operates on `Bytes`
    // (`Vec<u8>`) to match the runtime `compression_*(Vec<u8>) -> Vec<u8>` shape.
    /// `Compression.gzip : Bytes -> Task Error Bytes`.
    CompressionGzip,
    /// `Compression.gunzip : Bytes -> Task Error Bytes`.
    CompressionGunzip,
    /// `Compression.zstdCompress : Bytes -> Task Error Bytes`.
    CompressionZstdCompress,
    /// `Compression.zstdDecompress : Bytes -> Task Error Bytes`.
    CompressionZstdDecompress,

    // ── Ipe.Csv — RFC 4180 encode/decode ──────────────────────────────
    // Runtime `ipe_runtime::csv::*`. `Csv` is the record
    // `{ header : List String, rows : List (List String) }`.
    /// `Csv.parse : String -> Result Error Csv`.
    CsvParse,
    /// `Csv.parseWithDelimiter : String -> String -> Result Error Csv`.
    CsvParseWithDelimiter,
    /// `Csv.encode : Csv -> String`.
    CsvEncode,
    /// `Csv.encodeWithDelimiter : String -> Csv -> String`.
    CsvEncodeWithDelimiter,
    /// `Csv.parseStreamFromFile : String -> Task Error (List (List String))`.
    CsvParseStreamFromFile,

    // ── Ipe.Cache — in-memory LRU + TTL cache ─────────────────────────
    // Task-effectful; runtime `ipe_runtime::cache::*` (the emitted `mod.rs`
    // declares `cache` unconditionally — same ungated-vendoring posture as
    // Csv/Compression). Routed via the compiled-source `Ipe.Cache` Layer-3
    // surface + `Ffi.kernel "Cache_*"` aliases. Class `Pure` (the effect lives
    // in the `Task` scheme, same as File/Io/Http). All kernels take the raw
    // `Int` handle; the surface `Cache k v` ADT is unwrapped in Ipê source.
    /// `Cache.newRaw : CacheCfg -> Task Error Int` — allocate, return the handle.
    CacheNewRaw,
    /// `Cache.getRaw : Int -> k -> Task Error (Maybe v)` — look up a key.
    CacheGet,
    /// `Cache.putRaw : Int -> k -> v -> Task Error ()` — insert / update.
    CachePut,
    /// `Cache.removeRaw : Int -> k -> Task Error ()` — delete a key (idempotent).
    CacheRemove,
    /// `Cache.clearRaw : Int -> Task Error ()` — purge every entry.
    CacheClear,
    /// `Cache.sizeRaw : Int -> Task Error Int` — current entry count.
    CacheSize,
    /// `Cache.statsRaw : Int -> Task Error { hits, misses, evictions }`.
    CacheStats,

    // ── Ipe.Config — typed TOML/YAML/JSON decoders ────────────────────
    // Config shares the JSON `Decoder<E, T>` carrier and its `decode_*`
    // combinator runtime fns: `string`/`int`/`float`/`bool`/`field`/`at`/
    // `list`/`map`/`andThen`/`succeed`/`fail` route to the SAME runtime fns
    // as the corresponding `JsonDec*` kernels (see `naming.rs`). Only the
    // format front-ends (`decodeToml`/`decodeYaml`/`decodeJson`), `nullable`,
    // and `loadFromFile` have Config-specific runtime fns
    // (`ipe_runtime::config_decode::*`). Distinct variants keep
    // `Config.<member>` resolution clean while reusing the shared decoder
    // runtime. Class `Pure` (Task effect lives in the scheme, same as
    // File/Io/Http).
    /// `Config.string : Decoder String` — shares `json_decode_string`.
    ConfigString,
    /// `Config.int : Decoder Int` — shares `json_decode_int`.
    ConfigInt,
    /// `Config.float : Decoder Float` — shares `json_decode_float`.
    ConfigFloat,
    /// `Config.bool : Decoder Bool` — shares `json_decode_bool`.
    ConfigBool,
    /// `Config.nullable : Decoder a -> Decoder (Maybe a)`.
    ConfigNullable,
    /// `Config.field : String -> Decoder a -> Decoder a` — shares `decode_field`.
    ConfigField,
    /// `Config.at : List String -> Decoder a -> Decoder a` — shares `decode_at`.
    ConfigAt,
    /// `Config.list : Decoder a -> Decoder (List a)` — shares `decode_list`.
    ConfigList,
    /// `Config.succeed : a -> Decoder a` — shares `decode_succeed`.
    ConfigSucceed,
    /// `Config.fail : String -> Decoder a` — shares `decode_fail`.
    ConfigFail,
    /// `Config.map : (a -> b) -> Decoder a -> Decoder b` — shares `decode_map`.
    ConfigMap,
    /// `Config.andThen : (a -> Decoder b) -> Decoder a -> Decoder b` — shares `decode_and_then`.
    ConfigAndThen,
    /// `Config.map2`..`Config.map8` — combine 2..8 decoders with an N-ary
    /// function; share the runtime `decode_map2`..`decode_map8`.
    ConfigMap2,
    ConfigMap3,
    ConfigMap4,
    ConfigMap5,
    ConfigMap6,
    ConfigMap7,
    ConfigMap8,
    /// `Config.oneOf : List (Decoder a) -> Decoder a` — first succeeding branch;
    /// shares `decode_one_of`.
    ConfigOneOf,
    /// `Config.index : Int -> Decoder a -> Decoder a` — decode the n-th array
    /// element; shares `decode_index`.
    ConfigIndex,
    /// `Config.keyValuePairs : Decoder a -> Decoder (List (String, a))` — decode
    /// every object entry; shares `decode_key_value_pairs`.
    ConfigKeyValuePairs,
    /// `Config.maybe : Decoder a -> Decoder (Maybe a)` — `Just` on success,
    /// `Nothing` on ANY failure (`config_maybe`).
    ConfigMaybe,
    /// `Config.dict : Decoder a -> Decoder (Dict String a)` — decode an object
    /// into a `Dict String a` (`config_dict`).
    ConfigDict,
    /// `Config.decodeToml : String -> Decoder a -> Result Error a`.
    ConfigDecodeToml,
    /// `Config.decodeYaml : String -> Decoder a -> Result Error a`.
    ConfigDecodeYaml,
    /// `Config.decodeJson : String -> Decoder a -> Result Error a`.
    ConfigDecodeJson,
    /// `Config.loadFromFile : String -> Decoder a -> Task Error a`.
    ConfigLoadFromFile,
    // ── Ipe.Email — provider-abstract email send ──────────────────────
    // Task-effectful; runtime `ipe_runtime::email::email_send`. Routed via the
    // compiled-source `Ipe.Email` Layer-3 surface + `Ffi.kernel "Email_send"`.
    // Class `Pure` (the effect lives in the `Task` scheme, same as File/Http).
    // Takes the runtime `EmailProvider` enum + `EmailMessage` struct (the Ipê
    // ADT / record aliases fold to those nominal runtime types).
    /// `Email.send : EmailProvider -> EmailMessage -> Task Error String`.
    EmailSend,

    // ── Ipe.Crypto typed-key newtypes ─────────────────────────────────
    // Additive Layer-3 API that wraps raw `String` keys / MACs in opaque
    // role-typed newtypes.  The existing bare-`String` kernels remain unchanged
    // (backward-compatible); these new kernels carry `Key`/`Mac` runtime
    // types from `ipe_runtime::crypto`.  All are Pure (no side-effect).
    /// `Key.fromString : String -> Key` — the ONLY constructor; parse boundary.
    CryptoKeyFromString,
    /// `Key.fromBytes : String -> Key` — construction boundary for byte-string callers.
    CryptoKeyFromBytes,
    /// `Mac.toHex : Mac -> String` — the single extraction boundary for MAC output.
    CryptoMacToHex,
    /// `Crypto.hmacSha256WithKey : Key -> String -> Mac` — typed HMAC-SHA256.
    CryptoHmacSha256WithKey,
    /// `Crypto.hmacSha512WithKey : Key -> String -> Mac` — typed HMAC-SHA512.
    CryptoHmacSha512WithKey,
    /// `Crypto.aesKeyFromPasswordKey : String -> String -> Key` — typed key derivation.
    CryptoAesKeyFromPasswordKey,
    /// `Crypto.chachaKeyFromPasswordKey : String -> String -> Key` — typed key derivation.
    CryptoChachaKeyFromPasswordKey,
    /// `Crypto.aesGcmEncryptKey : Key -> String -> Result Error String` — typed AEAD encrypt.
    CryptoAesGcmEncryptKey,
    /// `Crypto.aesGcmDecryptKey : Key -> String -> Result Error String` — typed AEAD decrypt.
    CryptoAesGcmDecryptKey,
    /// `Crypto.chacha20EncryptKey : Key -> String -> Result Error String` — typed AEAD encrypt.
    CryptoChacha20EncryptKey,
    /// `Crypto.chacha20DecryptKey : Key -> String -> Result Error String` — typed AEAD decrypt.
    CryptoChacha20DecryptKey,

    // ── Ipe.Email.EmailAddress — typed parse-don't-validate boundary ───
    // Additive API: `EmailAddress.parse` is the only constructor; downstream
    // code never sees the raw `String`.  `EmailAddress.toString` is the single
    // extraction boundary.  Both are Pure.
    /// `EmailAddress.parse : String -> Maybe EmailAddress` — parse boundary.
    EmailAddressParse,
    /// `EmailAddress.toString : EmailAddress -> String` — single extraction boundary.
    EmailAddressToString,
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
        use KernelClass::{Db, Pure, Server, Tea, Terminal, Ui, Web, WebView};
        match self {
            // ── Log ─────────────────────────────────────────────────────────
            // Qualifier "Log" is installed via `install_builtin_vars` as an
            // unqualified name; it is NOT in the canon `QUALIFIERS` table.
            // The tripwire test skips it because "Log" is absent from
            // `env.qual_vars`.
            Self::LogInfo => d("Log", "info", 1, Pure, "log_info"),
            Self::LogDebug => d("Log", "debug", 1, Pure, "log_debug"),
            Self::LogWarn => d("Log", "warn", 1, Pure, "log_warn"),
            Self::LogError => d("Log", "error", 1, Pure, "log_error"),
            Self::LogInfoWith => d("Log", "infoWith", 2, Pure, "log_info_with"),
            Self::LogDebugWith => d("Log", "debugWith", 2, Pure, "log_debug_with"),
            Self::LogWarnWith => d("Log", "warnWith", 2, Pure, "log_warn_with"),
            Self::LogErrorWith => d("Log", "errorWith", 2, Pure, "log_error_with"),
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
            Self::StringContainsIn => d("String", "containsIn", 2, Pure, "string_contains_in"),
            Self::StringStartsWithIn => {
                d("String", "startsWithIn", 2, Pure, "string_starts_with_in")
            }
            Self::StringEndsWithIn => d("String", "endsWithIn", 2, Pure, "string_ends_with_in"),
            Self::StringLeft => d("String", "left", 2, Pure, "string_left"),
            Self::StringRight => d("String", "right", 2, Pure, "string_right"),
            Self::StringCons => d("String", "cons", 2, Pure, "string_cons"),
            Self::StringUncons => d("String", "uncons", 1, Pure, "string_uncons"),
            Self::StringPad => d("String", "pad", 3, Pure, "string_pad"),
            Self::StringIndexes => d("String", "indexes", 2, Pure, "string_indexes"),
            Self::StringMap => d("String", "map", 2, Pure, "string_map"),
            Self::StringFilter => d("String", "filter", 2, Pure, "string_filter"),
            Self::StringFoldl => d("String", "foldl", 3, Pure, "string_foldl"),
            Self::StringFoldr => d("String", "foldr", 3, Pure, "string_foldr"),
            Self::StringAny => d("String", "any", 2, Pure, "string_any"),
            Self::StringAll => d("String", "all", 2, Pure, "string_all"),
            // ── Char ────────────────────────────────────────────────────────
            Self::CharIsAlpha => d("Char", "isAlpha", 1, Pure, "char_is_alpha"),
            Self::CharIsDigit => d("Char", "isDigit", 1, Pure, "char_is_digit"),
            Self::CharIsLower => d("Char", "isLower", 1, Pure, "char_is_lower"),
            Self::CharIsUpper => d("Char", "isUpper", 1, Pure, "char_is_upper"),
            Self::CharToLower => d("Char", "toLower", 1, Pure, "char_to_lower"),
            Self::CharToUpper => d("Char", "toUpper", 1, Pure, "char_to_upper"),
            Self::CharToCode => d("Char", "toCode", 1, Pure, "char_to_code"),
            Self::CharFromCode => d("Char", "fromCode", 1, Pure, "char_from_code"),
            Self::CharIsAlphaNum => d("Char", "isAlphaNum", 1, Pure, "char_is_alpha_num"),
            Self::CharIsHexDigit => d("Char", "isHexDigit", 1, Pure, "char_is_hex_digit"),
            Self::CharIsOctDigit => d("Char", "isOctDigit", 1, Pure, "char_is_oct_digit"),
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
            Self::ListCons => d("List", "cons", 2, Pure, "ipe_list_cons"),
            Self::ListIsEmpty => d("List", "isEmpty", 1, Pure, "list_is_empty"),
            Self::ListConcatMap => d("List", "concatMap", 2, Pure, "list_concat_map"),
            Self::ListIndexedMap => d("List", "indexedMap", 2, Pure, "list_indexed_map"),
            Self::ListAny => d("List", "any", 2, Pure, "list_any"),
            Self::ListAll => d("List", "all", 2, Pure, "list_all"),
            Self::ListFind => d("List", "find", 2, Pure, "list_find"),
            // ── List batch ────────────────────────────────────────────
            Self::ListFilterMap => d("List", "filterMap", 2, Pure, "list_filter_map"),
            Self::ListSortBy => d("List", "sortBy", 2, Pure, "list_sort_by"),
            Self::ListSort => d("List", "sort", 1, Pure, "list_sort"),
            Self::ListSortWith => d("List", "sortWith", 2, Pure, "list_sort_with_order"),
            Self::ListSingleton => d("List", "singleton", 1, Pure, "list_singleton"),
            Self::ListRepeat => d("List", "repeat", 2, Pure, "list_repeat"),
            Self::ListSum => d("List", "sum", 1, Pure, "list_sum"),
            Self::ListProduct => d("List", "product", 1, Pure, "list_product"),
            Self::ListMaximum => d("List", "maximum", 1, Pure, "list_maximum"),
            Self::ListMinimum => d("List", "minimum", 1, Pure, "list_minimum"),
            Self::ListIntersperse => d("List", "intersperse", 2, Pure, "list_intersperse"),
            Self::ListPartition => d("List", "partition", 2, Pure, "list_partition"),
            Self::ListUnzip => d("List", "unzip", 1, Pure, "list_unzip"),
            Self::ListMap2 => d("List", "map2", 3, Pure, "list_map2"),
            Self::ListMap3 => d("List", "map3", 4, Pure, "list_map3"),
            Self::ListMap4 => d("List", "map4", 5, Pure, "list_map4"),
            Self::ListMap5 => d("List", "map5", 6, Pure, "list_map5"),
            Self::BasicsNot => d("Basics", "not", 1, Pure, "basics_not"),
            Self::BasicsIdentity => d("Basics", "identity", 1, Pure, "basics_identity"),
            Self::BasicsAlways => d("Basics", "always", 2, Pure, "basics_always"),
            Self::BasicsFst => d("Basics", "fst", 1, Pure, "basics_fst"),
            Self::BasicsSnd => d("Basics", "snd", 1, Pure, "basics_snd"),
            Self::BasicsModBy => d("Basics", "modBy", 2, Pure, "basics_mod_by"),
            Self::BasicsClamp => d("Basics", "clamp", 3, Pure, "basics_clamp"),
            Self::BasicsToString => d("Basics", "toString", 1, Pure, "basics_to_string"),
            // ── Basics numerics ──────────────────────────────────────────
            Self::BasicsNegate => d("Basics", "negate", 1, Pure, "basics_negate"),
            Self::BasicsAbs => d("Basics", "abs", 1, Pure, "basics_abs"),
            Self::BasicsSqrt => d("Basics", "sqrt", 1, Pure, "math_sqrt"),
            Self::BasicsMin => d("Basics", "min", 2, Pure, "math_min"),
            Self::BasicsMax => d("Basics", "max", 2, Pure, "math_max"),
            Self::BasicsCompare => d("Basics", "compare", 2, Pure, "basics_compare"),
            // ── end Basics numerics ──────────────────────────────────────
            // ── Error (Ipe.Error — real Error/ErrorKind ADT) ──
            // Each message constructor classifies its own `ErrorKind` at
            // construction (`ipe_runtime::error::IpeError`, no longer a
            // shared string-identity). `toString` reuses the existing
            // `errorToString` runtime (`basics_error_to_string`).
            Self::ErrorUnexpected => d("Error", "unexpected", 1, Pure, "ipe_error_unexpected"),
            Self::ErrorInvalidInput => {
                d("Error", "invalidInput", 1, Pure, "ipe_error_invalid_input")
            }
            Self::ErrorIo => d("Error", "io", 1, Pure, "ipe_error_io"),
            Self::ErrorNetwork => d("Error", "network", 1, Pure, "ipe_error_network"),
            Self::ErrorFfi => d("Error", "ffi", 1, Pure, "ipe_error_ffi"),
            Self::ErrorDecode => d("Error", "decode", 1, Pure, "ipe_error_decode"),
            Self::ErrorConflict => d("Error", "conflict", 1, Pure, "ipe_error_conflict"),
            Self::ErrorUnavailable => d("Error", "unavailable", 1, Pure, "ipe_error_unavailable"),
            Self::ErrorTimeout => d("Error", "timeout", 0, Pure, "ipe_error_timeout"),
            Self::ErrorNotFound => d("Error", "notFound", 0, Pure, "ipe_error_not_found"),
            Self::ErrorPermissionDenied => d(
                "Error",
                "permissionDenied",
                0,
                Pure,
                "ipe_error_permission_denied",
            ),
            Self::ErrorToString => d("Error", "toString", 1, Pure, "basics_error_to_string"),
            Self::ErrorWithMessage => d("Error", "withMessage", 2, Pure, "ipe_error_with_message"),
            Self::ErrorIsRetryable => d("Error", "isRetryable", 1, Pure, "ipe_error_is_retryable"),
            Self::ErrorWithDetails => d("Error", "withDetails", 2, Pure, "ipe_error_with_details"),
            Self::ErrorKind => d("Error", "kind", 1, Pure, "ipe_error_kind"),
            Self::ErrorMessage => d("Error", "message", 1, Pure, "ipe_error_message"),
            Self::ErrorKindName => d("Error", "kindName", 1, Pure, "ipe_error_kind_name"),
            // ── CssSafety (Ipe.CssSafety — Ipe.Css leaf kernels) ────
            // The `emit` symbols are the bare runtime fn names re-exported at the
            // `ipe_runtime` root (`pub use css::*`): `safe_value` /
            // `safe_prop_name` / `safe_selector` / `strip_style_close_kernel`.
            Self::CssSafetySafeValue => d("CssSafety", "safeValue", 1, Pure, "safe_value"),
            Self::CssSafetySafePropName => {
                d("CssSafety", "safePropName", 1, Pure, "safe_prop_name")
            }
            Self::CssSafetySafeSelector => d("CssSafety", "safeSelector", 1, Pure, "safe_selector"),
            Self::CssSafetyStripStyleClose => d(
                "CssSafety",
                "stripStyleClose",
                1,
                Pure,
                "strip_style_close_kernel",
            ),
            // ── Maybe ───────────────────────────────────────────────────────
            Self::MaybeWithDefault => d("Maybe", "withDefault", 2, Pure, "maybe_with_default"),
            Self::MaybeMap => d("Maybe", "map", 2, Pure, "ipe_maybe_map"),
            Self::MaybeAndThen => d("Maybe", "andThen", 2, Pure, "ipe_maybe_and_then"),
            // `mapN` arity = 1 (fn) + N containers; `andMap` = 2; `combine` = 1.
            Self::MaybeMap2 => d("Maybe", "map2", 3, Pure, "maybe_map2"),
            Self::MaybeMap3 => d("Maybe", "map3", 4, Pure, "maybe_map3"),
            Self::MaybeMap4 => d("Maybe", "map4", 5, Pure, "maybe_map4"),
            Self::MaybeMap5 => d("Maybe", "map5", 6, Pure, "maybe_map5"),
            Self::MaybeAndMap => d("Maybe", "andMap", 2, Pure, "maybe_and_map"),
            Self::MaybeCombine => d("Maybe", "combine", 1, Pure, "maybe_combine"),
            // ── Result ──────────────────────────────────────────────────────
            Self::ResultWithDefault => d("Result", "withDefault", 2, Pure, "result_with_default"),
            Self::ResultMap => d("Result", "map", 2, Pure, "ipe_result_map"),
            Self::ResultAndThen => d("Result", "andThen", 2, Pure, "ipe_result_and_then"),
            Self::ResultMapError => d("Result", "mapError", 2, Pure, "ipe_result_map_error"),
            Self::ResultMap2 => d("Result", "map2", 3, Pure, "result_map2"),
            Self::ResultMap3 => d("Result", "map3", 4, Pure, "result_map3"),
            Self::ResultMap4 => d("Result", "map4", 5, Pure, "result_map4"),
            Self::ResultMap5 => d("Result", "map5", 6, Pure, "result_map5"),
            Self::ResultAndMap => d("Result", "andMap", 2, Pure, "result_and_map"),
            Self::ResultCombine => d("Result", "combine", 1, Pure, "result_combine"),
            Self::ResultTraverse => d("Result", "traverse", 2, Pure, "result_traverse"),
            Self::ResultToMaybe => d("Result", "toMaybe", 1, Pure, "ipe_result_to_maybe"),
            Self::ResultFromMaybe => d("Result", "fromMaybe", 2, Pure, "ipe_result_from_maybe"),
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
            Self::MathIsNaN => d("Math", "isNaN", 1, Pure, "math_is_nan"),
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
            Self::DictSingleton => d("Dict", "singleton", 2, Pure, "dict_singleton"),
            Self::DictFoldr => d("Dict", "foldr", 3, Pure, "dict_foldr"),
            Self::DictFilter => d("Dict", "filter", 2, Pure, "dict_filter"),
            Self::DictPartition => d("Dict", "partition", 2, Pure, "dict_partition"),
            Self::DictIntersect => d("Dict", "intersect", 2, Pure, "dict_intersect"),
            Self::DictDiff => d("Dict", "diff", 2, Pure, "dict_diff"),
            Self::DictUpdate => d("Dict", "update", 3, Pure, "dict_update"),
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
            Self::SetIsEmpty => d("Set", "isEmpty", 1, Pure, "set_is_empty"),
            Self::SetSingleton => d("Set", "singleton", 1, Pure, "set_singleton"),
            Self::SetFoldl => d("Set", "foldl", 3, Pure, "set_foldl"),
            Self::SetFoldr => d("Set", "foldr", 3, Pure, "set_foldr"),
            Self::SetMap => d("Set", "map", 2, Pure, "set_map"),
            Self::SetFilter => d("Set", "filter", 2, Pure, "set_filter"),
            Self::SetPartition => d("Set", "partition", 2, Pure, "set_partition"),
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
                d("Encoding", "base64Decode", 1, Pure, "ipe_base64_decode")
            }
            Self::EncodingUrlEncode => d("Encoding", "urlEncode", 1, Pure, "url_encode"),
            Self::EncodingUrlDecode => d("Encoding", "urlDecode", 1, Pure, "ipe_url_decode"),
            Self::EncodingHexEncode => d("Encoding", "hexEncode", 1, Pure, "encoding_hex_encode"),
            Self::EncodingHexDecode => {
                d("Encoding", "hexDecode", 1, Pure, "ipe_encoding_hex_decode")
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
                "ipe_crypto_rsa_sha256_sign",
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
            // (`ipe_aes_gcm_encrypt(key, plaintext)` etc.) prepends/strips a
            // fresh random nonce internally, so — unlike the Go backend which
            // took an explicit nonce/AAD arg — there is no third argument.
            Self::CryptoAesGcmEncrypt => {
                d("Crypto", "aesGcmEncrypt", 2, Pure, "ipe_aes_gcm_encrypt")
            }
            Self::CryptoAesGcmDecrypt => {
                d("Crypto", "aesGcmDecrypt", 2, Pure, "ipe_aes_gcm_decrypt")
            }
            Self::CryptoChacha20Encrypt => {
                d("Crypto", "chacha20Encrypt", 2, Pure, "ipe_chacha20_encrypt")
            }
            Self::CryptoChacha20Decrypt => {
                d("Crypto", "chacha20Decrypt", 2, Pure, "ipe_chacha20_decrypt")
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
            // `v4`/`v7` are EFFECT-tier (`() -> Task Error String`):
            // entropy is not a memoizable pure `String`. Arity is 1 (the unit
            // argument) so the FIRST_SCHEMED `arrow-count == decl().arity`
            // invariant holds against the `fun(Unit, task(string))` scheme.
            // Runtime `uuid_v4::<E>(_: ())` / `uuid_v7::<E>(_: ())` take that unit.
            Self::UuidV4 => d("Uuid", "v4", 1, Pure, "uuid_v4"),
            Self::UuidV7 => d("Uuid", "v7", 1, Pure, "uuid_v7"),
            Self::UuidParse => d("Uuid", "parse", 1, Pure, "uuid_parse"),
            // ── Jwt ─────────────────────────────────────────────────────────
            // Encode arity is 2 (secret/key, claims_json): the Rust runtime
            // `ipe_jwt_encode_hs256(secret, claims_json)` / `_rs256(key_pem,
            // claims_json)` take exactly two args.
            Self::JwtEncodeHs256 => d("Jwt", "encodeHs256", 2, Pure, "ipe_jwt_encode_hs256"),
            Self::JwtDecodeHs256 => d("Jwt", "decodeHs256", 2, Pure, "ipe_jwt_decode_hs256"),
            Self::JwtEncodeRs256 => d("Jwt", "encodeRs256", 2, Pure, "ipe_jwt_encode_rs256"),
            Self::JwtDecodeRs256 => d("Jwt", "decodeRs256", 2, Pure, "ipe_jwt_decode_rs256"),
            // ── Jwt builder API ──────────────────────────────────
            Self::JwtClaims => d("Jwt", "claims", 0, Pure, "ipe_jwt_claims"),
            Self::JwtHs256 => d("Jwt", "hs256", 1, Pure, "ipe_jwt_hs256"),
            Self::JwtRs256 => d("Jwt", "rs256", 1, Pure, "ipe_jwt_rs256"),
            Self::JwtSubject => d("Jwt", "subject", 2, Pure, "ipe_jwt_subject"),
            Self::JwtIssuer => d("Jwt", "issuer", 2, Pure, "ipe_jwt_issuer"),
            Self::JwtAudience => d("Jwt", "audience", 2, Pure, "ipe_jwt_audience"),
            Self::JwtExpiresAt => d("Jwt", "expiresAt", 2, Pure, "ipe_jwt_expires_at"),
            Self::JwtNotBefore => d("Jwt", "notBefore", 2, Pure, "ipe_jwt_not_before"),
            Self::JwtIssuedAt => d("Jwt", "issuedAt", 2, Pure, "ipe_jwt_issued_at"),
            Self::JwtJwtId => d("Jwt", "jwtId", 2, Pure, "ipe_jwt_jwt_id"),
            Self::JwtWithClaim => d("Jwt", "withClaim", 3, Pure, "ipe_jwt_with_claim"),
            Self::JwtEncode => d("Jwt", "encode", 2, Pure, "ipe_jwt_encode"),
            Self::JwtDecode => d("Jwt", "decode", 3, Pure, "ipe_jwt_decode"),
            // ── Task combinators ────────────────────────────────────────────
            Self::TaskSucceed => d("Task", "succeed", 1, Pure, "task_succeed"),
            Self::TaskFail => d("Task", "fail", 1, Pure, "task_fail"),
            Self::TaskMap => d("Task", "map", 2, Pure, "task_map"),
            Self::TaskMap2 => d("Task", "map2", 3, Pure, "task_map2"),
            Self::TaskMap3 => d("Task", "map3", 4, Pure, "task_map3"),
            Self::TaskMap4 => d("Task", "map4", 5, Pure, "task_map4"),
            Self::TaskMap5 => d("Task", "map5", 6, Pure, "task_map5"),
            Self::TaskAttempt => d("Task", "attempt", 2, Tea, "cmd_perform"),
            Self::TaskAndThen => d("Task", "andThen", 2, Pure, "task_and_then"),
            Self::TaskMapError => d("Task", "mapError", 2, Pure, "task_map_error"),
            Self::TaskOnError => d("Task", "onError", 2, Pure, "task_on_error"),
            Self::TaskFromResult => d("Task", "fromResult", 1, Pure, "task_from_result"),
            Self::TaskAndThenResult => d("Task", "andThenResult", 2, Pure, "task_and_then_result"),
            Self::TaskSequence => d("Task", "sequence", 1, Pure, "task_sequence"),
            Self::TaskParallel => d("Task", "parallel", 1, Pure, "task_parallel"),
            Self::TaskRun => d("Task", "run", 1, Pure, "task_run"),
            Self::TaskPerform => d("Task", "perform", 1, Pure, "task_run"),
            Self::TaskLazy => d("Task", "lazy", 1, Pure, "task_lazy"),
            // ── Task retry surface (special-case emitter in emit_expr.rs) ───
            Self::TaskRetryWith => d("Task", "retryWith", 2, Pure, "task_retry_with"),
            Self::TaskLinearBackoff => d("Task", "linearBackoff", 2, Pure, "task_linear_backoff"),
            Self::TaskExponentialBackoff => d(
                "Task",
                "exponentialBackoff",
                2,
                Pure,
                "task_exponential_backoff",
            ),
            Self::TaskWithJitter => d("Task", "withJitter", 1, Pure, "task_with_jitter"),
            Self::TaskRetryOn => d("Task", "retryOn", 2, Pure, "task_retry_on"),
            Self::TaskWithRetryOn => d("Task", "withRetryOn", 2, Pure, "task_with_retry_on"),
            Self::TaskDefaultRetryPolicy => d(
                "Task",
                "defaultRetryPolicy",
                0,
                Pure,
                "task_default_retry_policy",
            ),
            Self::TaskWithMaxAttempts => {
                d("Task", "withMaxAttempts", 2, Pure, "task_with_max_attempts")
            }
            Self::TaskWithBaseMs => d("Task", "withBaseMs", 2, Pure, "task_with_base_ms"),
            Self::TaskWithKind => d("Task", "withKind", 2, Pure, "task_with_kind"),
            // ── Io ──────────────────────────────────────────────────────────
            Self::IoReadLine => d("Io", "readLine", 1, Pure, "io_read_line"),
            Self::IoWriteStdout => d("Io", "writeStdout", 1, Pure, "io_write_stdout"),
            Self::IoWriteStderr => d("Io", "writeStderr", 1, Pure, "io_write_stderr"),
            Self::IoPrintln => d("Io", "println", 1, Pure, "io_println"),
            Self::IoEprintln => d("Io", "eprintln", 1, Pure, "io_eprintln"),
            Self::DebugLog => d("Debug", "log", 2, Pure, "debug_log"),
            // ── Time (non-TEA) ──────────────────────────────────────────────
            Self::TimeNow => d("Time", "now", 1, Pure, "time_now"),
            Self::TimeSleep => d("Time", "sleep", 1, Pure, "time_sleep"),
            Self::TimeUnixMillis => d("Time", "unixMillis", 1, Pure, "time_unix_millis"),
            Self::TimeTimeString => d("Time", "timeString", 1, Pure, "time_time_string"),
            Self::TimeIsLeapYear => d("Time", "isLeapYear", 1, Pure, "time_is_leap_year"),
            Self::TimeDaysInMonth => d("Time", "daysInMonth", 2, Pure, "time_days_in_month"),
            // ── System ──────────────────────────────────────────────────────
            Self::SystemArgs => d("System", "args", 1, Pure, "system_args"),
            Self::SystemGetenv => d("System", "getenv", 1, Pure, "system_getenv"),
            Self::SystemGetenvOr => d("System", "getenvOr", 2, Pure, "system_getenv_or"),
            Self::SystemGetArg => d("System", "getArg", 1, Pure, "system_get_arg"),
            Self::SystemGetenvInt => d("System", "getenvInt", 1, Pure, "system_getenv_int"),
            Self::SystemGetenvBool => d("System", "getenvBool", 1, Pure, "system_getenv_bool"),
            Self::SystemSetenv => d("System", "setenv", 2, Pure, "system_setenv"),
            Self::SystemUnsetenv => d("System", "unsetenv", 1, Pure, "system_unsetenv"),
            Self::SystemCwd => d("System", "cwd", 1, Pure, "system_cwd"),
            Self::SystemLoadEnv => d("System", "loadEnv", 1, Pure, "system_load_env"),
            Self::SystemExit => d("System", "exit", 1, Pure, "system_exit"),
            // ── Random ──────────────────────────────────────────────────────
            Self::RandomInt => d("Random", "int", 2, Pure, "random_int"),
            Self::RandomFloat => d("Random", "float", 2, Pure, "random_float"),
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
            // ── Process ───────────────────────────────────────────────────────
            Self::ProcessRun => d("Process", "run", 2, Pure, "process_run"),
            // ── Http ────────────────────────────────────────────────────────
            Self::HttpGet => d("Http", "get", 1, Pure, "http_get"),
            Self::HttpPost => d("Http", "post", 2, Pure, "http_post"),
            Self::HttpRequest => d("Http", "request", 1, Pure, "http_request"),
            Self::HttpParseQuery => d("Http", "parseQuery", 1, Pure, "http_parse_query"),
            Self::HttpDefaultRequest => {
                d("Http", "defaultRequest", 1, Pure, "http_default_request")
            }
            Self::HttpWithMethod => d("Http", "withMethod", 2, Pure, "http_with_method"),
            Self::HttpWithTimeout => d("Http", "withTimeout", 2, Pure, "http_with_timeout"),
            Self::HttpWithBody => d("Http", "withBody", 2, Pure, "http_with_body"),
            Self::HttpWithHeader => d("Http", "withHeader", 3, Pure, "http_with_header"),
            Self::HttpWithUrl => d("Http", "withUrl", 2, Pure, "http_with_url"),
            Self::HttpWithFollowRedirects => d(
                "Http",
                "withFollowRedirects",
                2,
                Pure,
                "http_with_follow_redirects",
            ),
            Self::HttpWithMaxRedirects => d(
                "Http",
                "withMaxRedirects",
                2,
                Pure,
                "http_with_max_redirects",
            ),
            Self::HttpMethodFromString => d(
                "Http",
                "methodFromString",
                1,
                Pure,
                "http_method_from_string",
            ),
            Self::HttpMethodToString => {
                d("Http", "methodToString", 1, Pure, "http_method_to_string")
            }
            // ── Db ──────────────────────────────────────────────────────────
            Self::DbConnect => d("Db", "connect", 1, Db, "db_connect"),
            Self::DbOpen => d("Db", "open", 2, Db, "db_open"),
            Self::DbClose => d("Db", "close", 1, Db, "db_close"),
            Self::DbExecRaw => d("Db", "execRaw", 2, Db, "db_exec_raw"),
            Self::DbExec => d("Db", "exec", 3, Db, "db_exec_params"),
            Self::DbQuery => d("Db", "query", 3, Db, "db_query_params"),
            Self::DbQueryDecode => d("Db", "queryDecode", 4, Db, "db_query_decode_params"),
            Self::DbGetString => d("Db", "getString", 2, Db, "db_get_string"),
            Self::DbGetInt => d("Db", "getInt", 2, Db, "db_get_int"),
            Self::DbGetBool => d("Db", "getBool", 2, Db, "db_get_bool"),
            Self::DbGetField => d("Db", "getField", 2, Db, "db_get_field"),
            Self::DbInsertRow => d("Db", "insertRow", 3, Db, "db_insert_row"),
            Self::DbGetById => d("Db", "getById", 3, Db, "db_get_by_id"),
            Self::DbUpdateById => d("Db", "updateById", 4, Db, "db_update_by_id"),
            Self::DbDeleteById => d("Db", "deleteById", 3, Db, "db_delete_by_id"),
            Self::DbFindOneByField => d("Db", "findOneByField", 4, Db, "db_find_one_by_field"),
            Self::DbFindManyByField => d("Db", "findManyByField", 4, Db, "db_find_many_by_field"),
            Self::DbFindByConditions => d("Db", "findByConditions", 3, Db, "db_find_by_conditions"),
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
            // Pure record builder — emitted inline as a `Migration` struct
            // literal (see the `DbDefaultMigration` arm in `emit_expr`), so the
            // runtime-fn name is a never-called placeholder.
            Self::DbDefaultMigration => {
                d("Db", "defaultMigration", 1, Pure, "db_default_migration")
            }
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
            Self::DbDecMoney => d("Db.Decode", "money", 1, Db, "db_decode_money"),
            Self::DbDecBytes => d("Db.Decode", "bytes", 1, Db, "db_decode_bytes"),
            // ── TEA: Cmd / Sub / Time.every ─────────────────────────────────
            Self::CmdNone => d("Cmd", "none", 0, Tea, "cmd_none"),
            Self::CmdBatch => d("Cmd", "batch", 1, Tea, "cmd_batch"),
            Self::CmdPerform => d("Cmd", "perform", 2, Tea, "cmd_perform"),
            Self::CmdMap => d("Cmd", "map", 2, Tea, "cmd_map"),
            Self::SubNone => d("Sub", "none", 0, Tea, "sub_none"),
            Self::SubBatch => d("Sub", "batch", 1, Tea, "sub_batch"),
            Self::SubEvery => d("Sub", "every", 2, Tea, "sub_every"),
            Self::TimeEvery => d("Time", "every", 2, Tea, "time_every"),
            Self::SubMap => d("Sub", "map", 2, Tea, "sub_map"),
            // ── TEA: reserved pub/sub ────────────────────────────────────────
            // Qualifier "Cmd" IS in qual_vars but "publish"/"publishNoEcho" are
            // NOT yet. Absent from ALL until wired; decl() is still exhaustive.
            Self::CmdPublish => d("Cmd", "publish", 2, Tea, "cmd_publish"),
            Self::CmdPublishNoEcho => d("Cmd", "publishNoEcho", 2, Tea, "cmd_publish_no_echo"),
            // Qualifier "Sub" IS in qual_vars but "subscribeTopic" is NOT yet.
            Self::SubSubscribeTopic => d("Sub", "subscribeTopic", 2, Tea, "sub_subscribe_topic"),
            // `Ipe.PubSub` is the Task-shaped top-level publish surface — NOT
            // TEA-loop machinery. `class = Web` because its runtime symbols live
            // in `ipe_runtime::live::pubsub` (the Web/live module), the same home
            // as `Web.renderStatic`; it is excluded from `is_tea()` so it never
            // pulls in the `Cmd`/`Sub` (`tea` module) aliases. `Ipe.PubSub` is a
            // compiled-source module, so `Ipe.PubSub.publish` resolves through its
            // `Ffi.kernel "PubSub_publish"` alias to this `("PubSub", "publish")`
            // canonical kernel — the `"PubSub"` qualifier is intentionally NOT in
            // canon `QUALIFIERS` (compiled-source, not a kernel qualifier).
            Self::PubSubPublish => d("PubSub", "publish", 2, Web, "pubsub_publish"),
            Self::PubSubPublishNoEcho => {
                d("PubSub", "publishNoEcho", 2, Web, "pubsub_publish_no_echo")
            }
            // ── Ipe.Http.Server / Middleware / RateLimit ─────────────────────
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
            Self::ServerCookieNew => d("Server", "cookie", 2, Server, "server_cookie"),
            Self::ServerWithCookie => d("Server", "withCookie", 2, Server, "server_with_cookie"),
            Self::MiddlewareWithCors => {
                d("Middleware", "withCors", 2, Server, "middleware_with_cors")
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
                3,
                Server,
                "middleware_with_basic_auth",
            ),
            Self::MiddlewareWithRateLimit => d(
                "Middleware",
                "withRateLimit",
                4,
                Server,
                "middleware_with_rate_limit",
            ),
            Self::MiddlewareWithCsrf => {
                d("Middleware", "withCsrf", 1, Server, "middleware_with_csrf")
            }
            Self::RateLimitAllow => d("RateLimit", "allow", 4, Server, "rate_limit_allow"),
            // ── Ipe.Ui / Ipe.Html render kernels ─────────────────────────
            Self::UiLayout => d("Ui", "layout", 2, Ui, "ui_layout"),
            Self::UiLayoutWith => d("Ui", "layoutWith", 2, Ui, "ui_layout_with"),
            Self::HtmlRender => d("Html", "render", 1, Ui, "html_render_"),
            Self::HtmlEscapeText => d("Html", "escapeHtml", 1, Ui, "html_escape_text_"),
            Self::HtmlEscapeAttr => d("Html", "escapeAttr", 1, Ui, "html_escape_attr_"),
            Self::HtmlAttrToString => d("Html", "attrToString", 1, Ui, "html_attr_to_string_"),
            // ── Ipe.Ui element builders ──────────────────────────────────
            Self::UiNone => d("Ui", "none", 0, Ui, "ui_none_"),
            Self::UiText => d("Ui", "text", 1, Ui, "ui_text_"),
            Self::UiHtml => d("Ui", "html", 1, Ui, "ui_html_"),
            Self::UiCells => d("Ui", "cells", 1, Ui, "ui_cells_"),
            Self::UiEl => d("Ui", "el", 2, Ui, "ui_el_"),
            Self::UiRow => d("Ui", "row", 2, Ui, "ui_row_"),
            Self::UiColumn => d("Ui", "column", 2, Ui, "ui_column_"),
            Self::UiWrappedRow => d("Ui", "wrappedRow", 2, Ui, "ui_wrapped_row_"),
            Self::UiGrid => d("Ui", "grid", 2, Ui, "ui_grid_"),
            Self::UiParagraph => d("Ui", "paragraph", 2, Ui, "ui_paragraph_"),
            Self::UiTextColumn => d("Ui", "textColumn", 2, Ui, "ui_text_column_"),
            Self::UiButton => d("Ui", "button", 2, Ui, "ui_button_"),
            Self::UiLink => d("Ui", "link", 2, Ui, "ui_link_"),
            Self::UiForm => d("Ui", "form", 2, Ui, "ui_form_"),
            Self::UiImage => d("Ui", "image", 2, Ui, "ui_image_"),
            // ── Ipe.Ui nearby attribute builders ───────────────────────
            Self::UiAbove => d("Ui", "above", 1, Ui, "ui_above_"),
            Self::UiBelow => d("Ui", "below", 1, Ui, "ui_below_"),
            Self::UiOnLeft => d("Ui", "onLeft", 1, Ui, "ui_on_left_"),
            Self::UiOnRight => d("Ui", "onRight", 1, Ui, "ui_on_right_"),
            Self::UiInFront => d("Ui", "inFront", 1, Ui, "ui_in_front_"),
            Self::UiBehind => d("Ui", "behind", 1, Ui, "ui_behind_"),
            // ── Ipe.Ui attribute builders ────────────────────────────────
            Self::UiSpacing => d("Ui", "spacing", 1, Ui, "ui_spacing_"),
            Self::UiPadding => d("Ui", "padding", 1, Ui, "ui_padding_"),
            Self::UiPaddingXY => d("Ui", "paddingXY", 2, Ui, "ui_padding_xy_"),
            Self::UiPaddingEach => d("Ui", "paddingEach", 1, Ui, "ui_padding_each_"),
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
            Self::UiClipX => d("Ui", "clipX", 0, Ui, "ui_clip_x_"),
            Self::UiClipY => d("Ui", "clipY", 0, Ui, "ui_clip_y_"),
            Self::UiScrollbars => d("Ui", "scrollbars", 0, Ui, "ui_scrollbars_"),
            Self::UiScrollbarX => d("Ui", "scrollbarX", 0, Ui, "ui_scrollbar_x_"),
            Self::UiScrollbarY => d("Ui", "scrollbarY", 0, Ui, "ui_scrollbar_y_"),
            Self::UiGridColumns => d("Ui", "gridColumns", 1, Ui, "ui_grid_columns_"),
            // ── Ipe.Ui Length builders ───────────────────────────────────
            Self::UiPx => d("Ui", "px", 1, Ui, "ui_px_"),
            Self::UiFill => d("Ui", "fill", 0, Ui, "ui_fill_"),
            Self::UiContent => d("Ui", "content", 0, Ui, "ui_content_"),
            Self::UiShrink => d("Ui", "shrink", 0, Ui, "ui_shrink_"),
            Self::UiFillPortion => d("Ui", "fillPortion", 1, Ui, "ui_fill_portion_"),
            Self::UiVh => d("Ui", "vh", 1, Ui, "ui_vh_"),
            Self::UiVw => d("Ui", "vw", 1, Ui, "ui_vw_"),
            Self::UiMinimum => d("Ui", "minimum", 2, Ui, "ui_minimum_"),
            Self::UiMaximum => d("Ui", "maximum", 2, Ui, "ui_maximum_"),
            // ── Ipe.Ui Color builders ────────────────────────────────────
            Self::UiRgb => d("Ui", "rgb", 3, Ui, "ui_rgb_"),
            Self::UiRgba => d("Ui", "rgba", 4, Ui, "ui_rgba_"),
            Self::UiWhite => d("Ui", "white", 0, Ui, "ui_white_"),
            Self::UiBlack => d("Ui", "black", 0, Ui, "ui_black_"),
            Self::UiTransparent => d("Ui", "transparent", 0, Ui, "ui_transparent_"),
            Self::UiColorCss => d("Ui", "colorCss", 1, Ui, "ui_color_css_"),
            // ── Background / Border / Font sub-modules ───────────────────
            Self::BackgroundColor => d("Background", "color", 1, Ui, "ui_background_color_"),
            Self::BackgroundImage => d("Background", "image", 1, Ui, "ui_background_image_"),
            Self::BackgroundLinearGradient => d(
                "Background",
                "linearGradient",
                2,
                Ui,
                "ui_background_linear_gradient_",
            ),
            Self::BorderWidth => d("Border", "width", 1, Ui, "ui_border_width_"),
            Self::BorderRounded => d("Border", "rounded", 1, Ui, "ui_border_rounded_"),
            Self::BorderColor => d("Border", "color", 1, Ui, "ui_border_color_"),
            Self::BorderWidthEach => d("Border", "widthEach", 1, Ui, "ui_border_width_each_"),
            Self::BorderShadow => d("Border", "shadow", 1, Ui, "ui_border_shadow_"),
            Self::BorderGlow => d("Border", "glow", 2, Ui, "ui_border_glow_"),
            Self::BorderInnerShadow => d("Border", "innerShadow", 1, Ui, "ui_border_inner_shadow_"),
            Self::FontSize => d("Font", "size", 1, Ui, "ui_font_size_"),
            Self::FontColor => d("Font", "color", 1, Ui, "ui_font_color_"),
            Self::FontFamily => d("Font", "family", 1, Ui, "ui_font_family_"),
            Self::FontBold => d("Font", "bold", 0, Ui, "ui_font_bold_"),
            Self::FontItalic => d("Font", "italic", 0, Ui, "ui_font_italic_"),
            // ── Html element builders ────────────────────────────────────
            Self::HtmlTextNode => d("Html", "text", 1, Ui, "html_text_node_"),
            Self::HtmlRawNode => d("Html", "raw", 1, Ui, "html_raw_node_"),
            Self::HtmlNode => d("Html", "node", 3, Ui, "html_node_"),
            Self::HtmlVoidNode => d("Html", "voidNode", 2, Ui, "html_node_"),
            Self::HtmlDoctype => d("Html", "doctype", 1, Ui, "html_doctype_"),
            Self::HtmlTitleNode => d("Html", "titleNode", 1, Ui, "html_title_node_"),
            Self::HtmlToString => d("Html", "toString", 1, Ui, "html_render_"),
            Self::HtmlStyleNode => d("Html", "styleNode", 2, Ui, "html_style_node_"),
            // The tag is a baked literal, not a parameter — `html_div_` etc.
            // take (attrs, children) = 2, the void `html_input_`/`html_img_`
            // take (attrs) = 1. Runtime fn params AND lower `callee_arity`
            // (2/1) are the authorities for the decl arity.
            Self::HtmlDiv => d("Html", "div", 2, Ui, "html_div_"),
            Self::HtmlSpan => d("Html", "span", 2, Ui, "html_span_"),
            Self::HtmlA => d("Html", "a", 2, Ui, "html_a_"),
            Self::HtmlButton => d("Html", "button", 2, Ui, "html_button_"),
            Self::HtmlP => d("Html", "p", 2, Ui, "html_p_"),
            Self::HtmlInput => d("Html", "input", 1, Ui, "html_input_"),
            Self::HtmlImg => d("Html", "img", 1, Ui, "html_img_"),
            // ── Ipe.Html element builders (tag baked via decl name) ─
            Self::HtmlH1 => d("Html", "h1", 2, Ui, "html_node_"),
            Self::HtmlH2 => d("Html", "h2", 2, Ui, "html_node_"),
            Self::HtmlH3 => d("Html", "h3", 2, Ui, "html_node_"),
            Self::HtmlH4 => d("Html", "h4", 2, Ui, "html_node_"),
            Self::HtmlH5 => d("Html", "h5", 2, Ui, "html_node_"),
            Self::HtmlH6 => d("Html", "h6", 2, Ui, "html_node_"),
            Self::HtmlNav => d("Html", "nav", 2, Ui, "html_node_"),
            Self::HtmlSection => d("Html", "section", 2, Ui, "html_node_"),
            Self::HtmlArticle => d("Html", "article", 2, Ui, "html_node_"),
            Self::HtmlHeader => d("Html", "header", 2, Ui, "html_node_"),
            Self::HtmlHeaderNode => d("Html", "headerNode", 2, Ui, "html_node_"),
            Self::HtmlCodeNode => d("Html", "codeNode", 2, Ui, "html_node_"),
            Self::HtmlMainNode => d("Html", "mainNode", 2, Ui, "html_node_"),
            Self::HtmlFooterNode => d("Html", "footerNode", 2, Ui, "html_node_"),
            Self::HtmlLinkNode => d("Html", "linkNode", 1, Ui, "html_node_"),
            Self::HtmlFooter => d("Html", "footer", 2, Ui, "html_node_"),
            Self::HtmlMain => d("Html", "main", 2, Ui, "html_node_"),
            Self::HtmlAside => d("Html", "aside", 2, Ui, "html_node_"),
            Self::HtmlUl => d("Html", "ul", 2, Ui, "html_node_"),
            Self::HtmlOl => d("Html", "ol", 2, Ui, "html_node_"),
            Self::HtmlLi => d("Html", "li", 2, Ui, "html_node_"),
            Self::HtmlTable => d("Html", "table", 2, Ui, "html_node_"),
            Self::HtmlThead => d("Html", "thead", 2, Ui, "html_node_"),
            Self::HtmlTbody => d("Html", "tbody", 2, Ui, "html_node_"),
            Self::HtmlTfoot => d("Html", "tfoot", 2, Ui, "html_node_"),
            Self::HtmlTr => d("Html", "tr", 2, Ui, "html_node_"),
            Self::HtmlTh => d("Html", "th", 2, Ui, "html_node_"),
            Self::HtmlTd => d("Html", "td", 2, Ui, "html_node_"),
            Self::HtmlTextarea => d("Html", "textarea", 2, Ui, "html_node_"),
            Self::HtmlSelect => d("Html", "select", 2, Ui, "html_node_"),
            Self::HtmlOption => d("Html", "option", 2, Ui, "html_node_"),
            Self::HtmlLabel => d("Html", "label", 2, Ui, "html_node_"),
            Self::HtmlForm => d("Html", "form", 2, Ui, "html_node_"),
            Self::HtmlFieldset => d("Html", "fieldset", 2, Ui, "html_node_"),
            Self::HtmlLegend => d("Html", "legend", 2, Ui, "html_node_"),
            Self::HtmlPre => d("Html", "pre", 2, Ui, "html_node_"),
            Self::HtmlCode => d("Html", "code", 2, Ui, "html_node_"),
            Self::HtmlStrong => d("Html", "strong", 2, Ui, "html_node_"),
            Self::HtmlEm => d("Html", "em", 2, Ui, "html_node_"),
            Self::HtmlSmall => d("Html", "small", 2, Ui, "html_node_"),
            Self::HtmlBlockquote => d("Html", "blockquote", 2, Ui, "html_node_"),
            Self::HtmlFigure => d("Html", "figure", 2, Ui, "html_node_"),
            Self::HtmlFigcaption => d("Html", "figcaption", 2, Ui, "html_node_"),
            Self::HtmlDetails => d("Html", "details", 2, Ui, "html_node_"),
            Self::HtmlSummary => d("Html", "summary", 2, Ui, "html_node_"),
            Self::HtmlDialog => d("Html", "dialog", 2, Ui, "html_node_"),
            Self::HtmlVideo => d("Html", "video", 2, Ui, "html_node_"),
            Self::HtmlAudio => d("Html", "audio", 2, Ui, "html_node_"),
            Self::HtmlCanvas => d("Html", "canvas", 2, Ui, "html_node_"),
            Self::HtmlIframe => d("Html", "iframe", 2, Ui, "html_node_"),
            Self::HtmlProgress => d("Html", "progress", 2, Ui, "html_node_"),
            Self::HtmlMeter => d("Html", "meter", 2, Ui, "html_node_"),
            Self::HtmlScript => d("Html", "script", 2, Ui, "html_node_"),
            Self::HtmlBody => d("Html", "body", 2, Ui, "html_node_"),
            Self::HtmlTitle => d("Html", "title", 2, Ui, "html_node_"),
            Self::HtmlHtmlNode => d("Html", "htmlNode", 2, Ui, "html_node_"),
            Self::HtmlHeadNode => d("Html", "headNode", 2, Ui, "html_node_"),
            Self::HtmlBr => d("Html", "br", 1, Ui, "html_node_"),
            Self::HtmlHr => d("Html", "hr", 1, Ui, "html_node_"),
            Self::HtmlMeta => d("Html", "meta", 1, Ui, "html_node_"),
            Self::HtmlLink => d("Html", "link", 1, Ui, "html_node_"),
            Self::HtmlArea => d("Html", "area", 1, Ui, "html_node_"),
            Self::HtmlBase => d("Html", "base", 1, Ui, "html_node_"),
            Self::HtmlCol => d("Html", "col", 1, Ui, "html_node_"),
            Self::HtmlEmbed => d("Html", "embed", 1, Ui, "html_node_"),
            Self::HtmlSource => d("Html", "source", 1, Ui, "html_node_"),
            Self::HtmlTrack => d("Html", "track", 1, Ui, "html_node_"),
            Self::HtmlWbr => d("Html", "wbr", 1, Ui, "html_node_"),
            // ── Ipe.Html.Attributes builders ────────────────────────────
            // Qualifier "Attr" matches the `QUALIFIERS` table in env.rs. Emit
            // routes through the two generic runtime helpers; the fixed key is
            // supplied by the emit arm (see `html_attr_key`).
            Self::HtmlAttrClass => d("Attr", "class", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrId => d("Attr", "id", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrHref => d("Attr", "href", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrSrc => d("Attr", "src", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrAlt => d("Attr", "alt", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrValue => d("Attr", "value", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrName => d("Attr", "name", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrPlaceholder => d("Attr", "placeholder", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrType => d("Attr", "type_", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrFor => d("Attr", "for_", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrStyle => d("Attr", "style", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrTitle => d("Attr", "title", 1, Ui, "html_named_attr_"),
            Self::HtmlAttrChecked => d("Attr", "checked", 1, Ui, "html_bool_named_attr_"),
            Self::HtmlAttrDisabled => d("Attr", "disabled", 1, Ui, "html_bool_named_attr_"),
            Self::HtmlAttrReadonly => d("Attr", "readonly", 1, Ui, "html_bool_named_attr_"),
            Self::HtmlAttrRequired => d("Attr", "required", 1, Ui, "html_bool_named_attr_"),
            Self::HtmlAttrMultiple => d("Attr", "multiple", 1, Ui, "html_bool_named_attr_"),
            Self::HtmlAttrSelected => d("Attr", "selected", 1, Ui, "html_bool_named_attr_"),
            Self::HtmlAttrAutofocus => d("Attr", "autofocus", 1, Ui, "html_bool_named_attr_"),
            Self::HtmlAttrAutocomplete => d("Attr", "autocomplete", 1, Ui, "html_named_attr_"),
            Self::HtmlAttribute => d("Attr", "attribute", 2, Ui, "html_named_attr_"),
            Self::HtmlBoolAttribute => d("Attr", "boolAttribute", 2, Ui, "html_bool_named_attr_"),
            Self::HtmlNoAttr => d("Attr", "noAttr", 0, Ui, "html_no_attr_"),
            // ── Ipe.Web app-entry kernels ───────────────────────────────
            Self::WebApp => d("Web", "app", 1, Web, "web_app"),
            Self::WebAppRouted => d("Web", "appRouted", 1, Web, "web_app_routed"),
            Self::WebRoute => d("Web", "route", 2, Web, "web_route"),
            Self::WebRenderStatic => d("Web", "renderStatic", 2, Web, "web_render_static"),
            // ── Ipe.Terminal app-entry kernels ───────────────────────────
            Self::TerminalAppScreen => d("Terminal", "appScreen", 1, Terminal, "tui_app_ui"),
            // ── Ipe.WebView app-entry kernel ─────────────────────────────
            Self::WebViewApp => d("WebView", "app", 1, WebView, "webview_app"),
            // ── event-attribute builders ─────────────────────────────────
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
            Self::UiOnSubmit => d("Ui", "onSubmit", 1, Ui, "ui_on_submit_"),
            Self::UiOnFile => d("Ui", "onFile", 1, Ui, "ui_on_file_"),
            // ── Ipe.Html.Events builders (qualifier "Event" — matches the
            // `QUALIFIERS` table in env.rs). Each produces `html::Attribute<M>`
            // via a dedicated runtime constructor (family `Ui` so emit routes
            // through `emit_ui_call`). The emit arm supplies the fixed wire
            // event name; see `html_event_wire_name`.
            Self::HtmlOnClick => d("Event", "onClick", 1, Ui, "html_on_msg_"),
            Self::HtmlOnFocus => d("Event", "onFocus", 1, Ui, "html_on_msg_"),
            Self::HtmlOnBlur => d("Event", "onBlur", 1, Ui, "html_on_msg_"),
            Self::HtmlOnMouseOver => d("Event", "onMouseOver", 1, Ui, "html_on_msg_"),
            Self::HtmlOnMouseOut => d("Event", "onMouseOut", 1, Ui, "html_on_msg_"),
            Self::HtmlOnSubmit => d("Event", "onSubmit", 1, Ui, "html_on_raw_"),
            Self::HtmlOnInput => d("Event", "onInput", 1, Ui, "html_on_string_"),
            Self::HtmlOnChange => d("Event", "onChange", 1, Ui, "html_on_string_"),
            Self::HtmlOnKeyDown => d("Event", "onKeyDown", 1, Ui, "html_on_string_"),
            Self::HtmlOnKeyUp => d("Event", "onKeyUp", 1, Ui, "html_on_string_"),
            Self::HtmlOnBool => d("Event", "onBool", 1, Ui, "html_on_bool_"),
            // Ui namespace
            Self::UiSquare => d("Ui", "square", 0, Ui, "ui_square_"),
            Self::UiWidescreen => d("Ui", "widescreen", 0, Ui, "ui_widescreen_"),
            Self::UiCinemascope => d("Ui", "cinemascope", 0, Ui, "ui_cinemascope_"),
            Self::UiAspectRatio => d("Ui", "aspectRatio", 1, Ui, "ui_aspect_ratio_"),
            Self::UiAspectRatioWH => d("Ui", "aspectRatioWH", 2, Ui, "ui_aspect_ratio_wh_"),
            Self::UiHtmlAttribute => d("Ui", "htmlAttribute", 2, Ui, "ui_html_attribute_"),
            Self::UiName => d("Ui", "name", 1, Ui, "ui_name_"),
            Self::UiStyle => d("Ui", "style", 2, Ui, "ui_style_"),
            Self::UiTransitionRaw => d("Ui", "transitionRaw", 2, Ui, "ui_transition_raw_"),
            Self::UiGridTracksRaw => d("Ui", "gridTracksRaw", 2, Ui, "ui_grid_tracks_raw_"),
            Self::UiAnimateRaw => d("Ui", "animateRaw", 4, Ui, "ui_animate_raw_"),
            // Breakpoint
            Self::UiBreakpoint => d("Ui", "breakpoint", 3, Ui, "ui_breakpoint_"),
            Self::UiMediaQuery => d("Ui", "mediaQuery", 3, Ui, "ui_media_query_"),
            Self::UiMobile => d("Ui", "mobile", 0, Ui, "ui_mobile_"),
            Self::UiTablet => d("Ui", "tablet", 0, Ui, "ui_tablet_"),
            Self::UiDesktop => d("Ui", "desktop", 0, Ui, "ui_desktop_"),
            Self::UiDarkMode => d("Ui", "darkMode", 0, Ui, "ui_dark_mode_"),
            Self::UiLightMode => d("Ui", "lightMode", 0, Ui, "ui_light_mode_"),
            Self::UiReducedMotion => d("Ui", "reducedMotion", 0, Ui, "ui_reduced_motion_"),
            // PseudoClass opaque constants + Ui.onPseudo
            Self::UiOnPseudo => d("Ui", "onPseudo", 2, Ui, "ui_on_pseudo_"),
            Self::UiHover => d("Ui", "hover", 0, Ui, "ui_hover_"),
            Self::UiFocus => d("Ui", "focus", 0, Ui, "ui_focus_"),
            Self::UiFocusVisible => d("Ui", "focusVisible", 0, Ui, "ui_focus_visible_"),
            Self::UiActive => d("Ui", "active", 0, Ui, "ui_active_"),
            Self::UiDisabled => d("Ui", "disabled", 0, Ui, "ui_disabled_"),
            // Background namespace
            Self::BackgroundHoverColor => {
                d("Background", "hoverColor", 1, Ui, "ui_bg_hover_color_")
            }
            Self::BackgroundFocusColor => {
                d("Background", "focusColor", 1, Ui, "ui_bg_focus_color_")
            }
            Self::BackgroundActiveColor => {
                d("Background", "activeColor", 1, Ui, "ui_bg_active_color_")
            }
            Self::BackgroundDisabledColor => d(
                "Background",
                "disabledColor",
                1,
                Ui,
                "ui_bg_disabled_color_",
            ),
            // Border namespace
            Self::BorderSolid => d("Border", "solid", 0, Ui, "ui_border_solid_"),
            Self::BorderDashed => d("Border", "dashed", 0, Ui, "ui_border_dashed_"),
            Self::BorderDotted => d("Border", "dotted", 0, Ui, "ui_border_dotted_"),
            Self::BorderHoverColor => d("Border", "hoverColor", 1, Ui, "ui_border_hover_color_"),
            Self::BorderFocusColor => d("Border", "focusColor", 1, Ui, "ui_border_focus_color_"),
            Self::BorderActiveColor => d("Border", "activeColor", 1, Ui, "ui_border_active_color_"),
            Self::BorderHoverWidth => d("Border", "hoverWidth", 1, Ui, "ui_border_hover_width_"),
            Self::BorderHoverRounded => {
                d("Border", "hoverRounded", 1, Ui, "ui_border_hover_rounded_")
            }
            // Font namespace
            Self::FontWeight => d("Font", "weight", 1, Ui, "ui_font_weight_"),
            Self::FontSemiBold => d("Font", "semiBold", 0, Ui, "ui_font_semi_bold_"),
            Self::FontRegular => d("Font", "regular", 0, Ui, "ui_font_regular_"),
            Self::FontLight => d("Font", "light", 0, Ui, "ui_font_light_"),
            Self::FontExtraBold => d("Font", "extraBold", 0, Ui, "ui_font_extra_bold_"),
            Self::FontBlack => d("Font", "black", 0, Ui, "ui_font_black_"),
            Self::FontUnderline => d("Font", "underline", 0, Ui, "ui_font_underline_"),
            Self::FontNoDecoration => d("Font", "noDecoration", 0, Ui, "ui_font_no_decoration_"),
            Self::FontLineThrough => d("Font", "lineThrough", 0, Ui, "ui_font_line_through_"),
            Self::FontLetterSpacing => d("Font", "letterSpacing", 1, Ui, "ui_font_letter_spacing_"),
            Self::FontWordSpacing => d("Font", "wordSpacing", 1, Ui, "ui_font_word_spacing_"),
            Self::FontAlignLeft => d("Font", "alignLeft", 0, Ui, "ui_font_align_left_"),
            Self::FontAlignRight => d("Font", "alignRight", 0, Ui, "ui_font_align_right_"),
            Self::FontAlignCenter => d("Font", "alignCenter", 0, Ui, "ui_font_align_center_"),
            Self::FontCenter => d("Font", "center", 0, Ui, "ui_font_center_"),
            Self::FontJustify => d("Font", "justify", 0, Ui, "ui_font_justify_"),
            Self::FontSansSerif => d("Font", "sansSerif", 0, Ui, "ui_font_sans_serif_"),
            Self::FontSerif => d("Font", "serif", 0, Ui, "ui_font_serif_"),
            Self::FontMonospace => d("Font", "monospace", 0, Ui, "ui_font_monospace_"),
            Self::FontHoverColor => d("Font", "hoverColor", 1, Ui, "ui_font_hover_color_"),
            Self::FontFocusColor => d("Font", "focusColor", 1, Ui, "ui_font_focus_color_"),
            Self::FontActiveColor => d("Font", "activeColor", 1, Ui, "ui_font_active_color_"),
            Self::FontDisabledColor => d("Font", "disabledColor", 1, Ui, "ui_font_disabled_color_"),
            Self::FontHoverSize => d("Font", "hoverSize", 1, Ui, "ui_font_hover_size_"),
            // Html.Attributes
            Self::HtmlAttrTabindex => d("Attr", "tabindex", 1, Ui, "html_attr_tabindex_"),
            Self::HtmlAttrRows => d("Attr", "rows", 1, Ui, "html_attr_rows_"),
            // ── Effect stdlib modules ────────────────────────────────────
            // Ipe.Terminal line-oriented app-entry.
            Self::TerminalAppLines => d("Terminal", "appLines", 1, Terminal, "console_app"),
            // Ipe.Auth / Ipe.Auth (fail-closed: qual-registered only, no lower arm).
            Self::AuthHashPassword => d("Auth", "hashPassword", 1, Pure, "auth_hash_password"),
            Self::AuthHashPasswordCost => d(
                "Auth",
                "hashPasswordCost",
                2,
                Pure,
                "auth_hash_password_cost",
            ),
            Self::AuthVerifyPassword => {
                d("Auth", "verifyPassword", 2, Pure, "auth_verify_password")
            }
            Self::AuthPasswordStrength => d(
                "Auth",
                "passwordStrength",
                1,
                Pure,
                "auth_password_strength",
            ),
            Self::AuthSignToken => d("Auth", "signToken", 3, Pure, "auth_sign_token"),
            Self::AuthVerifyToken => d("Auth", "verifyToken", 2, Pure, "auth_verify_token"),
            Self::AuthRegister => d("Auth", "register", 3, Pure, "auth_register"),
            Self::AuthLogin => d("Auth", "login", 3, Pure, "auth_login"),
            Self::AuthSetRole => d("Auth", "setRole", 3, Pure, "auth_set_role"),
            // Ipe.Http.Server.Stream (fail-closed: qual-registered only, no lower arm).
            Self::StreamStream => d("Stream", "stream", 2, Server, "server_stream_stream"),
            Self::StreamEmit => d("Stream", "emit", 2, Server, "server_stream_emit"),
            Self::StreamFinish => d("Stream", "finish", 1, Server, "server_stream_finish"),
            Self::StreamWithContentType => d(
                "Stream",
                "withContentType",
                2,
                Server,
                "server_stream_with_content_type",
            ),
            // Ipe.Http.Stream (fail-closed: qual-registered only, no lower arm).
            Self::HttpStreamOpen => d("HttpStream", "open", 1, Pure, "http_stream_open"),
            Self::HttpStreamForEachChunk => d(
                "HttpStream",
                "forEachChunk",
                2,
                Pure,
                "http_stream_for_each_chunk",
            ),
            Self::HttpStreamClose => d("HttpStream", "close", 1, Pure, "http_stream_close"),
            Self::HttpStreamChunks => d("HttpStream", "chunks", 2, Pure, "sub_subscribe_stream"),
            // ── Ipe.Http.Server.WebSocket (12 kernels) ─────────────────────
            Self::WsDefaultCfg => d("Ws", "defaultCfg", 0, Server, "ws_server_default_cfg"),
            Self::WsWithOnConnect => d(
                "Ws",
                "withOnConnect",
                2,
                Server,
                "ws_server_with_on_connect",
            ),
            Self::WsWithOnMessage => d(
                "Ws",
                "withOnMessage",
                2,
                Server,
                "ws_server_with_on_message",
            ),
            Self::WsWithOnClose => d("Ws", "withOnClose", 2, Server, "ws_server_with_on_close"),
            Self::WsWithOnError => d("Ws", "withOnError", 2, Server, "ws_server_with_on_error"),
            Self::WsWithMaxMessageBytes => d(
                "Ws",
                "withMaxMessageBytes",
                2,
                Server,
                "ws_server_with_max_message_bytes",
            ),
            Self::WsWithOriginPatterns => d(
                "Ws",
                "withOriginPatterns",
                2,
                Server,
                "ws_server_with_origin_patterns",
            ),
            Self::WsUpgrade => d("Ws", "upgrade", 2, Server, "server_web_socket_upgrade"),
            Self::WsSendToClient => d("Ws", "sendToClient", 2, Server, "ws_server_send_to_client"),
            Self::WsSendBinaryToClient => d(
                "Ws",
                "sendBinaryToClient",
                2,
                Server,
                "ws_server_send_binary_to_client",
            ),
            Self::WsBroadcast => d("Ws", "broadcast", 2, Server, "ws_server_broadcast"),
            Self::WsCloseClient => d("Ws", "closeClient", 1, Server, "ws_server_close_client"),
            // ── Ipe.WebSocket — outbound WebSocket client (7 kernels) ──
            // The Task-tier six are `Pure`-classed (plain effects, default N-arg
            // emit like `Http.get`); the runtime fns live in `ws_client.rs`
            // (gated by the `websocket_client` feature the backend adds via the
            // `uses_websocket` flag). `Sub_subscribeWebSocket` is `Tea`-classed —
            // the backend's `emit_tea_call` peephole splits it on the literal
            // `kind` into the four typed `sub_subscribe_ws_*` runtime fns.
            Self::WebSocketConnect => d("WebSocket", "connect", 1, Pure, "web_socket_connect"),
            Self::WebSocketConnectWith => d(
                "WebSocket",
                "connectWith",
                1,
                Pure,
                "web_socket_connect_with",
            ),
            Self::WebSocketSend => d("WebSocket", "send", 2, Pure, "web_socket_send"),
            Self::WebSocketSendBinary => {
                d("WebSocket", "sendBinary", 2, Pure, "web_socket_send_binary")
            }
            Self::WebSocketClose => d("WebSocket", "close", 1, Pure, "web_socket_close"),
            Self::WebSocketCloseWithCode => d(
                "WebSocket",
                "closeWithCode",
                3,
                Pure,
                "web_socket_close_with_code",
            ),
            // The runtime fn here is a placeholder: the peephole always rewrites
            // the call to one of `sub_subscribe_ws_{message,open,close,error}`,
            // so this name is never emitted directly.
            Self::SubSubscribeWebSocket => d(
                "Sub",
                "subscribeWebSocket",
                3,
                Tea,
                "sub_subscribe_ws_message",
            ),
            Self::EnvPublic => d("Env", "public", 1, Pure, "env_public"),
            // ── Ipe.Ui.Region ──────────────────────────────────────────────
            Self::RegionMainContent => d("Region", "mainContent", 0, Ui, "ui_region_main_content_"),
            Self::RegionNavigation => d("Region", "navigation", 0, Ui, "ui_region_navigation_"),
            Self::RegionFooter => d("Region", "footer", 0, Ui, "ui_region_footer_"),
            Self::RegionAside => d("Region", "aside", 0, Ui, "ui_region_aside_"),
            Self::RegionHeading => d("Region", "heading", 1, Ui, "ui_region_heading_"),
            Self::RegionLabel => d("Region", "label", 1, Ui, "ui_region_label_"),
            Self::RegionAnnounce => d("Region", "announce", 0, Ui, "ui_region_announce_"),
            Self::RegionAnnounceUrgently => d(
                "Region",
                "announceUrgently",
                0,
                Ui,
                "ui_region_announce_urgently_",
            ),
            // ── Ui.input + Ui.describe + desc* constructors ───────────────
            Self::UiInput => d("Ui", "input", 1, Ui, "ui_input_"),
            Self::UiDescribe => d("Ui", "describe", 1, Ui, "ui_describe_"),
            Self::UiDescMain => d("Ui", "descMain", 0, Ui, "ui_desc_main_"),
            Self::UiDescNavigation => d("Ui", "descNavigation", 0, Ui, "ui_desc_navigation_"),
            Self::UiDescContentInfo => d("Ui", "descContentInfo", 0, Ui, "ui_desc_content_info_"),
            Self::UiDescComplementary => {
                d("Ui", "descComplementary", 0, Ui, "ui_desc_complementary_")
            }
            Self::UiDescLivePolite => d("Ui", "descLivePolite", 0, Ui, "ui_desc_live_polite_"),
            Self::UiDescLiveAssertive => {
                d("Ui", "descLiveAssertive", 0, Ui, "ui_desc_live_assertive_")
            }
            Self::UiDescHeading => d("Ui", "descHeading", 1, Ui, "ui_desc_heading_"),
            Self::UiDescLabel => d("Ui", "descLabel", 1, Ui, "ui_desc_label_"),
            // ── Ipe.Ui.Input ───────────────────────────────────────────
            Self::InputLabelAbove => d("Input", "labelAbove", 2, Ui, "input_label_above_"),
            Self::InputLabelBelow => d("Input", "labelBelow", 2, Ui, "input_label_below_"),
            Self::InputLabelLeft => d("Input", "labelLeft", 2, Ui, "input_label_left_"),
            Self::InputLabelRight => d("Input", "labelRight", 2, Ui, "input_label_right_"),
            Self::InputLabelHidden => d("Input", "labelHidden", 1, Ui, "input_label_hidden_"),
            Self::InputPlaceholder => d("Input", "placeholder", 2, Ui, "input_placeholder_"),
            // Record-arg kernels: arity 2 (attrs + cfg record).
            Self::InputText => d("Input", "text", 2, Ui, "input_text_"),
            Self::InputMultiline => d("Input", "multiline", 2, Ui, "input_multiline_"),
            Self::InputEmail => d("Input", "email", 2, Ui, "input_email_"),
            Self::InputUsername => d("Input", "username", 2, Ui, "input_username_"),
            Self::InputSearch => d("Input", "search", 2, Ui, "input_search_"),
            Self::InputCurrentPassword => {
                d("Input", "currentPassword", 2, Ui, "input_current_password_")
            }
            Self::InputNewPassword => d("Input", "newPassword", 2, Ui, "input_new_password_"),
            Self::InputCheckbox => d("Input", "checkbox", 2, Ui, "input_checkbox_"),
            Self::InputSlider => d("Input", "slider", 2, Ui, "input_slider_"),
            Self::InputOption => d("Input", "option", 2, Ui, "input_option_"),
            Self::InputRadio => d("Input", "radio", 2, Ui, "input_radio_"),
            Self::InputRadioRow => d("Input", "radioRow", 2, Ui, "input_radio_row_"),
            // ── Ipe.Ui.Lazy ─────────────────────────────────────��──────
            Self::LazyLazy => d("Lazy", "lazy", 2, Ui, "lazy_lazy_"),
            Self::LazyLazy2 => d("Lazy", "lazy2", 3, Ui, "lazy_lazy2_"),
            Self::LazyLazy3 => d("Lazy", "lazy3", 4, Ui, "lazy_lazy3_"),
            Self::LazyLazy4 => d("Lazy", "lazy4", 5, Ui, "lazy_lazy4_"),
            Self::LazyLazy5 => d("Lazy", "lazy5", 6, Ui, "lazy_lazy5_"),
            // ── Ipe.Ui.Keyed ────────────────────────────────────────────────
            Self::KeyedColumn => d("Keyed", "column", 2, Ui, "keyed_column_"),
            Self::KeyedRow => d("Keyed", "row", 2, Ui, "keyed_row_"),
            // ── Ipe.Decimal — arbitrary-precision decimal arithmetic ──────────
            Self::DecZero => d("Decimal", "zero", 0, Pure, "decimal_zero"),
            Self::DecOne => d("Decimal", "one", 0, Pure, "decimal_one"),
            Self::DecOneHundred => d("Decimal", "oneHundred", 0, Pure, "decimal_one_hundred"),
            Self::DecFromString => d("Decimal", "fromString", 1, Pure, "decimal_from_string"),
            Self::DecFromInt => d("Decimal", "fromInt", 1, Pure, "decimal_from_int"),
            Self::DecFromFloat => d("Decimal", "fromFloat", 1, Pure, "decimal_from_float"),
            Self::DecFromMinor => d("Decimal", "fromMinor", 2, Pure, "decimal_from_minor"),
            Self::DecToString => d("Decimal", "toString", 1, Pure, "decimal_to_string"),
            Self::DecToStringFixed => d(
                "Decimal",
                "toStringFixed",
                2,
                Pure,
                "decimal_to_string_fixed",
            ),
            Self::DecToFloat => d("Decimal", "toFloat", 1, Pure, "decimal_to_float"),
            Self::DecToInt => d("Decimal", "toInt", 1, Pure, "decimal_to_int"),
            Self::DecToMinor => d("Decimal", "toMinor", 2, Pure, "decimal_to_minor"),
            Self::DecAdd => d("Decimal", "add", 2, Pure, "decimal_add"),
            Self::DecSub => d("Decimal", "sub", 2, Pure, "decimal_sub"),
            Self::DecMul => d("Decimal", "mul", 2, Pure, "decimal_mul"),
            Self::DecDiv => d("Decimal", "div", 2, Pure, "decimal_div"),
            Self::DecMod => d("Decimal", "mod", 2, Pure, "decimal_mod"),
            Self::DecNeg => d("Decimal", "neg", 1, Pure, "decimal_neg"),
            Self::DecAbs => d("Decimal", "abs", 1, Pure, "decimal_abs"),
            Self::DecFloor => d("Decimal", "floor", 1, Pure, "decimal_floor"),
            Self::DecCeil => d("Decimal", "ceil", 1, Pure, "decimal_ceil"),
            Self::DecRound => d("Decimal", "round", 2, Pure, "decimal_round"),
            Self::DecRoundHalfUp => d("Decimal", "roundHalfUp", 2, Pure, "decimal_round_half_up"),
            Self::DecTruncate => d("Decimal", "truncate", 2, Pure, "decimal_truncate"),
            Self::DecCompare => d("Decimal", "compare", 2, Pure, "decimal_compare"),
            Self::DecEq => d("Decimal", "eq", 2, Pure, "decimal_eq"),
            Self::DecNeq => d("Decimal", "neq", 2, Pure, "decimal_neq"),
            Self::DecLt => d("Decimal", "lt", 2, Pure, "decimal_lt"),
            Self::DecLte => d("Decimal", "lte", 2, Pure, "decimal_lte"),
            Self::DecGt => d("Decimal", "gt", 2, Pure, "decimal_gt"),
            Self::DecGte => d("Decimal", "gte", 2, Pure, "decimal_gte"),
            Self::DecMin => d("Decimal", "min", 2, Pure, "decimal_min"),
            Self::DecMax => d("Decimal", "max", 2, Pure, "decimal_max"),
            Self::DecIsZero => d("Decimal", "isZero", 1, Pure, "decimal_is_zero"),
            Self::DecIsPositive => d("Decimal", "isPositive", 1, Pure, "decimal_is_positive"),
            Self::DecIsNegative => d("Decimal", "isNegative", 1, Pure, "decimal_is_negative"),
            Self::DecPercentOf => d("Decimal", "percentOf", 2, Pure, "decimal_percent_of"),
            Self::DecAddPercent => d("Decimal", "addPercent", 2, Pure, "decimal_add_percent"),
            Self::DecSubPercent => d("Decimal", "subPercent", 2, Pure, "decimal_sub_percent"),
            Self::DecFormatWith => d("Decimal", "formatWith", 4, Pure, "decimal_format_with"),
            // ── Ipe.Money — currency table + FX registry + allocate ───────────
            Self::MoneyMinorUnits => d("Money", "minorUnits", 1, Pure, "money_minor_units"),
            Self::MoneySymbol => d("Money", "symbol", 1, Pure, "money_symbol"),
            Self::MoneyCurrencyName => d("Money", "currencyName", 1, Pure, "money_currency_name"),
            Self::MoneyIsKnownCurrency => d(
                "Money",
                "isKnownCurrency",
                1,
                Pure,
                "money_is_known_currency",
            ),
            Self::MoneyFormat => d("Money", "format", 2, Pure, "money_format"),
            Self::MoneyFormatWithCode => {
                d("Money", "formatWithCode", 2, Pure, "money_format_with_code")
            }
            Self::MoneyAllocate => d("Money", "allocate", 3, Pure, "money_allocate"),
            Self::MoneySetRate => d("Money", "setRate", 3, Pure, "money_set_rate"),
            Self::MoneyGetRate => d("Money", "getRate", 2, Pure, "money_get_rate"),
            Self::MoneyHasRate => d("Money", "hasRate", 2, Pure, "money_has_rate"),
            Self::MoneyClearRates => d("Money", "clearRates", 1, Pure, "money_clear_rates"),
            // ── Ipe.Db.Sql — SqlFragment builder ───────────────
            Self::SqlColumn => d("Sql", "column", 1, Db, "sql_column"),
            // `int` / `string` / `float` / `bool` are Ipê-level type
            // narrowings of `param`; all five share the `sql_param` runtime
            // symbol (see the emit-side note in `ipe_backend_rust::naming`).
            Self::SqlParam => d("Sql", "param", 1, Db, "sql_param"),
            Self::SqlInt => d("Sql", "int", 1, Db, "sql_param"),
            Self::SqlString => d("Sql", "string", 1, Db, "sql_param"),
            Self::SqlFloat => d("Sql", "float", 1, Db, "sql_param"),
            Self::SqlBool => d("Sql", "bool", 1, Db, "sql_param"),
            Self::SqlEq => d("Sql", "eq", 2, Db, "sql_eq"),
            Self::SqlNe => d("Sql", "ne", 2, Db, "sql_ne"),
            Self::SqlGt => d("Sql", "gt", 2, Db, "sql_gt"),
            Self::SqlLt => d("Sql", "lt", 2, Db, "sql_lt"),
            Self::SqlGte => d("Sql", "gte", 2, Db, "sql_gte"),
            Self::SqlLte => d("Sql", "lte", 2, Db, "sql_lte"),
            Self::SqlAnd => d("Sql", "and", 2, Db, "sql_and"),
            Self::SqlOr => d("Sql", "or", 2, Db, "sql_or"),
            Self::SqlNot => d("Sql", "not", 1, Db, "sql_not"),
            Self::SqlIsNull => d("Sql", "isNull", 1, Db, "sql_is_null"),
            Self::SqlIsNotNull => d("Sql", "isNotNull", 1, Db, "sql_is_not_null"),
            Self::SqlInList => d("Sql", "inList", 2, Db, "sql_in_list"),
            Self::SqlLike => d("Sql", "like", 2, Db, "sql_like"),
            Self::DbFindWhere => d("Db", "findWhere", 3, Db, "db_find_where"),
            Self::DbDeleteWhere => d("Db", "deleteWhere", 3, Db, "db_delete_where"),
            // ── Ipe.Secret — opaque secret-string wrapper ─
            Self::SecretFromString => d("Secret", "fromString", 1, Pure, "secret_from_string"),
            Self::SecretReveal => d("Secret", "reveal", 1, Pure, "secret_reveal"),
            Self::SecretRedacted => d("Secret", "redacted", 1, Pure, "secret_redacted"),
            // ── Ipe.Regex ────────────────────────────────────────
            // Runtime names MUST match `ipe_runtime::regex_kernel::*` exactly
            // (note `regex_find_all`). Class `Pure` — the kernels are total/pure
            // (no effect); the HM scheme carries no `Task`. `compile` parses the
            // pattern once; every operation then takes the compiled `Regex`.
            Self::RegexCompile => d("Regex", "compile", 1, Pure, "regex_compile"),
            Self::RegexMatch => d("Regex", "match", 2, Pure, "regex_match"),
            Self::RegexFind => d("Regex", "find", 2, Pure, "regex_find"),
            Self::RegexFindAll => d("Regex", "findAll", 2, Pure, "regex_find_all"),
            Self::RegexReplace => d("Regex", "replace", 3, Pure, "regex_replace"),
            Self::RegexSplit => d("Regex", "split", 2, Pure, "regex_split"),
            // ── Ipe.Path ─────────────────────────────────────────
            // Runtime names MUST match `ipe_runtime::path::*` exactly
            // (`path_is_absolute`). Pure/total, no effect.
            Self::PathFromString => d("Path", "fromString", 1, Pure, "path_from_string"),
            Self::PathToString => d("Path", "toString", 1, Pure, "path_to_string"),
            Self::PathBase => d("Path", "base", 1, Pure, "path_base"),
            Self::PathDir => d("Path", "dir", 1, Pure, "path_dir"),
            Self::PathExt => d("Path", "ext", 1, Pure, "path_ext"),
            Self::PathIsAbsolute => d("Path", "isAbsolute", 1, Pure, "path_is_absolute"),
            // ── Ipe.Trace ─────────────────────────────────────────────
            // Runtime names MUST match `ipe_runtime::trace::*` exactly.
            Self::TraceSpan => d("Trace", "span", 2, Pure, "trace_span"),
            Self::TraceEvent => d("Trace", "event", 1, Pure, "trace_event"),
            Self::TraceAttr => d("Trace", "attr", 2, Pure, "trace_attr"),
            // ── Ipe.Compression ───────────────────────────────────────
            // Runtime names MUST match `ipe_runtime::compression::*` exactly.
            Self::CompressionGzip => d("Compression", "gzip", 1, Pure, "compression_gzip"),
            Self::CompressionGunzip => d("Compression", "gunzip", 1, Pure, "compression_gunzip"),
            Self::CompressionZstdCompress => d(
                "Compression",
                "zstdCompress",
                1,
                Pure,
                "compression_zstd_compress",
            ),
            Self::CompressionZstdDecompress => d(
                "Compression",
                "zstdDecompress",
                1,
                Pure,
                "compression_zstd_decompress",
            ),
            // ── Ipe.Csv ───────────────────────────────────────────────
            Self::CsvParse => d("Csv", "parse", 1, Pure, "csv_parse"),
            Self::CsvParseWithDelimiter => d(
                "Csv",
                "parseWithDelimiter",
                2,
                Pure,
                "csv_parse_with_delimiter",
            ),
            Self::CsvEncode => d("Csv", "encode", 1, Pure, "csv_encode"),
            Self::CsvEncodeWithDelimiter => d(
                "Csv",
                "encodeWithDelimiter",
                2,
                Pure,
                "csv_encode_with_delimiter",
            ),
            Self::CsvParseStreamFromFile => d(
                "Csv",
                "parseStreamFromFile",
                1,
                Pure,
                "csv_parse_stream_from_file",
            ),
            // ── Ipe.Cache ─────────────────────────────────────────────
            // Runtime names MUST match `ipe_runtime::cache::*` exactly. Alias
            // strings `Cache_newRaw`/`Cache_get`/… split to qualifier `Cache` +
            // the `*Raw`-stripped `name` written here; the emit column is the
            // runtime fn (`cache_new_raw` for `newRaw`).
            Self::CacheNewRaw => d("Cache", "newRaw", 1, Pure, "cache_new_raw"),
            Self::CacheGet => d("Cache", "get", 2, Pure, "cache_get"),
            Self::CachePut => d("Cache", "put", 3, Pure, "cache_put"),
            Self::CacheRemove => d("Cache", "remove", 2, Pure, "cache_remove"),
            Self::CacheClear => d("Cache", "clear", 1, Pure, "cache_clear"),
            Self::CacheSize => d("Cache", "size", 1, Pure, "cache_size"),
            Self::CacheStats => d("Cache", "stats", 1, Pure, "cache_stats"),

            // ── Ipe.Config ────────────────────────────────────────────
            // The 11 combinator/primitive kernels share the JSON `decode_*`
            // runtime fns; the 5 format/nullable/load kernels are Config-own
            // (`ipe_runtime::config_decode::*`).
            Self::ConfigString => d("Config", "string", 0, Pure, "json_decode_string"),
            Self::ConfigInt => d("Config", "int", 0, Pure, "json_decode_int"),
            Self::ConfigFloat => d("Config", "float", 0, Pure, "json_decode_float"),
            Self::ConfigBool => d("Config", "bool", 0, Pure, "json_decode_bool"),
            Self::ConfigNullable => d("Config", "nullable", 1, Pure, "config_nullable"),
            Self::ConfigField => d("Config", "field", 2, Pure, "decode_field"),
            Self::ConfigAt => d("Config", "at", 2, Pure, "decode_at"),
            Self::ConfigList => d("Config", "list", 1, Pure, "decode_list"),
            Self::ConfigSucceed => d("Config", "succeed", 1, Pure, "decode_succeed"),
            Self::ConfigFail => d("Config", "fail", 1, Pure, "decode_fail"),
            Self::ConfigMap => d("Config", "map", 2, Pure, "decode_map"),
            Self::ConfigAndThen => d("Config", "andThen", 2, Pure, "decode_and_then"),
            Self::ConfigMap2 => d("Config", "map2", 3, Pure, "decode_map2"),
            Self::ConfigMap3 => d("Config", "map3", 4, Pure, "decode_map3"),
            Self::ConfigMap4 => d("Config", "map4", 5, Pure, "decode_map4"),
            Self::ConfigMap5 => d("Config", "map5", 6, Pure, "decode_map5"),
            Self::ConfigMap6 => d("Config", "map6", 7, Pure, "decode_map6"),
            Self::ConfigMap7 => d("Config", "map7", 8, Pure, "decode_map7"),
            Self::ConfigMap8 => d("Config", "map8", 9, Pure, "decode_map8"),
            Self::ConfigOneOf => d("Config", "oneOf", 1, Pure, "decode_one_of"),
            Self::ConfigIndex => d("Config", "index", 2, Pure, "decode_index"),
            Self::ConfigKeyValuePairs => {
                d("Config", "keyValuePairs", 1, Pure, "decode_key_value_pairs")
            }
            Self::ConfigMaybe => d("Config", "maybe", 1, Pure, "config_maybe"),
            Self::ConfigDict => d("Config", "dict", 1, Pure, "config_dict"),
            Self::ConfigDecodeToml => d("Config", "decodeToml", 2, Pure, "config_decode_toml"),
            Self::ConfigDecodeYaml => d("Config", "decodeYaml", 2, Pure, "config_decode_yaml"),
            Self::ConfigDecodeJson => d("Config", "decodeJson", 2, Pure, "config_decode_json"),
            Self::ConfigLoadFromFile => {
                d("Config", "loadFromFile", 2, Pure, "config_load_from_file")
            }
            // ── Ipe.Email ─────────────────────────────────────────────
            // Alias `Email_send` splits to qualifier `Email` + name `send`; the
            // emit column is the runtime fn `ipe_runtime::email::email_send`.
            Self::EmailSend => d("Email", "send", 2, Pure, "email_send"),
            // ── Ipe.Crypto typed-key newtypes ─────────────────────────
            Self::CryptoKeyFromString => d("Key", "fromString", 1, Pure, "crypto_key_from_string"),
            Self::CryptoKeyFromBytes => d("Key", "fromBytes", 1, Pure, "crypto_key_from_bytes"),
            Self::CryptoMacToHex => d("Mac", "toHex", 1, Pure, "crypto_mac_to_hex"),
            Self::CryptoHmacSha256WithKey => d(
                "Crypto",
                "hmacSha256WithKey",
                2,
                Pure,
                "crypto_hmac_sha256_key",
            ),
            Self::CryptoHmacSha512WithKey => d(
                "Crypto",
                "hmacSha512WithKey",
                2,
                Pure,
                "crypto_hmac_sha512_key",
            ),
            Self::CryptoAesKeyFromPasswordKey => d(
                "Crypto",
                "aesKeyFromPasswordKey",
                2,
                Pure,
                "crypto_aes_key_from_password_key",
            ),
            Self::CryptoChachaKeyFromPasswordKey => d(
                "Crypto",
                "chachaKeyFromPasswordKey",
                2,
                Pure,
                "crypto_chacha_key_from_password_key",
            ),
            Self::CryptoAesGcmEncryptKey => d(
                "Crypto",
                "aesGcmEncryptKey",
                2,
                Pure,
                "crypto_aes_gcm_encrypt_key",
            ),
            Self::CryptoAesGcmDecryptKey => d(
                "Crypto",
                "aesGcmDecryptKey",
                2,
                Pure,
                "crypto_aes_gcm_decrypt_key",
            ),
            Self::CryptoChacha20EncryptKey => d(
                "Crypto",
                "chacha20EncryptKey",
                2,
                Pure,
                "crypto_chacha20_encrypt_key",
            ),
            Self::CryptoChacha20DecryptKey => d(
                "Crypto",
                "chacha20DecryptKey",
                2,
                Pure,
                "crypto_chacha20_decrypt_key",
            ),
            // ── Ipe.Email.EmailAddress ─────────────────────────────────
            Self::EmailAddressParse => d("EmailAddress", "parse", 1, Pure, "email_address_parse"),
            Self::EmailAddressToString => d(
                "EmailAddress",
                "toString",
                1,
                Pure,
                "email_address_to_string",
            ),
        }
    }

    /// All **wired** stdlib kernel variants.
    ///
    /// This slice is the single source of truth used by the canon-equality
    /// tripwire test (`canon_equals_registry` in `ipe_canon`) to verify that
    /// every registry entry has a matching entry in the canon `QUALIFIERS`
    /// table.
    ///
    /// # Exclusions
    ///
    /// `PubSubPublish` / `PubSubPublishNoEcho` are in `ALL` but their `"PubSub"`
    /// qualifier is not a kernel-`QUALIFIERS` entry — `Ipe.PubSub` is a
    /// compiled-source module, so it is resolved through the `Ffi.kernel
    /// "PubSub_publish"` alias, not a canon qualifier. The tripwire skips a
    /// qualifier absent from `qual_vars`, so this is an automatic skip, not a
    /// hand-maintained exclusion. `CmdPublish` / `CmdPublishNoEcho` carry their
    /// own `"Cmd"` `QUALIFIERS` entries.
    pub const ALL: &'static [Self] = &[
        // Log
        Self::LogInfo,
        Self::LogDebug,
        Self::LogWarn,
        Self::LogError,
        Self::LogInfoWith,
        Self::LogDebugWith,
        Self::LogWarnWith,
        Self::LogErrorWith,
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
        Self::StringContainsIn,
        Self::StringStartsWithIn,
        Self::StringEndsWithIn,
        Self::StringLeft,
        Self::StringRight,
        Self::StringCons,
        Self::StringUncons,
        Self::StringPad,
        Self::StringIndexes,
        Self::StringMap,
        Self::StringFilter,
        Self::StringFoldl,
        Self::StringFoldr,
        Self::StringAny,
        Self::StringAll,
        // Char
        Self::CharIsAlpha,
        Self::CharIsDigit,
        Self::CharIsLower,
        Self::CharIsUpper,
        Self::CharToLower,
        Self::CharToUpper,
        Self::CharToCode,
        Self::CharFromCode,
        Self::CharIsAlphaNum,
        Self::CharIsHexDigit,
        Self::CharIsOctDigit,
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
        Self::ListAny,
        Self::ListAll,
        Self::ListFind,
        // ── List batch ────────────────────────────────────────────────
        Self::ListFilterMap,
        Self::ListSortBy,
        Self::ListSort,
        Self::ListSortWith,
        Self::ListSingleton,
        Self::ListRepeat,
        Self::ListSum,
        Self::ListProduct,
        Self::ListMaximum,
        Self::ListMinimum,
        Self::ListIntersperse,
        Self::ListPartition,
        Self::ListUnzip,
        Self::ListMap2,
        Self::ListMap3,
        Self::ListMap4,
        Self::ListMap5,
        // Basics
        Self::BasicsNot,
        Self::BasicsIdentity,
        Self::BasicsAlways,
        Self::BasicsFst,
        Self::BasicsSnd,
        Self::BasicsModBy,
        Self::BasicsClamp,
        Self::BasicsToString,
        // ── Basics numerics ──────────────────────────────────────────
        Self::BasicsNegate,
        Self::BasicsAbs,
        Self::BasicsSqrt,
        Self::BasicsMin,
        Self::BasicsMax,
        Self::BasicsCompare,
        // ── end Basics numerics ──────────────────────────────────────
        // Error (Ipe.Error — minimal `Error = String` slice)
        Self::ErrorUnexpected,
        Self::ErrorInvalidInput,
        Self::ErrorIo,
        Self::ErrorNetwork,
        Self::ErrorFfi,
        Self::ErrorDecode,
        Self::ErrorConflict,
        Self::ErrorUnavailable,
        Self::ErrorTimeout,
        Self::ErrorNotFound,
        Self::ErrorPermissionDenied,
        Self::ErrorToString,
        Self::ErrorWithMessage,
        Self::ErrorIsRetryable,
        Self::ErrorWithDetails,
        Self::ErrorKind,
        Self::ErrorMessage,
        Self::ErrorKindName,
        // CssSafety (Ipe.CssSafety — Ipe.Css leaf security kernels)
        Self::CssSafetySafeValue,
        Self::CssSafetySafePropName,
        Self::CssSafetySafeSelector,
        Self::CssSafetyStripStyleClose,
        // Maybe
        Self::MaybeWithDefault,
        Self::MaybeMap,
        Self::MaybeAndThen,
        Self::MaybeMap2,
        Self::MaybeMap3,
        Self::MaybeMap4,
        Self::MaybeMap5,
        Self::MaybeAndMap,
        Self::MaybeCombine,
        // Result
        Self::ResultWithDefault,
        Self::ResultMap,
        Self::ResultAndThen,
        Self::ResultMapError,
        Self::ResultMap2,
        Self::ResultMap3,
        Self::ResultMap4,
        Self::ResultMap5,
        Self::ResultAndMap,
        Self::ResultCombine,
        Self::ResultTraverse,
        Self::ResultToMaybe,
        Self::ResultFromMaybe,
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
        Self::MathIsNaN,
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
        Self::DictSingleton,
        Self::DictFoldr,
        Self::DictFilter,
        Self::DictPartition,
        Self::DictIntersect,
        Self::DictDiff,
        Self::DictUpdate,
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
        Self::SetIsEmpty,
        Self::SetSingleton,
        Self::SetFoldl,
        Self::SetFoldr,
        Self::SetMap,
        Self::SetFilter,
        Self::SetPartition,
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
        // Jwt builder API (D-00)
        Self::JwtClaims,
        Self::JwtHs256,
        Self::JwtRs256,
        Self::JwtSubject,
        Self::JwtIssuer,
        Self::JwtAudience,
        Self::JwtExpiresAt,
        Self::JwtNotBefore,
        Self::JwtIssuedAt,
        Self::JwtJwtId,
        Self::JwtWithClaim,
        Self::JwtEncode,
        Self::JwtDecode,
        // Task
        Self::TaskSucceed,
        Self::TaskFail,
        Self::TaskMap,
        Self::TaskMap2,
        Self::TaskMap3,
        Self::TaskMap4,
        Self::TaskMap5,
        Self::TaskAttempt,
        Self::TaskAndThen,
        Self::TaskMapError,
        Self::TaskOnError,
        Self::TaskFromResult,
        Self::TaskAndThenResult,
        Self::TaskSequence,
        Self::TaskParallel,
        Self::TaskRun,
        Self::TaskPerform,
        Self::TaskLazy,
        Self::TaskRetryWith,
        Self::TaskLinearBackoff,
        Self::TaskExponentialBackoff,
        Self::TaskWithJitter,
        Self::TaskRetryOn,
        Self::TaskWithRetryOn,
        Self::TaskDefaultRetryPolicy,
        Self::TaskWithMaxAttempts,
        Self::TaskWithBaseMs,
        Self::TaskWithKind,
        // Io
        Self::IoReadLine,
        Self::IoWriteStdout,
        Self::IoWriteStderr,
        Self::IoPrintln,
        Self::IoEprintln,
        // Debug (development-only)
        Self::DebugLog,
        // Time (non-TEA)
        Self::TimeNow,
        Self::TimeSleep,
        Self::TimeUnixMillis,
        Self::TimeTimeString,
        Self::TimeIsLeapYear,
        Self::TimeDaysInMonth,
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
        // Process
        Self::ProcessRun,
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
        Self::HttpWithUrl,
        Self::HttpWithFollowRedirects,
        Self::HttpWithMaxRedirects,
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
        Self::DbInsertFields,
        Self::DbUpdateFields,
        Self::DbInsertFieldsReturning,
        Self::DbWithTransaction,
        Self::DbMigrate,
        Self::DbDefaultMigration,
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
        Self::DbDecMoney,
        Self::DbDecBytes,
        // TEA: Cmd / Sub / Time.every
        Self::CmdNone,
        Self::CmdBatch,
        Self::CmdPerform,
        Self::CmdMap,
        Self::CmdPublish,
        Self::CmdPublishNoEcho,
        Self::SubNone,
        Self::SubBatch,
        Self::SubEvery,
        Self::SubMap,
        Self::SubSubscribeTopic,
        Self::TimeEvery,
        // Ipe.PubSub — Task-shaped top-level publish (qualifier "PubSub" in
        // canon QUALIFIERS; class = Web, not TEA-loop machinery)
        Self::PubSubPublish,
        Self::PubSubPublishNoEcho,
        // Ipe.Http.Server / Middleware / RateLimit
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
        Self::MiddlewareWithCsrf,
        Self::RateLimitAllow,
        // Ui / Html render kernels
        Self::UiLayout,
        Self::UiLayoutWith,
        Self::HtmlRender,
        Self::HtmlEscapeText,
        Self::HtmlEscapeAttr,
        Self::HtmlAttrToString,
        // Ui element builders
        Self::UiNone,
        Self::UiText,
        Self::UiHtml,
        Self::UiCells,
        Self::UiEl,
        Self::UiRow,
        Self::UiColumn,
        Self::UiWrappedRow,
        Self::UiGrid,
        Self::UiParagraph,
        Self::UiTextColumn,
        Self::UiButton,
        Self::UiLink,
        Self::UiForm,
        Self::UiImage,
        // Ui nearby attribute builders
        Self::UiAbove,
        Self::UiBelow,
        Self::UiOnLeft,
        Self::UiOnRight,
        Self::UiInFront,
        Self::UiBehind,
        // Ui attribute builders
        Self::UiSpacing,
        Self::UiPadding,
        Self::UiPaddingXY,
        Self::UiPaddingEach,
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
        Self::UiClipX,
        Self::UiClipY,
        Self::UiScrollbars,
        Self::UiScrollbarX,
        Self::UiScrollbarY,
        Self::UiGridColumns,
        // Ui Length builders
        Self::UiPx,
        Self::UiFill,
        Self::UiContent,
        Self::UiShrink,
        Self::UiFillPortion,
        Self::UiVh,
        Self::UiVw,
        Self::UiMinimum,
        Self::UiMaximum,
        // Ui Color builders
        Self::UiRgb,
        Self::UiRgba,
        Self::UiWhite,
        Self::UiBlack,
        Self::UiTransparent,
        Self::UiColorCss,
        // Background / Border / Font
        Self::BackgroundColor,
        Self::BackgroundImage,
        Self::BackgroundLinearGradient,
        Self::BorderWidth,
        Self::BorderRounded,
        Self::BorderColor,
        Self::BorderWidthEach,
        Self::BorderShadow,
        Self::BorderGlow,
        Self::BorderInnerShadow,
        Self::FontSize,
        Self::FontColor,
        Self::FontFamily,
        Self::FontBold,
        Self::FontItalic,
        // Html element builders
        Self::HtmlTextNode,
        Self::HtmlRawNode,
        Self::HtmlNode,
        Self::HtmlVoidNode,
        Self::HtmlDoctype,
        Self::HtmlTitleNode,
        Self::HtmlToString,
        Self::HtmlDiv,
        Self::HtmlSpan,
        Self::HtmlA,
        Self::HtmlButton,
        Self::HtmlP,
        Self::HtmlInput,
        Self::HtmlImg,
        // Ipe.Html element builders (container + void).
        Self::HtmlH1,
        Self::HtmlH2,
        Self::HtmlH3,
        Self::HtmlH4,
        Self::HtmlH5,
        Self::HtmlH6,
        Self::HtmlNav,
        Self::HtmlSection,
        Self::HtmlArticle,
        Self::HtmlHeader,
        Self::HtmlHeaderNode,
        Self::HtmlCodeNode,
        Self::HtmlMainNode,
        Self::HtmlFooterNode,
        Self::HtmlLinkNode,
        Self::HtmlFooter,
        Self::HtmlMain,
        Self::HtmlAside,
        Self::HtmlUl,
        Self::HtmlOl,
        Self::HtmlLi,
        Self::HtmlTable,
        Self::HtmlThead,
        Self::HtmlTbody,
        Self::HtmlTfoot,
        Self::HtmlTr,
        Self::HtmlTh,
        Self::HtmlTd,
        Self::HtmlTextarea,
        Self::HtmlSelect,
        Self::HtmlOption,
        Self::HtmlLabel,
        Self::HtmlForm,
        Self::HtmlFieldset,
        Self::HtmlLegend,
        Self::HtmlPre,
        Self::HtmlCode,
        Self::HtmlStrong,
        Self::HtmlEm,
        Self::HtmlSmall,
        Self::HtmlBlockquote,
        Self::HtmlFigure,
        Self::HtmlFigcaption,
        Self::HtmlDetails,
        Self::HtmlSummary,
        Self::HtmlDialog,
        Self::HtmlVideo,
        Self::HtmlAudio,
        Self::HtmlCanvas,
        Self::HtmlIframe,
        Self::HtmlProgress,
        Self::HtmlMeter,
        Self::HtmlScript,
        Self::HtmlBody,
        Self::HtmlTitle,
        Self::HtmlHtmlNode,
        Self::HtmlHeadNode,
        Self::HtmlBr,
        Self::HtmlHr,
        Self::HtmlMeta,
        Self::HtmlLink,
        Self::HtmlArea,
        Self::HtmlBase,
        Self::HtmlCol,
        Self::HtmlEmbed,
        Self::HtmlSource,
        Self::HtmlTrack,
        Self::HtmlWbr,
        // Ipe.Html.Attributes builders (all registered under "Attr" in
        // env.rs QUALIFIERS).
        Self::HtmlAttrClass,
        Self::HtmlAttrId,
        Self::HtmlAttrHref,
        Self::HtmlAttrSrc,
        Self::HtmlAttrAlt,
        Self::HtmlAttrValue,
        Self::HtmlAttrName,
        Self::HtmlAttrPlaceholder,
        Self::HtmlAttrType,
        Self::HtmlAttrFor,
        Self::HtmlAttrStyle,
        Self::HtmlAttrTitle,
        Self::HtmlAttrChecked,
        Self::HtmlAttrDisabled,
        Self::HtmlAttrReadonly,
        Self::HtmlAttrRequired,
        Self::HtmlAttrMultiple,
        Self::HtmlAttrSelected,
        Self::HtmlAttrAutofocus,
        Self::HtmlAttrAutocomplete,
        Self::HtmlAttribute,
        Self::HtmlBoolAttribute,
        Self::HtmlNoAttr,
        // `Html.styleNode` (F7) — a canon `Html` qualifier member (env.rs).
        // Registering it here gives it id=Some so its stdlib_scheme arm is
        // consulted; without this it would fail closed. A canon qualifier
        // member absent from ALL is minted with id=None and rides the
        // `Ty::Var(u32::MAX)` fallback.
        Self::HtmlStyleNode,
        // Web
        Self::WebApp,
        Self::WebAppRouted,
        Self::WebRoute,
        Self::WebRenderStatic,
        // Terminal
        Self::TerminalAppScreen,
        // WebView
        Self::WebViewApp,
        // event-attribute builders
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
        Self::UiOnSubmit,
        Self::UiOnFile,
        // Ipe.Html.Events builders (produce html_attr)
        Self::HtmlOnClick,
        Self::HtmlOnFocus,
        Self::HtmlOnBlur,
        Self::HtmlOnMouseOver,
        Self::HtmlOnMouseOut,
        Self::HtmlOnSubmit,
        Self::HtmlOnInput,
        Self::HtmlOnChange,
        Self::HtmlOnKeyDown,
        Self::HtmlOnKeyUp,
        Self::HtmlOnBool,
        Self::UiSquare,
        Self::UiWidescreen,
        Self::UiCinemascope,
        Self::UiAspectRatio,
        Self::UiAspectRatioWH,
        Self::UiHtmlAttribute,
        Self::UiName,
        Self::UiStyle,
        Self::UiTransitionRaw,
        Self::UiGridTracksRaw,
        Self::UiAnimateRaw,
        Self::UiBreakpoint,
        Self::UiMediaQuery,
        Self::UiMobile,
        Self::UiTablet,
        Self::UiDesktop,
        Self::UiDarkMode,
        Self::UiLightMode,
        Self::UiReducedMotion,
        Self::UiOnPseudo,
        Self::UiHover,
        Self::UiFocus,
        Self::UiFocusVisible,
        Self::UiActive,
        Self::UiDisabled,
        Self::BackgroundHoverColor,
        Self::BackgroundFocusColor,
        Self::BackgroundActiveColor,
        Self::BackgroundDisabledColor,
        Self::BorderSolid,
        Self::BorderDashed,
        Self::BorderDotted,
        Self::BorderHoverColor,
        Self::BorderFocusColor,
        Self::BorderActiveColor,
        Self::BorderHoverWidth,
        Self::BorderHoverRounded,
        Self::FontWeight,
        Self::FontSemiBold,
        Self::FontRegular,
        Self::FontLight,
        Self::FontExtraBold,
        Self::FontBlack,
        Self::FontUnderline,
        Self::FontNoDecoration,
        Self::FontLineThrough,
        Self::FontLetterSpacing,
        Self::FontWordSpacing,
        Self::FontAlignLeft,
        Self::FontAlignRight,
        Self::FontAlignCenter,
        Self::FontCenter,
        Self::FontJustify,
        Self::FontSansSerif,
        Self::FontSerif,
        Self::FontMonospace,
        Self::FontHoverColor,
        Self::FontFocusColor,
        Self::FontActiveColor,
        Self::FontDisabledColor,
        Self::FontHoverSize,
        Self::HtmlAttrTabindex,
        Self::HtmlAttrRows,
        // ── Effect stdlib modules ────────────────────────────────────────
        Self::TerminalAppLines,
        Self::AuthHashPassword,
        Self::AuthHashPasswordCost,
        Self::AuthVerifyPassword,
        Self::AuthPasswordStrength,
        Self::AuthSignToken,
        Self::AuthVerifyToken,
        Self::AuthRegister,
        Self::AuthLogin,
        Self::AuthSetRole,
        Self::StreamStream,
        Self::StreamEmit,
        Self::StreamFinish,
        Self::StreamWithContentType,
        Self::HttpStreamOpen,
        Self::HttpStreamForEachChunk,
        Self::HttpStreamClose,
        Self::HttpStreamChunks,
        // ── Ipe.Http.Server.WebSocket (12 kernels) ─────────────────────
        Self::WsDefaultCfg,
        Self::WsWithOnConnect,
        Self::WsWithOnMessage,
        Self::WsWithOnClose,
        Self::WsWithOnError,
        Self::WsWithMaxMessageBytes,
        Self::WsWithOriginPatterns,
        Self::WsUpgrade,
        Self::WsSendToClient,
        Self::WsSendBinaryToClient,
        Self::WsBroadcast,
        Self::WsCloseClient,
        // ── Ipe.WebSocket — outbound WebSocket client (7 kernels) ──
        Self::WebSocketConnect,
        Self::WebSocketConnectWith,
        Self::WebSocketSend,
        Self::WebSocketSendBinary,
        Self::WebSocketClose,
        Self::WebSocketCloseWithCode,
        Self::SubSubscribeWebSocket,
        // ── Ipe.Env — build-time-embedded public config ──────────────
        Self::EnvPublic,
        // ── Ipe.Ui.Region ──────────────────────────────────────────────
        Self::RegionMainContent,
        Self::RegionNavigation,
        Self::RegionFooter,
        Self::RegionAside,
        Self::RegionHeading,
        Self::RegionLabel,
        Self::RegionAnnounce,
        Self::RegionAnnounceUrgently,
        // ── Ui.input + Ui.describe + desc* constructors ───────────────────
        Self::UiInput,
        Self::UiDescribe,
        Self::UiDescMain,
        Self::UiDescNavigation,
        Self::UiDescContentInfo,
        Self::UiDescComplementary,
        Self::UiDescLivePolite,
        Self::UiDescLiveAssertive,
        Self::UiDescHeading,
        Self::UiDescLabel,
        // ── Ipe.Ui.Input ───────────────────────────────────────────────
        Self::InputLabelAbove,
        Self::InputLabelBelow,
        Self::InputLabelLeft,
        Self::InputLabelRight,
        Self::InputLabelHidden,
        Self::InputPlaceholder,
        Self::InputText,
        Self::InputMultiline,
        Self::InputEmail,
        Self::InputUsername,
        Self::InputSearch,
        Self::InputCurrentPassword,
        Self::InputNewPassword,
        Self::InputCheckbox,
        Self::InputSlider,
        Self::InputOption,
        Self::InputRadio,
        Self::InputRadioRow,
        // ── Ipe.Ui.Lazy ────────────────────────────────────────────────
        Self::LazyLazy,
        Self::LazyLazy2,
        Self::LazyLazy3,
        Self::LazyLazy4,
        Self::LazyLazy5,
        // ── Ipe.Ui.Keyed ──────────────────────────────────────────────────────
        Self::KeyedColumn,
        Self::KeyedRow,
        // ── Ipe.Decimal ───────────────────────────────────────────────────────
        Self::DecZero,
        Self::DecOne,
        Self::DecOneHundred,
        Self::DecFromString,
        Self::DecFromInt,
        Self::DecFromFloat,
        Self::DecFromMinor,
        Self::DecToString,
        Self::DecToStringFixed,
        Self::DecToFloat,
        Self::DecToInt,
        Self::DecToMinor,
        Self::DecAdd,
        Self::DecSub,
        Self::DecMul,
        Self::DecDiv,
        Self::DecMod,
        Self::DecNeg,
        Self::DecAbs,
        Self::DecFloor,
        Self::DecCeil,
        Self::DecRound,
        Self::DecRoundHalfUp,
        Self::DecTruncate,
        Self::DecCompare,
        Self::DecEq,
        Self::DecNeq,
        Self::DecLt,
        Self::DecLte,
        Self::DecGt,
        Self::DecGte,
        Self::DecMin,
        Self::DecMax,
        Self::DecIsZero,
        Self::DecIsPositive,
        Self::DecIsNegative,
        Self::DecPercentOf,
        Self::DecAddPercent,
        Self::DecSubPercent,
        Self::DecFormatWith,
        Self::MoneyMinorUnits,
        Self::MoneySymbol,
        Self::MoneyCurrencyName,
        Self::MoneyIsKnownCurrency,
        Self::MoneyFormat,
        Self::MoneyFormatWithCode,
        Self::MoneyAllocate,
        Self::MoneySetRate,
        Self::MoneyGetRate,
        Self::MoneyHasRate,
        Self::MoneyClearRates,
        Self::SqlColumn,
        Self::SqlParam,
        Self::SqlInt,
        Self::SqlString,
        Self::SqlFloat,
        Self::SqlBool,
        Self::SqlEq,
        Self::SqlNe,
        Self::SqlGt,
        Self::SqlLt,
        Self::SqlGte,
        Self::SqlLte,
        Self::SqlAnd,
        Self::SqlOr,
        Self::SqlNot,
        Self::SqlIsNull,
        Self::SqlIsNotNull,
        Self::SqlInList,
        Self::SqlLike,
        Self::DbFindWhere,
        Self::DbDeleteWhere,
        Self::SecretFromString,
        Self::SecretReveal,
        Self::SecretRedacted,
        // ── Ipe.Regex ────────────────────────────────────────────
        Self::RegexCompile,
        Self::RegexMatch,
        Self::RegexFind,
        Self::RegexFindAll,
        Self::RegexReplace,
        Self::RegexSplit,
        // ── Ipe.Path ─────────────────────────────────────────────
        Self::PathFromString,
        Self::PathToString,
        Self::PathBase,
        Self::PathDir,
        Self::PathExt,
        Self::PathIsAbsolute,
        // ── Ipe.Trace ─────────────────────────────────────────────────
        Self::TraceSpan,
        Self::TraceEvent,
        Self::TraceAttr,
        // ── Ipe.Compression ───────────────────────────────────────────
        Self::CompressionGzip,
        Self::CompressionGunzip,
        Self::CompressionZstdCompress,
        Self::CompressionZstdDecompress,
        // ── Ipe.Csv ───────────────────────────────────────────────────
        Self::CsvParse,
        Self::CsvParseWithDelimiter,
        Self::CsvEncode,
        Self::CsvEncodeWithDelimiter,
        Self::CsvParseStreamFromFile,
        // ── Ipe.Cache ─────────────────────────────────────────────────
        Self::CacheNewRaw,
        Self::CacheGet,
        Self::CachePut,
        Self::CacheRemove,
        Self::CacheClear,
        Self::CacheSize,
        Self::CacheStats,
        Self::ConfigString,
        Self::ConfigInt,
        Self::ConfigFloat,
        Self::ConfigBool,
        Self::ConfigNullable,
        Self::ConfigField,
        Self::ConfigAt,
        Self::ConfigList,
        Self::ConfigSucceed,
        Self::ConfigFail,
        Self::ConfigMap,
        Self::ConfigAndThen,
        Self::ConfigMap2,
        Self::ConfigMap3,
        Self::ConfigMap4,
        Self::ConfigMap5,
        Self::ConfigMap6,
        Self::ConfigMap7,
        Self::ConfigMap8,
        Self::ConfigOneOf,
        Self::ConfigIndex,
        Self::ConfigKeyValuePairs,
        Self::ConfigMaybe,
        Self::ConfigDict,
        Self::ConfigDecodeToml,
        Self::ConfigDecodeYaml,
        Self::ConfigDecodeJson,
        Self::ConfigLoadFromFile,
        // ── Ipe.Email ─────────────────────────────────────────────────
        Self::EmailSend,
        // ── Ipe.Crypto typed-key newtypes ─────────────────────────────
        Self::CryptoKeyFromString,
        Self::CryptoKeyFromBytes,
        Self::CryptoMacToHex,
        Self::CryptoHmacSha256WithKey,
        Self::CryptoHmacSha512WithKey,
        Self::CryptoAesKeyFromPasswordKey,
        Self::CryptoChachaKeyFromPasswordKey,
        Self::CryptoAesGcmEncryptKey,
        Self::CryptoAesGcmDecryptKey,
        Self::CryptoChacha20EncryptKey,
        Self::CryptoChacha20DecryptKey,
        // ── Ipe.Email.EmailAddress ─────────────────────────────────────
        Self::EmailAddressParse,
        Self::EmailAddressToString,
    ];

    // ── Classification predicates (moved from ipe_ir::KernelFn) ─────────────
    // These are the single authoritative classification lists.  `ipe_ir`
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
                | Self::DbDecMoney
                | Self::DbDecBytes
                // ── Ipe.Db.Sql — classified `Db` like
                // `Db.Decode.*` above: no live connection is touched by the
                // combinators, but the runtime types they build on
                // (`SqlFragment` / `SqlParam`) live in this crate's
                // `feature = "db"`-gated `db.rs` module, so a program using
                // ONLY `Sql.*` still needs the `db` Cargo feature turned on.
                | Self::SqlColumn
                | Self::SqlParam
                | Self::SqlInt
                | Self::SqlString
                | Self::SqlFloat
                | Self::SqlBool
                | Self::SqlEq
                | Self::SqlNe
                | Self::SqlGt
                | Self::SqlLt
                | Self::SqlGte
                | Self::SqlLte
                | Self::SqlAnd
                | Self::SqlOr
                | Self::SqlNot
                | Self::SqlIsNull
                | Self::SqlIsNotNull
                | Self::SqlInList
                | Self::SqlLike
                | Self::DbFindWhere
                | Self::DbDeleteWhere
        )
    }

    /// The conditionally-vendored runtime module this kernel's emitted symbol
    /// needs, when that module is NOT already pulled in by the kernel's emit
    /// [`KernelClass`]. `None` for the common case (symbol lives in the module
    /// the class declares, or in the always-present base set).
    ///
    /// This closes the module-set SEAL breach class: a kernel whose `rust_name`
    /// resolves to a feature-module the class does not declare MUST report that
    /// module here so the lowerer sets the matching `uses_*` flag. Keep this in
    /// lockstep with the emit table (`decl().emit`) — the `runtime_module_closure`
    /// backend test asserts every emitted crate is module-closed for every
    /// reachable flag combination, so a missing entry fails at `ipe` build time,
    /// never as a downstream `cargo` E0425/E0412.
    #[must_use]
    pub const fn required_runtime_module(self) -> Option<RuntimeModule> {
        match self {
            // `cmd_publish` / `cmd_publish_no_echo` / `sub_subscribe_topic` are
            // `class = Tea` (they dispatch through the standard TEA emit path) but
            // their runtime symbols are defined ONLY in `ipe_runtime::live::pubsub`
            // — the `live` module. Without this the `live` append never fires and
            // the emitted `main.rs` references undefined `cmd_publish` (E0425).
            Self::CmdPublish | Self::CmdPublishNoEcho | Self::SubSubscribeTopic => {
                Some(RuntimeModule::Web)
            }
            // `pubsub_publish` / `pubsub_publish_no_echo` are `class = Web` and
            // also `is_web`, so the `live` append fires via the `is_web` path in
            // the lowerer. Recording them here too keeps this function the complete
            // SSOT: every kernel whose emitted symbol diverges from its class's
            // module home is listed, whether or not a parallel predicate already
            // covers it. (`class = Web`'s home is the `web` module; the symbols
            // live in its `live::pubsub` submodule, gated by the `live` feature.)
            Self::PubSubPublish | Self::PubSubPublishNoEcho => Some(RuntimeModule::Web),
            // `HttpStream.chunks` is `class = Pure` but emits `sub_subscribe_stream`
            // and the `IpeStreamId` type, both defined in `ipe_runtime::http_stream`
            // — declared only by the `server` append. Its siblings
            // (`open`/`forEachChunk`/`close`) are `is_server` and ride along, but
            // `chunks` can be reached with a param-supplied `StreamId` and no `open`
            // in the same module set (E0412 `IpeStreamId` + E0425 otherwise).
            Self::HttpStreamChunks => Some(RuntimeModule::Server),
            _ => None,
        }
    }

    /// The security-relevant capability this kernel exercises, or `None` when it
    /// is pure. Classified by effect family: HTTP / server / WebSocket / email →
    /// [`Capability::Network`]; file / database / config-and-`.env`-file reads →
    /// [`Capability::Filesystem`]; environment-variable and argv reads →
    /// [`Capability::Env`]; wall-clock / sleep / timer → [`Capability::Clock`];
    /// RNG / random tokens / UUIDs → [`Capability::Random`]. `Env.public` reads a
    /// build-time-embedded allowlisted constant, not the live process
    /// environment, so it is pure. `Trace.*` write only to an observability sink,
    /// and `Io.*` only to the console, so neither is a sandboxed capability.
    ///
    /// The match is exhaustive with no `_` arm: a newly-added kernel cannot
    /// compile until it is classified here, so a program's inferred capability
    /// set cannot silently drift as the stdlib grows.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub const fn capability(self) -> Option<Capability> {
        match self {
            Self::HttpGet
            | Self::HttpPost
            | Self::HttpRequest
            | Self::ServerGet
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
            | Self::MiddlewareWithCsrf
            | Self::RateLimitAllow
            | Self::StreamStream
            | Self::StreamEmit
            | Self::StreamFinish
            | Self::StreamWithContentType
            | Self::HttpStreamOpen
            | Self::HttpStreamForEachChunk
            | Self::HttpStreamClose
            | Self::HttpStreamChunks
            | Self::WsDefaultCfg
            | Self::WsWithOnConnect
            | Self::WsWithOnMessage
            | Self::WsWithOnClose
            | Self::WsWithOnError
            | Self::WsWithMaxMessageBytes
            | Self::WsWithOriginPatterns
            | Self::WsUpgrade
            | Self::WsSendToClient
            | Self::WsSendBinaryToClient
            | Self::WsBroadcast
            | Self::WsCloseClient
            | Self::WebSocketConnect
            | Self::WebSocketConnectWith
            | Self::WebSocketSend
            | Self::WebSocketSendBinary
            | Self::WebSocketClose
            | Self::WebSocketCloseWithCode
            | Self::SubSubscribeWebSocket
            | Self::EmailSend => Some(Capability::Network),
            Self::SystemCwd
            | Self::SystemLoadEnv
            | Self::FileReadFile
            | Self::FileWriteFile
            | Self::FileExists
            | Self::FileRemove
            | Self::FileMkdirAll
            | Self::FileReadFileLimit
            | Self::FileReadFileBytes
            | Self::FileAppend
            | Self::FileReadDir
            | Self::FileIsDir
            | Self::FileTempFile
            | Self::FileTempDir
            | Self::FileCopy
            | Self::FileRename
            | Self::FileDelete
            | Self::CsvParseStreamFromFile
            | Self::ConfigLoadFromFile => Some(Capability::Filesystem),
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
            | Self::DbInsertFields
            | Self::DbUpdateFields
            | Self::DbInsertFieldsReturning
            | Self::DbWithTransaction
            | Self::DbMigrate
            | Self::DbFindWhere
            | Self::DbDeleteWhere
            | Self::DbDefaultMigration
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
            | Self::DbDecMoney
            | Self::DbDecBytes => Some(Capability::Database),
            Self::SystemArgs
            | Self::SystemGetenv
            | Self::SystemGetenvOr
            | Self::SystemGetArg
            | Self::SystemGetenvInt
            | Self::SystemGetenvBool
            | Self::SystemSetenv
            | Self::SystemUnsetenv => Some(Capability::Env),
            Self::ProcessRun => Some(Capability::Subprocess),
            Self::TimeNow
            | Self::TimeSleep
            | Self::TimeUnixMillis
            | Self::TimeTimeString
            | Self::SubEvery
            | Self::TimeEvery => Some(Capability::Clock),
            Self::CryptoRandomBytes
            | Self::CryptoRandomToken
            | Self::UuidV4
            | Self::UuidV7
            | Self::RandomInt
            | Self::RandomFloat
            | Self::RandomChoice => Some(Capability::Random),
            Self::LogInfo
            | Self::LogDebug
            | Self::LogWarn
            | Self::LogError
            | Self::LogInfoWith
            | Self::LogDebugWith
            | Self::LogWarnWith
            | Self::LogErrorWith
            | Self::DebugLog
            | Self::StringFromInt
            | Self::StringFromFloat
            | Self::StringLength
            | Self::StringIsEmpty
            | Self::StringReverse
            | Self::StringToUpper
            | Self::StringToLower
            | Self::StringCasefold
            | Self::StringTrim
            | Self::StringTrimStart
            | Self::StringTrimEnd
            | Self::StringToInt
            | Self::StringToFloat
            | Self::StringFromChar
            | Self::StringFromList
            | Self::StringConcat
            | Self::StringWords
            | Self::StringLines
            | Self::StringToList
            | Self::StringIsEmail
            | Self::StringIsUrl
            | Self::StringAppend
            | Self::StringContains
            | Self::StringStartsWith
            | Self::StringEndsWith
            | Self::StringEqualFold
            | Self::StringJoin
            | Self::StringSplit
            | Self::StringRepeat
            | Self::StringDropLeft
            | Self::StringDropRight
            | Self::StringReplace
            | Self::StringSlice
            | Self::StringPadLeft
            | Self::StringPadRight
            | Self::StringContainsIn
            | Self::StringStartsWithIn
            | Self::StringEndsWithIn
            | Self::StringLeft
            | Self::StringRight
            | Self::StringCons
            | Self::StringUncons
            | Self::StringPad
            | Self::StringIndexes
            | Self::StringMap
            | Self::StringFilter
            | Self::StringFoldl
            | Self::StringFoldr
            | Self::StringAny
            | Self::StringAll
            | Self::CharIsAlpha
            | Self::CharIsDigit
            | Self::CharIsLower
            | Self::CharIsUpper
            | Self::CharToLower
            | Self::CharToUpper
            | Self::CharToCode
            | Self::CharFromCode
            | Self::CharIsAlphaNum
            | Self::CharIsHexDigit
            | Self::CharIsOctDigit
            | Self::ListMap
            | Self::ListFilter
            | Self::ListFoldl
            | Self::ListFoldr
            | Self::ListLength
            | Self::ListHead
            | Self::ListTail
            | Self::ListMember
            | Self::ListRange
            | Self::ListReverse
            | Self::ListAppend
            | Self::ListConcat
            | Self::ListTake
            | Self::ListDrop
            | Self::ListZip
            | Self::ListCons
            | Self::ListIsEmpty
            | Self::ListConcatMap
            | Self::ListIndexedMap
            | Self::ListAny
            | Self::ListAll
            | Self::ListFind
            | Self::ListFilterMap
            | Self::ListSortBy
            | Self::ListSort
            | Self::ListSortWith
            | Self::ListSingleton
            | Self::ListRepeat
            | Self::ListSum
            | Self::ListProduct
            | Self::ListMaximum
            | Self::ListMinimum
            | Self::ListIntersperse
            | Self::ListPartition
            | Self::ListUnzip
            | Self::ListMap2
            | Self::ListMap3
            | Self::ListMap4
            | Self::ListMap5
            | Self::BasicsNot
            | Self::BasicsIdentity
            | Self::BasicsAlways
            | Self::BasicsFst
            | Self::BasicsSnd
            | Self::BasicsModBy
            | Self::BasicsClamp
            | Self::BasicsToString
            | Self::BasicsNegate
            | Self::BasicsAbs
            | Self::BasicsSqrt
            | Self::BasicsMin
            | Self::BasicsMax
            | Self::BasicsCompare
            | Self::ErrorUnexpected
            | Self::ErrorInvalidInput
            | Self::ErrorIo
            | Self::ErrorNetwork
            | Self::ErrorFfi
            | Self::ErrorDecode
            | Self::ErrorConflict
            | Self::ErrorUnavailable
            | Self::ErrorTimeout
            | Self::ErrorNotFound
            | Self::ErrorPermissionDenied
            | Self::ErrorToString
            | Self::ErrorWithMessage
            | Self::ErrorIsRetryable
            | Self::ErrorWithDetails
            | Self::ErrorKind
            | Self::ErrorMessage
            | Self::ErrorKindName
            | Self::CssSafetySafeValue
            | Self::CssSafetySafePropName
            | Self::CssSafetySafeSelector
            | Self::CssSafetyStripStyleClose
            | Self::MaybeWithDefault
            | Self::MaybeMap
            | Self::MaybeAndThen
            | Self::MaybeMap2
            | Self::MaybeMap3
            | Self::MaybeMap4
            | Self::MaybeMap5
            | Self::MaybeAndMap
            | Self::MaybeCombine
            | Self::ResultWithDefault
            | Self::ResultMap
            | Self::ResultAndThen
            | Self::ResultMapError
            | Self::ResultMap2
            | Self::ResultMap3
            | Self::ResultMap4
            | Self::ResultMap5
            | Self::ResultAndMap
            | Self::ResultCombine
            | Self::ResultTraverse
            | Self::ResultToMaybe
            | Self::ResultFromMaybe
            | Self::ResultOkDefault
            | Self::MathMin
            | Self::MathMax
            | Self::MathPi
            | Self::MathE
            | Self::MathPhi
            | Self::MathSqrt2
            | Self::MathInf
            | Self::MathNan
            | Self::MathIsNaN
            | Self::MathAbs
            | Self::MathSqrt
            | Self::MathCbrt
            | Self::MathExp
            | Self::MathExp2
            | Self::MathLog
            | Self::MathLog2
            | Self::MathLog10
            | Self::MathSin
            | Self::MathCos
            | Self::MathTan
            | Self::MathAsin
            | Self::MathAcos
            | Self::MathAtan
            | Self::MathSinh
            | Self::MathCosh
            | Self::MathTanh
            | Self::MathAsinh
            | Self::MathAcosh
            | Self::MathAtanh
            | Self::MathFloor
            | Self::MathCeil
            | Self::MathRound
            | Self::MathTrunc
            | Self::MathPow
            | Self::MathHypot
            | Self::MathAtan2
            | Self::MathMod
            | Self::MathRemainder
            | Self::DictEmpty
            | Self::DictIsEmpty
            | Self::DictSize
            | Self::DictKeys
            | Self::DictValues
            | Self::DictToList
            | Self::DictFromList
            | Self::DictGet
            | Self::DictMember
            | Self::DictRemove
            | Self::DictUnion
            | Self::DictMap
            | Self::DictInsert
            | Self::DictFoldl
            | Self::DictSingleton
            | Self::DictFoldr
            | Self::DictFilter
            | Self::DictPartition
            | Self::DictIntersect
            | Self::DictDiff
            | Self::DictUpdate
            | Self::SetEmpty
            | Self::SetSize
            | Self::SetToList
            | Self::SetFromList
            | Self::SetMember
            | Self::SetInsert
            | Self::SetRemove
            | Self::SetUnion
            | Self::SetIntersect
            | Self::SetDiff
            | Self::SetIsEmpty
            | Self::SetSingleton
            | Self::SetFoldl
            | Self::SetFoldr
            | Self::SetMap
            | Self::SetFilter
            | Self::SetPartition
            | Self::BytesEmpty
            | Self::BytesLength
            | Self::BytesIsEmpty
            | Self::BytesFromString
            | Self::BytesToString
            | Self::BytesFromHex
            | Self::BytesToHex
            | Self::BytesFromBase64
            | Self::BytesToBase64
            | Self::BytesAppend
            | Self::BytesSlice
            | Self::EncodingBase64Encode
            | Self::EncodingBase64Decode
            | Self::EncodingUrlEncode
            | Self::EncodingUrlDecode
            | Self::EncodingHexEncode
            | Self::EncodingHexDecode
            | Self::JsonEncString
            | Self::JsonEncInt
            | Self::JsonEncFloat
            | Self::JsonEncBool
            | Self::JsonEncNull
            | Self::JsonEncList
            | Self::JsonEncObject
            | Self::JsonEncEncode
            | Self::JsonDecString
            | Self::JsonDecInt
            | Self::JsonDecFloat
            | Self::JsonDecBool
            | Self::JsonDecDecodeString
            | Self::JsonDecField
            | Self::JsonDecAt
            | Self::JsonDecIndex
            | Self::JsonDecList
            | Self::JsonDecMap
            | Self::JsonDecAndThen
            | Self::JsonDecSucceed
            | Self::JsonDecFail
            | Self::JsonDecOneOf
            | Self::JsonDecMap2
            | Self::JsonDecMap3
            | Self::JsonDecMap4
            | Self::JsonDecPRequired
            | Self::JsonDecPOptional
            | Self::JsonDecPCustom
            | Self::JsonDecPRequiredAt
            | Self::CryptoSha256
            | Self::CryptoSha512
            | Self::CryptoSha1
            | Self::CryptoMd5
            | Self::CryptoHmacSha256
            | Self::CryptoHmacSha512
            | Self::CryptoRsaSha256Sign
            | Self::CryptoRsaSha256Verify
            | Self::CryptoConstantTimeEqual
            | Self::CryptoAesGcmEncrypt
            | Self::CryptoAesGcmDecrypt
            | Self::CryptoChacha20Encrypt
            | Self::CryptoChacha20Decrypt
            | Self::CryptoAesKeyFromPassword
            | Self::CryptoChachaKeyFromPassword
            | Self::UuidParse
            | Self::JwtEncodeHs256
            | Self::JwtDecodeHs256
            | Self::JwtEncodeRs256
            | Self::JwtDecodeRs256
            | Self::JwtClaims
            | Self::JwtHs256
            | Self::JwtRs256
            | Self::JwtSubject
            | Self::JwtIssuer
            | Self::JwtAudience
            | Self::JwtExpiresAt
            | Self::JwtNotBefore
            | Self::JwtIssuedAt
            | Self::JwtJwtId
            | Self::JwtWithClaim
            | Self::JwtEncode
            | Self::JwtDecode
            | Self::TaskSucceed
            | Self::TaskFail
            | Self::TaskMap
            | Self::TaskMap2
            | Self::TaskMap3
            | Self::TaskMap4
            | Self::TaskMap5
            | Self::TaskAttempt
            | Self::TaskAndThen
            | Self::TaskMapError
            | Self::TaskOnError
            | Self::TaskFromResult
            | Self::TaskAndThenResult
            | Self::TaskSequence
            | Self::TaskParallel
            | Self::TaskRun
            | Self::TaskPerform
            | Self::TaskLazy
            | Self::TaskRetryWith
            | Self::TaskLinearBackoff
            | Self::TaskExponentialBackoff
            | Self::TaskWithJitter
            | Self::TaskRetryOn
            | Self::TaskWithRetryOn
            | Self::TaskDefaultRetryPolicy
            | Self::TaskWithMaxAttempts
            | Self::TaskWithBaseMs
            | Self::TaskWithKind
            | Self::IoReadLine
            | Self::IoWriteStdout
            | Self::IoWriteStderr
            | Self::IoPrintln
            | Self::IoEprintln
            | Self::TimeIsLeapYear
            | Self::TimeDaysInMonth
            | Self::SystemExit
            | Self::HttpParseQuery
            | Self::HttpDefaultRequest
            | Self::HttpWithMethod
            | Self::HttpWithTimeout
            | Self::HttpWithBody
            | Self::HttpWithHeader
            | Self::HttpWithUrl
            | Self::HttpWithFollowRedirects
            | Self::HttpWithMaxRedirects
            | Self::CmdNone
            | Self::CmdBatch
            | Self::CmdPerform
            | Self::CmdMap
            | Self::SubNone
            | Self::SubBatch
            | Self::SubMap
            | Self::CmdPublish
            | Self::CmdPublishNoEcho
            | Self::SubSubscribeTopic
            | Self::PubSubPublish
            | Self::PubSubPublishNoEcho
            | Self::UiLayout
            | Self::UiLayoutWith
            | Self::HtmlRender
            | Self::HtmlEscapeText
            | Self::HtmlEscapeAttr
            | Self::HtmlAttrToString
            | Self::UiNone
            | Self::UiText
            | Self::UiHtml
            | Self::UiCells
            | Self::UiEl
            | Self::UiRow
            | Self::UiColumn
            | Self::UiWrappedRow
            | Self::UiGrid
            | Self::UiParagraph
            | Self::UiTextColumn
            | Self::UiButton
            | Self::UiLink
            | Self::UiForm
            | Self::UiImage
            | Self::UiAbove
            | Self::UiBelow
            | Self::UiOnLeft
            | Self::UiOnRight
            | Self::UiInFront
            | Self::UiBehind
            | Self::UiSpacing
            | Self::UiPadding
            | Self::UiPaddingXY
            | Self::UiPaddingEach
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
            | Self::UiClipX
            | Self::UiClipY
            | Self::UiScrollbars
            | Self::UiScrollbarX
            | Self::UiScrollbarY
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
            | Self::UiColorCss
            | Self::BackgroundColor
            | Self::BackgroundImage
            | Self::BackgroundLinearGradient
            | Self::BorderWidth
            | Self::BorderRounded
            | Self::BorderColor
            | Self::BorderWidthEach
            | Self::BorderShadow
            | Self::BorderGlow
            | Self::BorderInnerShadow
            | Self::FontSize
            | Self::FontColor
            | Self::FontFamily
            | Self::FontBold
            | Self::FontItalic
            | Self::HtmlTextNode
            | Self::HtmlRawNode
            | Self::HtmlNode
            | Self::HtmlVoidNode
            | Self::HtmlDoctype
            | Self::HtmlTitleNode
            | Self::HtmlToString
            | Self::HtmlStyleNode
            | Self::HtmlDiv
            | Self::HtmlSpan
            | Self::HtmlA
            | Self::HtmlButton
            | Self::HtmlP
            | Self::HtmlInput
            | Self::HtmlImg
            | Self::HtmlH1
            | Self::HtmlH2
            | Self::HtmlH3
            | Self::HtmlH4
            | Self::HtmlH5
            | Self::HtmlH6
            | Self::HtmlNav
            | Self::HtmlSection
            | Self::HtmlArticle
            | Self::HtmlHeader
            | Self::HtmlHeaderNode
            | Self::HtmlCodeNode
            | Self::HtmlMainNode
            | Self::HtmlFooterNode
            | Self::HtmlLinkNode
            | Self::HtmlFooter
            | Self::HtmlMain
            | Self::HtmlAside
            | Self::HtmlUl
            | Self::HtmlOl
            | Self::HtmlLi
            | Self::HtmlTable
            | Self::HtmlThead
            | Self::HtmlTbody
            | Self::HtmlTfoot
            | Self::HtmlTr
            | Self::HtmlTh
            | Self::HtmlTd
            | Self::HtmlTextarea
            | Self::HtmlSelect
            | Self::HtmlOption
            | Self::HtmlLabel
            | Self::HtmlForm
            | Self::HtmlFieldset
            | Self::HtmlLegend
            | Self::HtmlPre
            | Self::HtmlCode
            | Self::HtmlStrong
            | Self::HtmlEm
            | Self::HtmlSmall
            | Self::HtmlBlockquote
            | Self::HtmlFigure
            | Self::HtmlFigcaption
            | Self::HtmlDetails
            | Self::HtmlSummary
            | Self::HtmlDialog
            | Self::HtmlVideo
            | Self::HtmlAudio
            | Self::HtmlCanvas
            | Self::HtmlIframe
            | Self::HtmlProgress
            | Self::HtmlMeter
            | Self::HtmlScript
            | Self::HtmlBody
            | Self::HtmlTitle
            | Self::HtmlHtmlNode
            | Self::HtmlHeadNode
            | Self::HtmlBr
            | Self::HtmlHr
            | Self::HtmlMeta
            | Self::HtmlLink
            | Self::HtmlArea
            | Self::HtmlBase
            | Self::HtmlCol
            | Self::HtmlEmbed
            | Self::HtmlSource
            | Self::HtmlTrack
            | Self::HtmlWbr
            | Self::HtmlAttrClass
            | Self::HtmlAttrId
            | Self::HtmlAttrHref
            | Self::HtmlAttrSrc
            | Self::HtmlAttrAlt
            | Self::HtmlAttrValue
            | Self::HtmlAttrName
            | Self::HtmlAttrPlaceholder
            | Self::HtmlAttrType
            | Self::HtmlAttrFor
            | Self::HtmlAttrStyle
            | Self::HtmlAttrTitle
            | Self::HtmlAttrChecked
            | Self::HtmlAttrDisabled
            | Self::HtmlAttrReadonly
            | Self::HtmlAttrRequired
            | Self::HtmlAttrMultiple
            | Self::HtmlAttrSelected
            | Self::HtmlAttrAutofocus
            | Self::HtmlAttrAutocomplete
            | Self::HtmlAttribute
            | Self::HtmlBoolAttribute
            | Self::HtmlNoAttr
            | Self::WebApp
            | Self::WebAppRouted
            | Self::WebRoute
            | Self::WebRenderStatic
            | Self::TerminalAppScreen
            | Self::WebViewApp
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
            | Self::UiOnSubmit
            | Self::UiOnFile
            | Self::HtmlOnClick
            | Self::HtmlOnFocus
            | Self::HtmlOnBlur
            | Self::HtmlOnMouseOver
            | Self::HtmlOnMouseOut
            | Self::HtmlOnSubmit
            | Self::HtmlOnInput
            | Self::HtmlOnChange
            | Self::HtmlOnKeyDown
            | Self::HtmlOnKeyUp
            | Self::HtmlOnBool
            | Self::UiSquare
            | Self::UiWidescreen
            | Self::UiCinemascope
            | Self::UiAspectRatio
            | Self::UiAspectRatioWH
            | Self::UiHtmlAttribute
            | Self::UiName
            | Self::UiStyle
            | Self::UiTransitionRaw
            | Self::UiGridTracksRaw
            | Self::UiAnimateRaw
            | Self::UiBreakpoint
            | Self::UiMediaQuery
            | Self::UiMobile
            | Self::UiTablet
            | Self::UiDesktop
            | Self::UiDarkMode
            | Self::UiLightMode
            | Self::UiReducedMotion
            | Self::UiOnPseudo
            | Self::UiHover
            | Self::UiFocus
            | Self::UiFocusVisible
            | Self::UiActive
            | Self::UiDisabled
            | Self::BackgroundHoverColor
            | Self::BackgroundFocusColor
            | Self::BackgroundActiveColor
            | Self::BackgroundDisabledColor
            | Self::BorderSolid
            | Self::BorderDashed
            | Self::BorderDotted
            | Self::BorderHoverColor
            | Self::BorderFocusColor
            | Self::BorderActiveColor
            | Self::BorderHoverWidth
            | Self::BorderHoverRounded
            | Self::FontWeight
            | Self::FontSemiBold
            | Self::FontRegular
            | Self::FontLight
            | Self::FontExtraBold
            | Self::FontBlack
            | Self::FontUnderline
            | Self::FontNoDecoration
            | Self::FontLineThrough
            | Self::FontLetterSpacing
            | Self::FontWordSpacing
            | Self::FontAlignLeft
            | Self::FontAlignRight
            | Self::FontAlignCenter
            | Self::FontCenter
            | Self::FontJustify
            | Self::FontSansSerif
            | Self::FontSerif
            | Self::FontMonospace
            | Self::FontHoverColor
            | Self::FontFocusColor
            | Self::FontActiveColor
            | Self::FontDisabledColor
            | Self::FontHoverSize
            | Self::HtmlAttrTabindex
            | Self::HtmlAttrRows
            | Self::TerminalAppLines
            | Self::AuthHashPassword
            | Self::AuthHashPasswordCost
            | Self::AuthVerifyPassword
            | Self::AuthPasswordStrength
            | Self::AuthSignToken
            | Self::AuthVerifyToken
            | Self::AuthRegister
            | Self::AuthLogin
            | Self::AuthSetRole
            | Self::EnvPublic
            | Self::RegionMainContent
            | Self::RegionNavigation
            | Self::RegionFooter
            | Self::RegionAside
            | Self::RegionHeading
            | Self::RegionLabel
            | Self::RegionAnnounce
            | Self::RegionAnnounceUrgently
            | Self::UiInput
            | Self::UiDescribe
            | Self::UiDescMain
            | Self::UiDescNavigation
            | Self::UiDescContentInfo
            | Self::UiDescComplementary
            | Self::UiDescLivePolite
            | Self::UiDescLiveAssertive
            | Self::UiDescHeading
            | Self::UiDescLabel
            | Self::InputLabelAbove
            | Self::InputLabelBelow
            | Self::InputLabelLeft
            | Self::InputLabelRight
            | Self::InputLabelHidden
            | Self::InputPlaceholder
            | Self::InputText
            | Self::InputMultiline
            | Self::InputEmail
            | Self::InputUsername
            | Self::InputSearch
            | Self::InputCurrentPassword
            | Self::InputNewPassword
            | Self::InputCheckbox
            | Self::InputSlider
            | Self::InputOption
            | Self::InputRadio
            | Self::InputRadioRow
            | Self::LazyLazy
            | Self::LazyLazy2
            | Self::LazyLazy3
            | Self::LazyLazy4
            | Self::LazyLazy5
            | Self::KeyedColumn
            | Self::KeyedRow
            | Self::DecZero
            | Self::DecOne
            | Self::DecOneHundred
            | Self::DecFromString
            | Self::DecFromInt
            | Self::DecFromFloat
            | Self::DecFromMinor
            | Self::DecToString
            | Self::DecToStringFixed
            | Self::DecToFloat
            | Self::DecToInt
            | Self::DecToMinor
            | Self::DecAdd
            | Self::DecSub
            | Self::DecMul
            | Self::DecDiv
            | Self::DecMod
            | Self::DecNeg
            | Self::DecAbs
            | Self::DecFloor
            | Self::DecCeil
            | Self::DecRound
            | Self::DecRoundHalfUp
            | Self::DecTruncate
            | Self::DecCompare
            | Self::DecEq
            | Self::DecNeq
            | Self::DecLt
            | Self::DecLte
            | Self::DecGt
            | Self::DecGte
            | Self::DecMin
            | Self::DecMax
            | Self::DecIsZero
            | Self::DecIsPositive
            | Self::DecIsNegative
            | Self::DecPercentOf
            | Self::DecAddPercent
            | Self::DecSubPercent
            | Self::DecFormatWith
            | Self::MoneyMinorUnits
            | Self::MoneySymbol
            | Self::MoneyCurrencyName
            | Self::MoneyIsKnownCurrency
            | Self::MoneyFormat
            | Self::MoneyFormatWithCode
            | Self::MoneyAllocate
            | Self::MoneySetRate
            | Self::MoneyGetRate
            | Self::MoneyHasRate
            | Self::MoneyClearRates
            | Self::SqlColumn
            | Self::SqlParam
            | Self::SqlInt
            | Self::SqlString
            | Self::SqlFloat
            | Self::SqlBool
            | Self::SqlEq
            | Self::SqlNe
            | Self::SqlGt
            | Self::SqlLt
            | Self::SqlGte
            | Self::SqlLte
            | Self::SqlAnd
            | Self::SqlOr
            | Self::SqlNot
            | Self::SqlIsNull
            | Self::SqlIsNotNull
            | Self::SqlInList
            | Self::SqlLike
            | Self::SecretFromString
            | Self::SecretReveal
            | Self::SecretRedacted
            | Self::RegexCompile
            | Self::RegexMatch
            | Self::RegexFind
            | Self::RegexFindAll
            | Self::RegexReplace
            | Self::RegexSplit
            | Self::PathFromString
            | Self::PathToString
            | Self::PathBase
            | Self::PathDir
            | Self::PathExt
            | Self::PathIsAbsolute
            | Self::TraceSpan
            | Self::TraceEvent
            | Self::TraceAttr
            | Self::CompressionGzip
            | Self::CompressionGunzip
            | Self::CompressionZstdCompress
            | Self::CompressionZstdDecompress
            | Self::CsvParse
            | Self::CsvParseWithDelimiter
            | Self::CsvEncode
            | Self::CsvEncodeWithDelimiter
            | Self::CacheNewRaw
            | Self::CacheGet
            | Self::CachePut
            | Self::CacheRemove
            | Self::CacheClear
            | Self::CacheSize
            | Self::CacheStats
            | Self::ConfigString
            | Self::ConfigInt
            | Self::ConfigFloat
            | Self::ConfigBool
            | Self::ConfigNullable
            | Self::ConfigField
            | Self::ConfigAt
            | Self::ConfigList
            | Self::ConfigSucceed
            | Self::ConfigFail
            | Self::ConfigMap
            | Self::ConfigAndThen
            | Self::ConfigMap2
            | Self::ConfigMap3
            | Self::ConfigMap4
            | Self::ConfigMap5
            | Self::ConfigMap6
            | Self::ConfigMap7
            | Self::ConfigMap8
            | Self::ConfigOneOf
            | Self::ConfigIndex
            | Self::ConfigKeyValuePairs
            | Self::ConfigMaybe
            | Self::ConfigDict
            | Self::ConfigDecodeToml
            | Self::ConfigDecodeYaml
            | Self::ConfigDecodeJson
            // `HttpMethodFromString` / `HttpMethodToString` are pure converters —
            // no network or I/O side-effect, capability = None.
            | Self::HttpMethodFromString
            | Self::HttpMethodToString
            // ── Ipe.Crypto typed-key newtypes ─────────────────────────
            | Self::CryptoKeyFromString
            | Self::CryptoKeyFromBytes
            | Self::CryptoMacToHex
            | Self::CryptoHmacSha256WithKey
            | Self::CryptoHmacSha512WithKey
            | Self::CryptoAesKeyFromPasswordKey
            | Self::CryptoChachaKeyFromPasswordKey
            | Self::CryptoAesGcmEncryptKey
            | Self::CryptoAesGcmDecryptKey
            | Self::CryptoChacha20EncryptKey
            | Self::CryptoChacha20DecryptKey
            // ── Ipe.Email.EmailAddress ─────────────────────────────────
            | Self::EmailAddressParse
            | Self::EmailAddressToString => None,
        }
    }

    /// `true` when this variant belongs to the TEA (`Cmd` / `Sub` /
    /// A development-only escape hatch (the `Ipe.Debug` family). Rejected in a
    /// PRODUCTION build (`ipe build --optimize`, IPE-L0140) rather than
    /// silently stripped or shipped. The single SSOT for "which kernels are
    /// dev-only" — the lowerer's usage scan and every gate consult this.
    #[must_use]
    pub const fn is_dev_only(self) -> bool {
        matches!(self, Self::DebugLog)
    }

    /// `Time.every`) subsystem, including reserved pub/sub variants.
    #[must_use]
    pub const fn is_tea(self) -> bool {
        matches!(
            self,
            Self::CmdNone
                | Self::CmdBatch
                | Self::CmdPerform
                | Self::CmdMap
                | Self::TaskAttempt
                | Self::SubNone
                | Self::SubBatch
                | Self::SubEvery
                | Self::SubMap
                | Self::TimeEvery
                | Self::CmdPublish
                | Self::CmdPublishNoEcho
                | Self::SubSubscribeTopic
                | Self::HttpStreamChunks
                | Self::SubSubscribeWebSocket
        )
    }

    /// `true` when this variant belongs to the `Ipe.Http.Server` / Middleware
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
                | Self::MiddlewareWithCsrf
                | Self::RateLimitAllow
                // ── Ipe.Http.Server.Stream (server-side) ───────────────────
                | Self::StreamStream
                | Self::StreamEmit
                | Self::StreamFinish
                | Self::StreamWithContentType
                // ── Ipe.Http.Stream (client-side relay) ───────────────
                | Self::HttpStreamOpen
                | Self::HttpStreamForEachChunk
                | Self::HttpStreamClose
                // ── Ipe.Http.Server.WebSocket (12 kernels) ─────────────
                | Self::WsDefaultCfg
                | Self::WsWithOnConnect
                | Self::WsWithOnMessage
                | Self::WsWithOnClose
                | Self::WsWithOnError
                | Self::WsWithMaxMessageBytes
                | Self::WsWithOriginPatterns
                | Self::WsUpgrade
                | Self::WsSendToClient
                | Self::WsSendBinaryToClient
                | Self::WsBroadcast
                | Self::WsCloseClient
        )
    }

    /// `true` when this variant is an outbound `Ipe.WebSocket` CLIENT
    /// kernel (the 6 Task-tier connect/send/close kernels plus the Sub-tier
    /// `Sub.subscribeWebSocket`).
    ///
    /// Used by `ipe_lower` to detect `uses_websocket` and by the backend to add
    /// the `websocket_client` Cargo feature + `ws_client` runtime module (whose
    /// fns are gated behind that feature — unlike `Http.get`, they are NOT part
    /// of the always-present base module set).
    #[must_use]
    pub const fn is_websocket_client(self) -> bool {
        matches!(
            self,
            Self::WebSocketConnect
                | Self::WebSocketConnectWith
                | Self::WebSocketSend
                | Self::WebSocketSendBinary
                | Self::WebSocketClose
                | Self::WebSocketCloseWithCode
                | Self::SubSubscribeWebSocket
        )
    }

    /// `true` when this variant belongs to the `Ipe.Auth` kernel family
    /// (`Ipe.Auth.hashPassword` / `verifyPassword` / `signToken` / `verifyToken` /
    /// `register` / `login` / `setRole` and companions).
    ///
    /// Used by `ipe_lower` to detect `uses_auth` and emit the `auth` module into
    /// the generated `ipe_runtime/mod.rs`.
    #[must_use]
    pub const fn is_auth(self) -> bool {
        matches!(
            self,
            Self::AuthHashPassword
                | Self::AuthHashPasswordCost
                | Self::AuthVerifyPassword
                | Self::AuthPasswordStrength
                | Self::AuthSignToken
                | Self::AuthVerifyToken
                | Self::AuthRegister
                | Self::AuthLogin
                | Self::AuthSetRole
        )
    }

    /// `true` when this variant belongs to the `Ipe.Ui` / `Ipe.Html`
    /// subsystem.
    #[must_use]
    #[allow(clippy::too_many_lines)] // exhaustive Ui/Html kernel enumeration
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
                | Self::UiCells
                | Self::UiEl
                | Self::UiRow
                | Self::UiColumn
                | Self::UiWrappedRow
                | Self::UiGrid
                | Self::UiParagraph
                | Self::UiTextColumn
                | Self::UiButton
                | Self::UiLink
                | Self::UiForm
                | Self::UiImage
                | Self::UiAbove
                | Self::UiBelow
                | Self::UiOnLeft
                | Self::UiOnRight
                | Self::UiInFront
                | Self::UiBehind
                | Self::UiSpacing
                | Self::UiPadding
                | Self::UiPaddingXY
                | Self::UiPaddingEach
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
                | Self::UiClipX
                | Self::UiClipY
                | Self::UiScrollbars
                | Self::UiScrollbarX
                | Self::UiScrollbarY
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
                | Self::UiColorCss
                | Self::BackgroundColor
                | Self::BackgroundImage
                | Self::BackgroundLinearGradient
                | Self::BorderWidth
                | Self::BorderRounded
                | Self::BorderColor
                | Self::BorderWidthEach
                | Self::BorderShadow
                | Self::BorderGlow
                | Self::BorderInnerShadow
                | Self::FontSize
                | Self::FontColor
                | Self::FontFamily
                | Self::FontBold
                | Self::FontItalic
                | Self::HtmlTextNode
                | Self::HtmlRawNode
                | Self::HtmlNode
                | Self::HtmlVoidNode
                | Self::HtmlDoctype
                | Self::HtmlTitleNode
                | Self::HtmlToString
                | Self::HtmlStyleNode
                | Self::HtmlDiv
                | Self::HtmlSpan
                | Self::HtmlA
                | Self::HtmlButton
                | Self::HtmlP
                | Self::HtmlInput
                | Self::HtmlImg
                | Self::HtmlH1
                | Self::HtmlH2
                | Self::HtmlH3
                | Self::HtmlH4
                | Self::HtmlH5
                | Self::HtmlH6
                | Self::HtmlNav
                | Self::HtmlSection
                | Self::HtmlArticle
                | Self::HtmlHeader
                | Self::HtmlHeaderNode
                | Self::HtmlCodeNode
                | Self::HtmlMainNode
                | Self::HtmlFooterNode
                | Self::HtmlLinkNode
                | Self::HtmlFooter
                | Self::HtmlMain
                | Self::HtmlAside
                | Self::HtmlUl
                | Self::HtmlOl
                | Self::HtmlLi
                | Self::HtmlTable
                | Self::HtmlThead
                | Self::HtmlTbody
                | Self::HtmlTfoot
                | Self::HtmlTr
                | Self::HtmlTh
                | Self::HtmlTd
                | Self::HtmlTextarea
                | Self::HtmlSelect
                | Self::HtmlOption
                | Self::HtmlLabel
                | Self::HtmlForm
                | Self::HtmlFieldset
                | Self::HtmlLegend
                | Self::HtmlPre
                | Self::HtmlCode
                | Self::HtmlStrong
                | Self::HtmlEm
                | Self::HtmlSmall
                | Self::HtmlBlockquote
                | Self::HtmlFigure
                | Self::HtmlFigcaption
                | Self::HtmlDetails
                | Self::HtmlSummary
                | Self::HtmlDialog
                | Self::HtmlVideo
                | Self::HtmlAudio
                | Self::HtmlCanvas
                | Self::HtmlIframe
                | Self::HtmlProgress
                | Self::HtmlMeter
                | Self::HtmlScript
                | Self::HtmlBody
                | Self::HtmlTitle
                | Self::HtmlHtmlNode
                | Self::HtmlHeadNode
                | Self::HtmlBr
                | Self::HtmlHr
                | Self::HtmlMeta
                | Self::HtmlLink
                | Self::HtmlArea
                | Self::HtmlBase
                | Self::HtmlCol
                | Self::HtmlEmbed
                | Self::HtmlSource
                | Self::HtmlTrack
                | Self::HtmlWbr
                | Self::HtmlAttrClass
                | Self::HtmlAttrId
                | Self::HtmlAttrHref
                | Self::HtmlAttrSrc
                | Self::HtmlAttrAlt
                | Self::HtmlAttrValue
                | Self::HtmlAttrName
                | Self::HtmlAttrPlaceholder
                | Self::HtmlAttrType
                | Self::HtmlAttrFor
                | Self::HtmlAttrStyle
                | Self::HtmlAttrTitle
                | Self::HtmlAttrChecked
                | Self::HtmlAttrDisabled
                | Self::HtmlAttrReadonly
                | Self::HtmlAttrRequired
                | Self::HtmlAttrMultiple
                | Self::HtmlAttrSelected
                | Self::HtmlAttrAutofocus
                | Self::HtmlAttrAutocomplete
                | Self::HtmlAttribute
                | Self::HtmlBoolAttribute
                | Self::HtmlNoAttr
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
                | Self::UiOnSubmit
                | Self::UiOnFile
                | Self::HtmlOnClick
                | Self::HtmlOnFocus
                | Self::HtmlOnBlur
                | Self::HtmlOnMouseOver
                | Self::HtmlOnMouseOut
                | Self::HtmlOnSubmit
                | Self::HtmlOnInput
                | Self::HtmlOnChange
                | Self::HtmlOnKeyDown
                | Self::HtmlOnKeyUp
                | Self::HtmlOnBool
                | Self::UiSquare
                | Self::UiWidescreen
                | Self::UiCinemascope
                | Self::UiAspectRatio
                | Self::UiAspectRatioWH
                | Self::UiHtmlAttribute
                | Self::UiName
                | Self::UiStyle
                | Self::UiTransitionRaw
                | Self::UiGridTracksRaw
                | Self::UiAnimateRaw
                // ── Breakpoint ──────────────────────────────────────────
                | Self::UiBreakpoint
                | Self::UiMediaQuery
                | Self::UiMobile
                | Self::UiTablet
                | Self::UiDesktop
                | Self::UiDarkMode
                | Self::UiLightMode
                | Self::UiReducedMotion
                // ── PseudoClass opaque constants + Ui.onPseudo ────────────
                | Self::UiOnPseudo
                | Self::UiHover
                | Self::UiFocus
                | Self::UiFocusVisible
                | Self::UiActive
                | Self::UiDisabled
                | Self::BackgroundHoverColor
                | Self::BackgroundFocusColor
                | Self::BackgroundActiveColor
                | Self::BackgroundDisabledColor
                | Self::BorderSolid
                | Self::BorderDashed
                | Self::BorderDotted
                | Self::BorderHoverColor
                | Self::BorderFocusColor
                | Self::BorderActiveColor
                | Self::BorderHoverWidth
                | Self::BorderHoverRounded
                | Self::FontWeight
                | Self::FontSemiBold
                | Self::FontRegular
                | Self::FontLight
                | Self::FontExtraBold
                | Self::FontBlack
                | Self::FontUnderline
                | Self::FontNoDecoration
                | Self::FontLineThrough
                | Self::FontLetterSpacing
                | Self::FontWordSpacing
                | Self::FontAlignLeft
                | Self::FontAlignRight
                | Self::FontAlignCenter
                | Self::FontCenter
                | Self::FontJustify
                | Self::FontSansSerif
                | Self::FontSerif
                | Self::FontMonospace
                | Self::FontHoverColor
                | Self::FontFocusColor
                | Self::FontActiveColor
                | Self::FontDisabledColor
                | Self::FontHoverSize
                | Self::HtmlAttrTabindex
                | Self::HtmlAttrRows
                // ── Ipe.Ui.Region ──────────────────────────────────────
                | Self::RegionMainContent
                | Self::RegionNavigation
                | Self::RegionFooter
                | Self::RegionAside
                | Self::RegionHeading
                | Self::RegionLabel
                | Self::RegionAnnounce
                | Self::RegionAnnounceUrgently
                // ── Ui.input + Ui.describe + desc* constructors ──────────
                | Self::UiInput
                | Self::UiDescribe
                | Self::UiDescMain
                | Self::UiDescNavigation
                | Self::UiDescContentInfo
                | Self::UiDescComplementary
                | Self::UiDescLivePolite
                | Self::UiDescLiveAssertive
                | Self::UiDescHeading
                | Self::UiDescLabel
                // ── Ipe.Ui.Input ───────────────────────────────────────
                | Self::InputLabelAbove
                | Self::InputLabelBelow
                | Self::InputLabelLeft
                | Self::InputLabelRight
                | Self::InputLabelHidden
                | Self::InputPlaceholder
                | Self::InputText
                | Self::InputMultiline
                | Self::InputEmail
                | Self::InputUsername
                | Self::InputSearch
                | Self::InputCurrentPassword
                | Self::InputNewPassword
                | Self::InputCheckbox
                | Self::InputSlider
                | Self::InputOption
                | Self::InputRadio
                | Self::InputRadioRow
                // ── Ipe.Ui.Lazy ────────────────────────────────────────
                | Self::LazyLazy
                | Self::LazyLazy2
                | Self::LazyLazy3
                | Self::LazyLazy4
                | Self::LazyLazy5
                // ── Ipe.Ui.Keyed ────────────────────────────────────────────
                | Self::KeyedColumn
                | Self::KeyedRow
        )
    }

    /// The fixed wire event name for a `Ipe.Html.Events` builder (`onClick` →
    /// `"click"`). `None` for any non-Html-event variant. The name is a
    /// compile-time constant (never attacker data) that the emit arm passes to
    /// the `html_on_*_` runtime constructor.
    #[must_use]
    pub const fn html_event_wire_name(self) -> Option<&'static str> {
        Some(match self {
            Self::HtmlOnClick => "click",
            Self::HtmlOnFocus => "focus",
            Self::HtmlOnBlur => "blur",
            Self::HtmlOnMouseOver => "mouseover",
            Self::HtmlOnMouseOut => "mouseout",
            Self::HtmlOnSubmit => "submit",
            Self::HtmlOnInput => "input",
            Self::HtmlOnKeyDown => "keydown",
            Self::HtmlOnKeyUp => "keyup",
            // `onBool` mirrors `Ipe.Html.Events.onCheck` — the checkbox check
            // state arrives on the `change` DOM event, same wire name as
            // `onChange`.
            Self::HtmlOnChange | Self::HtmlOnBool => "change",
            _ => return None,
        })
    }

    /// The event payload shape of a `Ipe.Html.Events` builder, driving both the
    /// constrain scheme and the emit arm. `None` for any non-Html-event variant.
    #[must_use]
    pub const fn html_event_shape(self) -> Option<HtmlEventShape> {
        Some(match self {
            Self::HtmlOnClick
            | Self::HtmlOnFocus
            | Self::HtmlOnBlur
            | Self::HtmlOnMouseOver
            | Self::HtmlOnMouseOut => HtmlEventShape::Msg,
            Self::HtmlOnInput | Self::HtmlOnChange | Self::HtmlOnKeyDown | Self::HtmlOnKeyUp => {
                HtmlEventShape::String
            }
            Self::HtmlOnBool => HtmlEventShape::Bool,
            Self::HtmlOnSubmit => HtmlEventShape::Raw,
            _ => return None,
        })
    }

    /// `true` for a kernel whose Rust runtime consumer requires its
    /// function-valued argument to be `Send + Sync` — either an
    /// `Arc<dyn Fn(..) -> .. + Send + Sync + 'static>` runtime slot
    /// (`ui_on_input_`/`ui_on_change_`/…, `html_on_string_`/`html_on_bool_`/
    /// `html_on_raw_`) or a generic `F: .. + Send + Sync + 'static` bound
    /// (`ui_on_submit_`, `server_stream_stream`) — NOT merely `Send`
    /// (`Box<dyn Fn(..) -> .. + Send + 'static>`, which is how a generic
    /// `IrType::Fun` renders in `emit_types.rs`).
    ///
    /// The emit-site "re-wrap the payload in a freshly-declared closure"
    /// technique (`ipe_backend_rust::emit_expr`'s `KernelFn::UiOnSubmit` /
    /// `HtmlEventShape::Raw` / `StreamStream` arms) only launders a
    /// MISSING `+Sync` bound when the payload is constructed INLINE at the call
    /// site (a literal `Lambda`/`FuncValue` — the box is rebuilt fresh, as
    /// source, inside the wrapper's body on every call, so it never enters the
    /// wrapper's own captured environment). A `Var`/`CloneVar` referencing an
    /// ALREADY-BUILT `let`-bound closure is a different shape: the wrapper
    /// closure captures that already-existing value BY MOVE, and Rust's
    /// auto-trait inference is structural over every captured field — a
    /// captured `Box<dyn Fn + Send>` (never `+Sync`) makes the wrapper itself
    /// non-`Sync`, no matter how the wrapper's body is written. Re-wrapping
    /// cannot launder a missing trait bound on a value that already exists.
    ///
    /// This predicate is consulted by
    /// `ipe_lower::flows_into_sync_kernel_call` (from `lower_let_pvar`,
    /// alongside the `needs_shared_capture` nested/sibling check) to decide
    /// whether a `let`-bound function-typed local must be
    /// promoted to `Expr::SharedLambda` — emitted as
    /// `Arc<dyn Fn(..) -> .. + Send + Sync + 'static>` — even for a single,
    /// non-nested use. Unlike `needs_shared_capture`'s trigger (2+ competing
    /// closure captures), a SINGLE occurrence here is already sufficient: the
    /// runtime callback slot's `+Sync` bound applies however many times the
    /// value is referenced.
    ///
    /// Deliberately excludes the WebSocket server-config callbacks and the
    /// `Ipe.Http.Server` request-handler shape: both are ALREADY immune by a
    /// different, structural mechanism —
    /// `ipe_backend_rust::emit_expr::wants_arc_ctor` recognises their FIXED
    /// closure shape at the closure's OWN construction site and boxes with
    /// `Arc::new` there, regardless of inline-vs-`let`-bound. `Ui.on*` /
    /// `Ipe.Html.Events.on*` / `Stream.stream` have no such fixed structural
    /// shape (their callback's argument/return type is the app's own
    /// polymorphic `msg`), so they need this USAGE-SITE detection instead.
    #[must_use]
    pub const fn requires_sync_capture(self) -> bool {
        matches!(
            self,
            Self::UiOnInput
                | Self::UiOnChange
                | Self::UiOnKeyDown
                | Self::UiOnKeyUp
                | Self::UiOnFile
                | Self::UiOnBool
                | Self::UiOnSubmit
                | Self::HtmlOnInput
                | Self::HtmlOnChange
                | Self::HtmlOnKeyDown
                | Self::HtmlOnKeyUp
                | Self::HtmlOnBool
                | Self::HtmlOnSubmit
                | Self::StreamStream
        )
    }

    /// `true` for a `Ipe.Html.Attributes` string-valued fixed-key builder
    /// (`class`/`id`/… — `String -> Attribute msg`). Used by the backend emit
    /// arm to route through `html_named_attr_` with the wire key from
    /// [`Self::html_attr_key`].
    #[must_use]
    pub const fn is_html_str_attr(self) -> bool {
        matches!(
            self,
            Self::HtmlAttrClass
                | Self::HtmlAttrId
                | Self::HtmlAttrHref
                | Self::HtmlAttrSrc
                | Self::HtmlAttrAlt
                | Self::HtmlAttrValue
                | Self::HtmlAttrName
                | Self::HtmlAttrPlaceholder
                | Self::HtmlAttrType
                | Self::HtmlAttrFor
                | Self::HtmlAttrStyle
                | Self::HtmlAttrTitle
                | Self::HtmlAttrAutocomplete
        )
    }

    /// `true` for a `Ipe.Html.Attributes` bool-valued fixed-key builder
    /// (`checked`/`disabled`/… — `Bool -> Attribute msg`).
    #[must_use]
    pub const fn is_html_bool_attr(self) -> bool {
        matches!(
            self,
            Self::HtmlAttrChecked
                | Self::HtmlAttrDisabled
                | Self::HtmlAttrReadonly
                | Self::HtmlAttrRequired
                | Self::HtmlAttrMultiple
                | Self::HtmlAttrSelected
                | Self::HtmlAttrAutofocus
        )
    }

    /// The wire attribute name for a fixed-key `Ipe.Html.Attributes` builder.
    /// Matches the member name except for the two Ipê-keyword-avoidance
    /// spellings `type_`→`type` and `for_`→`for`. `None` for any non-fixed-key
    /// variant (the generic `attribute`/`boolAttribute` carry the key as a
    /// runtime argument, `noAttr` has none).
    #[must_use]
    pub const fn html_attr_key(self) -> Option<&'static str> {
        Some(match self {
            Self::HtmlAttrClass => "class",
            Self::HtmlAttrId => "id",
            Self::HtmlAttrHref => "href",
            Self::HtmlAttrSrc => "src",
            Self::HtmlAttrAlt => "alt",
            Self::HtmlAttrValue => "value",
            Self::HtmlAttrName => "name",
            Self::HtmlAttrPlaceholder => "placeholder",
            Self::HtmlAttrType => "type",
            Self::HtmlAttrFor => "for",
            Self::HtmlAttrStyle => "style",
            Self::HtmlAttrTitle => "title",
            Self::HtmlAttrChecked => "checked",
            Self::HtmlAttrDisabled => "disabled",
            Self::HtmlAttrReadonly => "readonly",
            Self::HtmlAttrRequired => "required",
            Self::HtmlAttrMultiple => "multiple",
            Self::HtmlAttrSelected => "selected",
            Self::HtmlAttrAutofocus => "autofocus",
            Self::HtmlAttrAutocomplete => "autocomplete",
            _ => return None,
        })
    }

    /// `true` for a Ipe.Html CONTAINER element builder
    /// (`h1`/`nav`/`table`/... — `List Attr -> List Html -> Html msg`). The
    /// backend emit arm routes these through `html_node_(tag, attrs, children)`
    /// with the wire tag from [`Self::html_element_tag`]. Excludes the older
    /// per-tag kernels (`HtmlDiv`/`HtmlSpan`/`HtmlA`/`HtmlButton`/`HtmlP`).
    #[must_use]
    pub const fn is_html_container(self) -> bool {
        matches!(
            self,
            Self::HtmlH1
                | Self::HtmlH2
                | Self::HtmlH3
                | Self::HtmlH4
                | Self::HtmlH5
                | Self::HtmlH6
                | Self::HtmlNav
                | Self::HtmlSection
                | Self::HtmlArticle
                | Self::HtmlHeader
                | Self::HtmlHeaderNode
                | Self::HtmlCodeNode
                | Self::HtmlMainNode
                | Self::HtmlFooterNode
                | Self::HtmlFooter
                | Self::HtmlMain
                | Self::HtmlAside
                | Self::HtmlUl
                | Self::HtmlOl
                | Self::HtmlLi
                | Self::HtmlTable
                | Self::HtmlThead
                | Self::HtmlTbody
                | Self::HtmlTfoot
                | Self::HtmlTr
                | Self::HtmlTh
                | Self::HtmlTd
                | Self::HtmlTextarea
                | Self::HtmlSelect
                | Self::HtmlOption
                | Self::HtmlLabel
                | Self::HtmlForm
                | Self::HtmlFieldset
                | Self::HtmlLegend
                | Self::HtmlPre
                | Self::HtmlCode
                | Self::HtmlStrong
                | Self::HtmlEm
                | Self::HtmlSmall
                | Self::HtmlBlockquote
                | Self::HtmlFigure
                | Self::HtmlFigcaption
                | Self::HtmlDetails
                | Self::HtmlSummary
                | Self::HtmlDialog
                | Self::HtmlVideo
                | Self::HtmlAudio
                | Self::HtmlCanvas
                | Self::HtmlIframe
                | Self::HtmlProgress
                | Self::HtmlMeter
                | Self::HtmlScript
                | Self::HtmlBody
                | Self::HtmlTitle
                | Self::HtmlHtmlNode
                | Self::HtmlHeadNode
        )
    }

    /// `true` for a Ipe.Html VOID element builder
    /// (`br`/`hr`/`meta`/`link`/... — `List Attr -> Html msg`, no children).
    /// The emit arm routes these through `html_node_(tag, attrs, vec![])`; the
    /// render sink self-closes any tag in its `VOID` set and drops children, so
    /// passing an empty child vec is belt-and-braces. Excludes `HtmlInput`/`HtmlImg`.
    #[must_use]
    pub const fn is_html_void(self) -> bool {
        matches!(
            self,
            Self::HtmlBr
                | Self::HtmlHr
                | Self::HtmlMeta
                | Self::HtmlLink
                | Self::HtmlLinkNode
                | Self::HtmlArea
                | Self::HtmlBase
                | Self::HtmlCol
                | Self::HtmlEmbed
                | Self::HtmlSource
                | Self::HtmlTrack
                | Self::HtmlWbr
        )
    }

    /// The wire tag name for a Ipe.Html element builder
    /// (container or void). `None` for any non-element variant. The name is the
    /// HTML tag emitted verbatim as the first `html_node_` argument.
    #[must_use]
    pub const fn html_element_tag(self) -> Option<&'static str> {
        Some(match self {
            Self::HtmlH1 => "h1",
            Self::HtmlH2 => "h2",
            Self::HtmlH3 => "h3",
            Self::HtmlH4 => "h4",
            Self::HtmlH5 => "h5",
            Self::HtmlH6 => "h6",
            Self::HtmlNav => "nav",
            Self::HtmlSection => "section",
            Self::HtmlArticle => "article",
            Self::HtmlHeader | Self::HtmlHeaderNode => "header",
            Self::HtmlFooter | Self::HtmlFooterNode => "footer",
            Self::HtmlMain | Self::HtmlMainNode => "main",
            Self::HtmlAside => "aside",
            Self::HtmlUl => "ul",
            Self::HtmlOl => "ol",
            Self::HtmlLi => "li",
            Self::HtmlTable => "table",
            Self::HtmlThead => "thead",
            Self::HtmlTbody => "tbody",
            Self::HtmlTfoot => "tfoot",
            Self::HtmlTr => "tr",
            Self::HtmlTh => "th",
            Self::HtmlTd => "td",
            Self::HtmlTextarea => "textarea",
            Self::HtmlSelect => "select",
            Self::HtmlOption => "option",
            Self::HtmlLabel => "label",
            Self::HtmlForm => "form",
            Self::HtmlFieldset => "fieldset",
            Self::HtmlLegend => "legend",
            Self::HtmlPre => "pre",
            Self::HtmlCode | Self::HtmlCodeNode => "code",
            Self::HtmlStrong => "strong",
            Self::HtmlEm => "em",
            Self::HtmlSmall => "small",
            Self::HtmlBlockquote => "blockquote",
            Self::HtmlFigure => "figure",
            Self::HtmlFigcaption => "figcaption",
            Self::HtmlDetails => "details",
            Self::HtmlSummary => "summary",
            Self::HtmlDialog => "dialog",
            Self::HtmlVideo => "video",
            Self::HtmlAudio => "audio",
            Self::HtmlCanvas => "canvas",
            Self::HtmlIframe => "iframe",
            Self::HtmlProgress => "progress",
            Self::HtmlMeter => "meter",
            Self::HtmlScript => "script",
            Self::HtmlBody => "body",
            Self::HtmlTitle => "title",
            Self::HtmlHtmlNode => "html",
            Self::HtmlHeadNode => "head",
            Self::HtmlBr => "br",
            Self::HtmlHr => "hr",
            Self::HtmlMeta => "meta",
            Self::HtmlLink | Self::HtmlLinkNode => "link",
            Self::HtmlArea => "area",
            Self::HtmlBase => "base",
            Self::HtmlCol => "col",
            Self::HtmlEmbed => "embed",
            Self::HtmlSource => "source",
            Self::HtmlTrack => "track",
            Self::HtmlWbr => "wbr",
            _ => return None,
        })
    }

    /// `true` when this variant belongs to the `Ipe.Web` app-entry subsystem.
    #[must_use]
    pub const fn is_web(self) -> bool {
        matches!(
            self,
            Self::WebApp
                | Self::WebAppRouted
                | Self::WebRoute
                | Self::WebRenderStatic
                // The Task-shaped `PubSub.publish` / `publishNoEcho` are not
                // app-entry kernels, but they share the `web` module: their
                // symbols live in `ipe_runtime::live::pubsub` (gated by the `live`
                // Cargo feature). A program that uses either — even without a
                // Web.app — must have the `live` feature enabled so
                // `pubsub_publish` / `pubsub_publish_no_echo` are in scope.
                | Self::PubSubPublish
                | Self::PubSubPublishNoEcho
        )
    }

    /// `true` when this variant is the `Ipe.Terminal` full-screen app-entry.
    #[must_use]
    pub const fn is_tui(self) -> bool {
        matches!(self, Self::TerminalAppScreen)
    }

    /// `true` when this variant is the `Ipe.WebView` app-entry kernel.
    #[must_use]
    pub const fn is_webview(self) -> bool {
        matches!(self, Self::WebViewApp)
    }

    /// `true` when this variant is the `Ipe.Terminal` line-oriented app-entry.
    #[must_use]
    pub const fn is_console(self) -> bool {
        matches!(self, Self::TerminalAppLines)
    }

    /// `true` when this variant belongs to the `Ipe.CssSafety` leaf
    /// security-kernel family (the `Ipe.Css` backing): `safe_value` /
    /// `safe_prop_name` / `safe_selector` / `strip_style_close_kernel`.
    ///
    /// These kernels live in `ipe_runtime::css` (which glob-re-exports their
    /// bare names) and depend only on `ipe_runtime::css_safety`. A program that
    /// uses `Ipe.Css` WITHOUT any `Ipe.Ui` / `Ipe.Html` kernel does NOT set
    /// `uses_ui`, so the backend consults this predicate to decide whether the
    /// emitted `ipe_runtime/mod.rs` must declare `css_safety` / `css` (and
    /// `pub use css::*`) on its own — otherwise the bare `safe_value` … names
    /// `naming::kernel_name` emits are out of scope (E0425).
    #[must_use]
    pub const fn is_css(self) -> bool {
        matches!(
            self,
            Self::CssSafetySafeValue
                | Self::CssSafetySafePropName
                | Self::CssSafetySafeSelector
                | Self::CssSafetyStripStyleClose
        )
    }
}

// ── Two-tier kernel identity ─────────────────────────────────────────────────

/// Opaque identifier for a user-provided FFI binding.
///
/// Reserved. The landed FFI consumer wiring realises the open registry
/// WITHOUT a kernel-tier id: each bound crate becomes a driver-generated,
/// fully-annotated `Rust.<Crate>` interface module
/// (`ipe_canon::resolve::ModuleOrigin::FfiInterface`) whose forwarder bodies
/// lower to `ipe_ir::Callee::Ffi { ident }` — FFI signatures ride the ONE
/// existing annotation → `Ty` path, so there is no second scheme table for
/// this id to index. The variant stays reserved for a future need to
/// register an FFI binding at the KERNEL tier (e.g. a stdlib-visible alias
/// onto a bound crate); constructors are deliberately unexposed until that
/// consumer exists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FfiKernelId(u32);

/// A fully-resolved kernel function.
///
/// Either a known stdlib kernel (resolved at canonicalisation time) or a
/// user-provided FFI binding (reserved — see [`FfiKernelId`] for why the
/// landed FFI wiring does not mint these).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelId {
    /// A known stdlib kernel.
    Stdlib(StdlibKernel),
    /// A user-provided FFI binding (reserved).
    Ffi(FfiKernelId),
}

// ── Compilation target — kernel availability ──────────────────────────────────

/// The compilation target a build resolves kernels against.
///
/// `WasmClient` is a public browser bundle: every kernel is DENIED there
/// unless [`StdlibKernel::available_on`] explicitly allows it (default-deny —
/// a newly added kernel is unrepresentable client-side until audited and
/// allowed, so the forgotten state is the safe state; see
/// `docs/adr/0042-wasm-client-target.md` Q5 Layer 1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Target {
    /// The native host binary (server / CLI / TUI / desktop).
    #[default]
    Native,
    /// A browser WASM bundle (`ipe build --target wasm`) — fully public,
    /// `wasm2wat`-inspectable; no server effect or secret may compile in.
    WasmClient,
}

impl StdlibKernel {
    /// Whether this kernel has a denotation on `target`.
    ///
    /// Everything is available natively. The `WasmClient` arm is the
    /// default-deny allowlist over the capability matrix
    /// (`docs/adr/0042-wasm-client-target.md` Q3): the pure/fallible-pure
    /// families plus the whole `Ipe.Ui`/`Ipe.Html`/`Ipe.Css` render surface
    /// compile wholesale; effect kernels appear here ONLY once their browser
    /// substitute exists in the runtime `wasm` module (tagging earlier would
    /// break THE SEAL — the name would resolve with no symbol to link).
    #[must_use]
    pub fn available_on(self, target: Target) -> bool {
        match target {
            Target::Native => true,
            Target::WasmClient => self.wasm_client_available(),
        }
    }

    /// The `WasmClient` allowlist. The catch-all `false` arm IS the
    /// default-deny invariant — never widen it to a family without a probe
    /// build proving the family's runtime module compiles to wasm32.
    fn wasm_client_available(self) -> bool {
        let decl = self.decl();
        match decl.class {
            // The whole render surface (Ui/Html/Attr/Event/Font/Border/
            // Background/Input/Region/Lazy/Keyed) — probe-verified to
            // compile to wasm32 as part of the runtime floor.
            KernelClass::Ui => true,
            // `Web.app` gains a browser denotation via the runtime `wasm`
            // sink (`wasm_app`). Routed apps (`Web.route` / the routed
            // branch) stay out until the client router lands — tagging the
            // route kernel now would emit `Route::new` against a runtime
            // module the wasm crate does not vendor (a SEAL breach).
            // `PubSub.publish` / `publishNoEcho` are `class = Web` (Task-shaped,
            // not TEA-loop) and route through the in-tab broker (`wasm::pubsub`),
            // the same M4 Cmd/Sub browser-effects bridge the TEA-side pub/sub uses.
            KernelClass::Web => matches!(
                self,
                Self::WebApp | Self::PubSubPublish | Self::PubSubPublishNoEcho
            ),
            // TEA wiring the wasm scheduler drives today. `Cmd.perform` runs
            // on the browser microtask queue; `Sub.every`/`Time.every` run on
            // `gloo-timers` (`wasm::subs::SubManager`); `Cmd.publish` /
            // `Cmd.publishNoEcho` / `Sub.subscribeTopic` route through the in-tab
            // broker (`wasm::pubsub`) — the M4 Cmd/Sub browser-effects bridge.
            // (The Task-shaped `PubSub.publish` / `publishNoEcho` are `class = Web`
            // and handled in the `KernelClass::Web` arm above.)
            // `SubSubscribeWebSocket` (the WebSocket client's onOpen/
            // onMessage/onClose/onError receive surface) routes through
            // `ws_client.rs`'s wasm32 arm — `web_sys::WebSocket`'s
            // `onopen`/`onmessage`/`onclose`/`onerror` handler slots.
            KernelClass::Tea => matches!(
                self,
                Self::CmdNone
                    | Self::CmdBatch
                    | Self::CmdPerform
                    | Self::CmdMap
                    | Self::TaskAttempt
                    | Self::SubNone
                    | Self::SubBatch
                    | Self::SubEvery
                    | Self::SubMap
                    | Self::TimeEvery
                    | Self::CmdPublish
                    | Self::CmdPublishNoEcho
                    | Self::SubSubscribeTopic
                    | Self::SubSubscribeWebSocket
            ),
            KernelClass::Pure => {
                // Pure families whose runtime modules are in the proven wasm
                // floor (no host I/O, no tokio, no un-shimmed entropy) OR
                // whose M4 browser substitute has landed:
                //   - `Log` → `console.{debug,info,warn,error}` (log.rs).
                //   - `Random` → `crypto.getRandomValues` via getrandom(js)
                //     (random.rs's `lcg_init` wasm arm) — all 3 registered
                //     kernels (int/float/choice) share the one entropy fix.
                //   - `Http` → `fetch` (http_client.rs); this qualifier ALSO
                //     covers the header/UninitialisedRequest builder kernels
                //     (`defaultRequest`/`withMethod`/…), which have no
                //     runtime symbol at all (inline `HttpRequest{..}` struct
                //     literals in `emit_expr.rs`) and so carry no wasm risk.
                matches!(
                    decl.qualifier,
                    "String"
                        | "Char"
                        | "List"
                        | "Basics"
                        | "Math"
                        | "Dict"
                        | "Set"
                        | "Maybe"
                        | "Result"
                        | "Error"
                        | "Bytes"
                        | "Encoding"
                        | "JsonEnc"
                        | "JsonDec"
                        | "JsonDecP"
                        | "Decimal"
                        | "Regex"
                        | "Path"
                        | "Secret"
                        | "CssSafety"
                        | "Uuid"
                        | "Log"
                        | "Random"
                        | "Http"
                ) ||
                // Pure calendar helpers (chrono, no clock read) PLUS the M4
                // `Date.now()`/`setTimeout` clock+sleep substitutes.
                matches!(
                    self,
                    Self::TimeTimeString
                        | Self::TimeIsLeapYear
                        | Self::TimeDaysInMonth
                        | Self::TimeNow
                        | Self::TimeSleep
                        | Self::TimeUnixMillis
                ) ||
                // `Crypto.randomBytes`/`randomToken` — `crypto.getRandomValues`
                // via getrandom(js) (crypto.rs's wasm32 arm). Every OTHER
                // `Crypto` kernel (hashing, AEAD, RSA, PBKDF2) stays denied —
                // deliberately NOT a qualifier-wide allow.
                matches!(self, Self::CryptoRandomBytes | Self::CryptoRandomToken) ||
                // `Ipe.WebSocket` client Task-tier — `web_sys::WebSocket`
                // (ws_client.rs's wasm32 arm). The Sub-tier receive kernel
                // (`SubSubscribeWebSocket`) is `Tea`-classed, not `Pure` —
                // see the `KernelClass::Tea` arm above.
                matches!(
                    self,
                    Self::WebSocketConnect
                        | Self::WebSocketConnectWith
                        | Self::WebSocketSend
                        | Self::WebSocketSendBinary
                        | Self::WebSocketClose
                        | Self::WebSocketCloseWithCode
                ) ||
                // `Task.*` pure future combinators (`task.rs`'s ungated half —
                // no tokio dependency, just `Box::pin(async move { .. })` over
                // an already-`IpeTask`). Required for the M4 bridge to be
                // usable at all: `Ipe.WebSocket.connect`/`Http.get`'s own
                // stdlib wrappers (`Task.map`, …) call these, so every
                // Cmd.perform pipeline routes through at least `Task.map`.
                // `Task.run`/`Task.parallel`/`Task.retryWith`/`Task.perform`
                // stay denied — their runtime bodies are tokio-bound
                // (`block_on`/`tokio::spawn`/`tokio::time::sleep`) and have no
                // wasm arm.
                matches!(
                    self,
                    Self::TaskSucceed
                        | Self::TaskFail
                        | Self::TaskMap
                        | Self::TaskMap2
                        | Self::TaskMap3
                        | Self::TaskMap4
                        | Self::TaskMap5
                        | Self::TaskAndThen
                        | Self::TaskMapError
                        | Self::TaskOnError
                        | Self::TaskFromResult
                        | Self::TaskAndThenResult
                        | Self::TaskSequence
                ) ||
                // `Env.public` — build-time-embedded `[wasm] publicEnv`
                // allowlist (`option_env!` on wasm32; the SAME allowlist via
                // `std::env::var` natively — `env_public.rs`, backend-
                // generated per project, never vendored from the source tree).
                matches!(self, Self::EnvPublic)
            }
            // Server-only surfaces: no browser denotation, ever (Db/Server)
            // or until a dedicated backend exists (Terminal/WebView/Ffi).
            KernelClass::Db
            | KernelClass::Server
            | KernelClass::Terminal
            | KernelClass::WebView
            | KernelClass::Ffi => false,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::StdlibKernel;

    /// Every wired kernel is callable through `capability()` (the exhaustive
    /// match is total over the whole registry — no panic, no gap). The compile
    /// error on a missing arm is the real drift guarantee; this asserts the
    /// method is live over `ALL`.
    #[test]
    fn every_wired_kernel_has_a_capability_decision() {
        for k in StdlibKernel::ALL {
            let _ = k.capability();
        }
    }

    /// One representative kernel per effect family maps to the right capability,
    /// and a pure kernel maps to `None`.
    #[test]
    fn effect_kernels_map_to_their_capability() {
        use super::Capability;
        assert_eq!(
            StdlibKernel::HttpGet.capability(),
            Some(Capability::Network)
        );
        assert_eq!(
            StdlibKernel::ServerListen.capability(),
            Some(Capability::Network)
        );
        assert_eq!(
            StdlibKernel::EmailSend.capability(),
            Some(Capability::Network)
        );
        assert_eq!(
            StdlibKernel::FileReadFile.capability(),
            Some(Capability::Filesystem)
        );
        assert_eq!(
            StdlibKernel::DbQuery.capability(),
            Some(Capability::Database)
        );
        assert_eq!(
            StdlibKernel::DbDecString.capability(),
            Some(Capability::Database)
        );
        assert_eq!(
            StdlibKernel::SystemGetenv.capability(),
            Some(Capability::Env)
        );
        assert_eq!(StdlibKernel::TimeNow.capability(), Some(Capability::Clock));
        assert_eq!(
            StdlibKernel::RandomInt.capability(),
            Some(Capability::Random)
        );
        assert_eq!(StdlibKernel::UuidV4.capability(), Some(Capability::Random));
        assert_eq!(StdlibKernel::StringToUpper.capability(), None);
        assert_eq!(StdlibKernel::LogInfo.capability(), None);
        assert_eq!(StdlibKernel::IoPrintln.capability(), None);
        assert_eq!(StdlibKernel::DebugLog.capability(), None);
        // `Env.public` reads a build-time constant, not the live environment.
        assert_eq!(StdlibKernel::EnvPublic.capability(), None);
    }

    /// The `WasmClient` allowlist is default-deny: every server-effect family
    /// is denied and the pure floor + render surface is allowed.
    #[test]
    fn wasm_client_allowlist_is_default_deny() {
        use super::Target;
        // Crown-jewel denials (secret consumers / server surfaces / effects
        // whose browser substitute has not landed).
        for denied in [
            StdlibKernel::AuthSignToken,
            StdlibKernel::AuthVerifyToken,
            StdlibKernel::DbQuery,
            StdlibKernel::DbConnect,
            StdlibKernel::FileReadFile,
            StdlibKernel::ProcessRun,
            StdlibKernel::SystemGetenv,
            StdlibKernel::SystemExit,
            StdlibKernel::ServerListen,
            StdlibKernel::EmailSend,
            StdlibKernel::IoReadLine,
            StdlibKernel::TaskPerform,
            StdlibKernel::WebRenderStatic,
            StdlibKernel::WebRoute,
            // Crypto: only the entropy pair (`randomBytes`/`randomToken`) has
            // a wasm substitute; hashing/AEAD/RSA stay denied (M4 scope cut,
            // NOT a qualifier-wide allow — see `wasm_client_available`).
            StdlibKernel::CryptoSha256,
            StdlibKernel::CryptoAesGcmEncrypt,
            StdlibKernel::CryptoAesKeyFromPassword,
        ] {
            assert!(
                !denied.available_on(Target::WasmClient),
                "{denied:?} must have no wasm-client denotation"
            );
        }
        // The floor + the headline render surface + the M4 Cmd/Sub browser
        // effects bridge (Log/Random/Http/WebSocket substitutes, timers,
        // in-tab pub/sub).
        for allowed in [
            StdlibKernel::StringFromInt,
            StdlibKernel::ListMap,
            StdlibKernel::DictInsert,
            StdlibKernel::JsonDecDecodeString,
            StdlibKernel::DecAdd,
            StdlibKernel::UiLayout,
            StdlibKernel::UiButton,
            StdlibKernel::HtmlDiv,
            StdlibKernel::CssSafetySafeValue,
            StdlibKernel::WebApp,
            StdlibKernel::CmdNone,
            StdlibKernel::CmdPerform,
            StdlibKernel::SubNone,
            StdlibKernel::LogInfo,
            StdlibKernel::LogErrorWith,
            StdlibKernel::RandomInt,
            StdlibKernel::RandomFloat,
            StdlibKernel::RandomChoice,
            StdlibKernel::CryptoRandomBytes,
            StdlibKernel::CryptoRandomToken,
            StdlibKernel::HttpGet,
            StdlibKernel::HttpPost,
            StdlibKernel::HttpRequest,
            StdlibKernel::HttpParseQuery,
            StdlibKernel::TimeNow,
            StdlibKernel::TimeSleep,
            StdlibKernel::TimeUnixMillis,
            StdlibKernel::SubEvery,
            StdlibKernel::TimeEvery,
            StdlibKernel::CmdPublish,
            StdlibKernel::CmdPublishNoEcho,
            StdlibKernel::SubSubscribeTopic,
            StdlibKernel::PubSubPublish,
            StdlibKernel::PubSubPublishNoEcho,
            StdlibKernel::WebSocketConnect,
            StdlibKernel::WebSocketSend,
            StdlibKernel::WebSocketClose,
            // The WebSocket client's Sub-tier receive surface —
            // `ws_client.rs`'s wasm32 arm now wires `onOpen`/`onMessage`/
            // `onClose`/`onError` via `web_sys::WebSocket`'s `onopen`/
            // `onmessage`/`onclose`/`onerror` handler slots.
            StdlibKernel::SubSubscribeWebSocket,
            // `Env.public` — build-time-embedded `[wasm] publicEnv` allowlist.
            StdlibKernel::EnvPublic,
        ] {
            assert!(
                allowed.available_on(Target::WasmClient),
                "{allowed:?} must be wasm-client-representable"
            );
        }
        // Everything is available natively.
        for &sk in StdlibKernel::ALL {
            assert!(sk.available_on(Target::Native));
        }
    }

    /// Verifies that no two non-internal variants in [`StdlibKernel::ALL`] share
    /// the same `(qualifier, name)` pair.
    ///
    /// A collision in `decl()` would let `stdlib_index`'s silent last-wins insert
    /// silently alias one variant onto another, making `id = Some(k)` ambiguous:
    /// the variant stored in the index would not necessarily be the one `decl()`
    /// names, and the `stdlib_index` fast path would fire with the wrong
    /// variant.
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

    /// `PubSub.publish` / `publishNoEcho` have `class = Tea` and their emitted
    /// symbols (`pubsub_publish`, `pubsub_publish_no_echo`) live in
    /// `ipe_runtime::live::pubsub` — the `live` feature-module.  This test is
    /// the SSOT invariant: `required_runtime_module` MUST return
    /// `Some(RuntimeModule::Web)` for both so that any future code path relying
    /// solely on this function (rather than `is_web`) cannot silently omit the
    /// `live` append and produce an E0425 at `cargo build` time.
    #[test]
    fn pubsub_kernels_require_live_module() {
        use super::RuntimeModule;

        assert_eq!(
            StdlibKernel::PubSubPublish.required_runtime_module(),
            Some(RuntimeModule::Web),
            "PubSubPublish must map to RuntimeModule::Web — \
             pubsub_publish is defined in ipe_runtime::live::pubsub"
        );
        assert_eq!(
            StdlibKernel::PubSubPublishNoEcho.required_runtime_module(),
            Some(RuntimeModule::Web),
            "PubSubPublishNoEcho must map to RuntimeModule::Web — \
             pubsub_publish_no_echo is defined in ipe_runtime::live::pubsub"
        );
    }
}
