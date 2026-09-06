#[cfg(test)]
mod registry_phase_c_tests {
    use super::super::{
        Builder, Builtins, Content, Diagnostic, Feature, LowerError, Ty, UnionFind,
    };
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
            K::BackoffLinear,
            K::BackoffLinearWithJitter,
            K::BackoffExponential,
            K::BackoffExponentialWithJitter,
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
            K::TimeFormat,
            K::TimeFormatHTTP,
            K::TimeFormatISO8601,
            K::TimeFormatRFC3339,
            K::TimeAddMillis,
            K::TimeDiffMillis,
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
            K::SystemGetcwd,
            K::SystemLoadEnv,
            K::SystemExit,
            // Random (6)
            K::RandomInt,
            K::RandomFloat,
            K::RandomChoice,
            K::RandomChoiceMaybe,
            K::RandomShuffle,
            K::RandomWeighted,
            // File (17)
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
            K::FileWalk,
            K::FileWalkMatching,
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
            K::HttpWithRedirects,
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
            // Server (30)
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
            K::ServerAuthConfig,
            K::ServerTokenBearer,
            K::ServerCookieToken,
            K::ServerGetAuthed,
            K::ServerPostAuthed,
            K::ServerPutAuthed,
            K::ServerDeleteAuthed,
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
    ///   parity).
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
            // Json.Decode (18)
            K::JsonDecString,
            K::JsonDecInt,
            K::JsonDecFloat,
            K::JsonDecBool,
            K::JsonDecDecodeString,
            K::JsonDecField,
            K::JsonDecAt,
            K::JsonDecIndex,
            K::JsonDecList,
            K::JsonDecNullable,
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
            // Debug.todo / Debug.explain — Ipê-new dev-only; no legacy oracle.
            K::DebugTodo,
            K::DebugExplain,
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
            // path is UTF-8 (parity); byte round-tripping lives in
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
            K::UiCellsNone,
            K::UiCellsText,
            K::UiCellsEl,
            K::UiCellsRow,
            K::UiCellsColumn,
            K::UiCellsCells,
            K::TuiUiSpacing,
            K::TuiUiPadding,
            K::TuiUiAlignLeft,
            K::TuiUiAlignRight,
            K::TuiUiCenter,
            K::TuiUiBold,
            K::TuiUiUnderline,
            K::TuiUiDim,
            K::TuiUiReverse,
            K::TuiUiColor,
            K::TuiUiBg,
            K::CliUiNone,
            K::CliUiText,
            K::CliUiLine,
            K::CliUiLines,
            K::CliUiBold,
            K::CliUiUnderline,
            K::CliUiDim,
            K::CliUiReverse,
            K::CliUiColor,
            K::CliUiBg,
            K::TermColorBlack,
            K::TermColorRed,
            K::TermColorGreen,
            K::TermColorYellow,
            K::TermColorBlue,
            K::TermColorMagenta,
            K::TermColorCyan,
            K::TermColorWhite,
            K::TermColorBrightBlack,
            K::TermColorBrightRed,
            K::TermColorBrightGreen,
            K::TermColorBrightYellow,
            K::TermColorBrightBlue,
            K::TermColorBrightMagenta,
            K::TermColorBrightCyan,
            K::TermColorBrightWhite,
            K::TermColorDefault,
            K::TermColorRgb,
            K::TermColorRgba,
            K::UiWidget,
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
            // ── Ipe.Auth (10 kernels) — schemed + lowered ──
            K::AuthHashPassword,
            K::AuthHashPasswordCost,
            K::AuthVerifyPassword,
            K::AuthPasswordStrength,
            K::AuthSignToken,
            K::AuthVerifyToken,
            K::AuthRegister,
            K::AuthLogin,
            K::AuthSetRole,
            K::AuthSubject,
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
            // ── Ipe.Ffi.Js — the raw typed transport across the Ipê↔JS seam ──
            K::JsSend,
            K::JsSubscribe,
            K::JsRequest,
            K::JsOpenSession,
            K::JsSessionFrames,
            K::JsSendToSession,
            K::JsCloseSession,
            // ── Ipe.Process — subprocess execution (no shell) ──
            K::ProcessRun,
            K::ProcessRunWith,
            K::ProcessRunInPty,
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
            K::DbFindJoin,
            K::DbFindProjection,
            K::DbFindJoinOrdered,
            K::DbFindProjectionOrdered,
            K::DbDeleteWhere,
            K::DbUpdateWhere,
            // Two-store inner-join constructor (getter-arrow scheme, Ipê-new).
            K::StoreJoin,
            // Single-column projection over a join (getter-arrow scheme, Ipê-new).
            K::StoreSelect,
            // Literal-value projection element (Ipê-new, no legacy oracle).
            K::StoreLiteral,
            // Unary text projection operators (Ipê-new, no legacy oracle).
            K::StoreUpper,
            K::StoreLower,
            // Binary coalesce projection operator (Ipê-new, no legacy oracle).
            K::StoreCoalesce,
            // Binary arithmetic projection operators (Ipê-new, no legacy oracle).
            K::StoreAdd,
            K::StoreSub,
            K::StoreMul,
            // Typed accessor query leaves (getter-arrow schemes, Ipê-new).
            K::StoreEqCol,
            K::StoreEqBy,
            // Accessor-typed comparison leaves (Ipê-new, same burndown family).
            K::StoreNeqCol,
            K::StoreNeqBy,
            K::StoreGtCol,
            K::StoreGtBy,
            K::StoreGteCol,
            K::StoreGteBy,
            K::StoreLtCol,
            K::StoreLtBy,
            K::StoreLteCol,
            K::StoreLteBy,
            K::StoreLike,
            K::StoreIsNull,
            K::StoreNotNull,
            K::StoreInListCol,
            K::StoreInListBy,
            // Accessor-typed column-spec builders (Ipê-new).
            K::StorePrimaryKey,
            K::StoreSerial,
            K::StoreUnique,
            K::StoreDefaultNow,
            K::StoreTouchOnUpdate,
            K::StoreDefaultText,
            K::StoreDefaultInt,
            // Row-security policy builders (Ipê-new).
            K::StoreOwnerColumn,
            K::StoreImmutable,
            // ORDER BY modifiers (Ipê-new, no legacy oracle).
            K::StoreOrderByLeft,
            K::StoreOrderByRight,
            // `Db.Decode.money`, `Db.Decode.decimal`, and `Db.Decode.bytes` —
            // Ipê-new kernels (the ancestor has no DbDec money/decimal/bytes
            // routes), closing genuine holes rather than relocating legacy
            // `kernel_ty` schemes. Their DbDec siblings are RELOCATED; these
            // are deliberately not.
            K::DbDecMoney,
            K::DbDecDecimal,
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
            // ── Ipe.Crypto typed-key newtypes (5) ──────────────────────
            K::CryptoKeyFromString,
            K::CryptoKeyFromBytes,
            K::CryptoMacToHex,
            K::CryptoHmacSha256WithKey,
            K::CryptoHmacSha512WithKey,
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
            // `Web.embed : WebConfig -> WebApp` — Ipê-new (no legacy oracle);
            // a mountable web-app handle sharing `Web.app`'s cfg scheme.
            K::WebEmbed,
            // ── Ipe.App runtime-config front door (8, Ipê-new) ──────────────
            K::WebAppWith,
            K::AppFromEnv,
            K::AppFromEnvRequired,
            K::HostBind,
            K::LogLevelSetting,
            K::DbUrlSetting,
            K::ConsoleAdminToken,
            K::ConsoleIngestToken,
            K::ConsoleMetricsToken,
            K::WebCsrf,
            K::WebSessionTtl,
            K::WebAuthMaxLifetime,
            K::WebAuthSlideWindow,
            K::WebAuthRevocationMode,
            // Config-tag ADT constructors — nullary values of the
            // closed `HostMode` / `LogLevel` / `CsrfMode` / `RevocationMode` types,
            // projected to a raw `Int` tag at emit.
            K::HostLoopback,
            K::HostAllInterfaces,
            K::HostEnvDriven,
            K::LevelDebug,
            K::LevelInfo,
            K::LevelWarn,
            K::LevelError,
            K::WebCsrfStrict,
            K::WebCsrfInherit,
            K::WebRevocationOff,
            K::WebRevocationStore,
            // `Server.mountApp : String -> WebApp -> Route` — Ipê-new (no legacy
            // oracle); mounts an embedded web app into the shared server router.
            K::ServerMountApp,
            // `Server.withRevocation : RevocationMode -> AuthConfig -> AuthConfig` —
            // Ipê-new (no legacy oracle); arms the revocation gate on an auth config.
            K::ServerWithRevocation,
            // `Auth.Revocation` management kernels (4) — Ipê-new (no legacy oracle):
            // `revokeUser`/`revokeSession`/`restoreUser` are
            // `Principal -> String -> Task Error ()`;
            // `isRevoked` is `String -> Task Error Bool`.
            K::AuthRevocationRevokeUser,
            K::AuthRevocationRevokeSession,
            K::AuthRevocationRestoreUser,
            K::AuthRevocationIsRevoked,
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
            builtins.retry_f_max_attempts,
            builtins.retry_f_should_retry,
            builtins.retry_f_strategy,
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
        // app-entry cfg records (`Web.app` open-row, `Tui.app` open-row /
        // `Cli.app`), `HttpRequest` /
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
            K::StringFromInt
            | K::TimeTimeString
            | K::TimeFormatHTTP
            | K::TimeFormatISO8601
            | K::TimeFormatRFC3339 => fun(int(), string()),
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
            K::StringAppend | K::SystemGetenvOr => fun(string(), fun(string(), string())),
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
            K::TimeDaysInMonth | K::TimeAddMillis | K::TimeDiffMillis => {
                fun(int(), fun(int(), int()))
            }
            K::TimeFormat => fun(string(), fun(int(), string())),

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
        let src = concat!(
            include_str!("mod.rs"),
            include_str!("builtins.rs"),
            include_str!("builder_core.rs"),
            include_str!("constrain_ast.rs"),
            include_str!("scheme_table.rs"),
            include_str!("zonk.rs"),
        );
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
    use super::super::{Builder, Builtins, Content, Interner, Ty, UnionFind};
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
