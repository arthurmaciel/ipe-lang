//! Sky → Rust identifier naming rules (M0 subset).
//!
//! Ports the relevant arms of `Sky/Generate/Rust/Builder/Naming.hs`:
//! `toCamelCase` / `toSnakeCase` and the module-prefixing used to derive Rust
//! type and function names. M0 only needs the `Main` module's enum + functions,
//! but the conversions are written generally so they match the reference.

use sky_ir::KernelFn;

/// Convert a Sky module-prefixed name to `UpperCamelCase` (used for type names).
///
/// `Sky_Core_Error_Error` → `SkyCoreErrorError`, `Main_Msg` → `MainMsg`.
/// An underscore is dropped and the following character upper-cased; a trailing
/// underscore with no successor is kept verbatim (mirrors the Haskell pattern
/// `go ('_':c:cs)` falling through to `go (c:cs)` when there is no `c`).
#[must_use]
pub fn to_camel_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = if let Some(&c) = chars.first() {
        out.extend(c.to_uppercase());
        1usize
    } else {
        0
    };
    while let Some(&c) = chars.get(i) {
        if c == '_' {
            if let Some(&n) = chars.get(i + 1) {
                out.extend(n.to_uppercase());
                i += 2;
                continue;
            }
            out.push('_');
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Convert a Sky module-prefixed name to `snake_case` (used for function names).
///
/// `Sky_Core_List_map` → `sky_core_list_map`, `Main_update` → `main_update`.
/// Mirrors the Haskell `toSnakeCase`: the leading character is lower-cased; an
/// underscore followed by a character emits `_` plus the lower-cased successor;
/// an interior upper-case character emits `_` plus its lower-case form.
#[must_use]
pub fn to_snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = if let Some(&c) = chars.first() {
        out.extend(c.to_lowercase());
        1usize
    } else {
        0
    };
    while let Some(&c) = chars.get(i) {
        if c == '_' {
            if let Some(&n) = chars.get(i + 1) {
                out.push('_');
                out.extend(n.to_lowercase());
                i += 2;
                continue;
            }
            out.push('_');
        } else if c.is_uppercase() {
            out.push('_');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// The dotted module prefix rendered with `_` separators (`["Sky","Core","Io"]`
/// → `Sky_Core_Io`). This matches `moduleNameToRust` (dots → underscores) when
/// the path is supplied as already-split segments.
fn module_prefix(module: &[&str]) -> String {
    module.join("_")
}

/// Every Rust keyword that cannot appear as a bare identifier in emitted code:
/// the strict keywords (2015 + 2018), the reserved-for-future keywords, and the
/// 2024-reserved `gen`. Mirrors the Go backend's `reservedGoNames` audit. A name
/// in this set is mangled by [`mangle_reserved`] before it reaches the output.
///
/// `union`/`dyn`-style weak/contextual keywords are intentionally excluded: they
/// are legal in identifier position. The four keywords that additionally cannot
/// be written as raw identifiers (`crate`/`self`/`Self`/`super`) are covered
/// here too, which is why [`mangle_reserved`] uses the universally-valid
/// trailing-underscore form rather than `r#name`.
fn is_reserved_rust_name(s: &str) -> bool {
    matches!(
        s,
        // Strict keywords (Rust 2015).
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            // Strict keywords added in the 2018 edition.
            | "async"
            | "await"
            | "dyn"
            // Reserved for future use.
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            // Reserved in the 2024 edition.
            | "gen"
    )
}

/// Rewrite an emitted identifier that collides with a Rust keyword so the
/// generated code compiles. A reserved name gains a trailing underscore
/// (`type` → `type_`, `Self` → `Self_`); every other name passes through
/// unchanged, so emission for the M0 golden stays byte-identical.
///
/// The trailing-underscore form is chosen over raw identifiers (`r#name`)
/// because it is valid for *every* keyword — `r#crate` / `r#self` / `r#Self` /
/// `r#super` are themselves rejected by the Rust grammar — so one rule covers
/// the whole set without special cases.
#[must_use]
pub fn mangle_reserved(name: String) -> String {
    if is_reserved_rust_name(&name) {
        let mut mangled = name;
        mangled.push('_');
        mangled
    } else {
        name
    }
}

/// The Rust enum/type name for a user type: `enum_name(["Main"], "Msg")` →
/// `MainMsg`. Mirrors `unionToRustTypeDef`'s `codegenName`. A name colliding
/// with a Rust keyword (e.g. a bare `type Self`) is mangled by
/// [`mangle_reserved`].
#[must_use]
pub fn enum_name(module: &[&str], ty: &str) -> String {
    mangle_reserved(to_camel_case(&format!("{}_{}", module_prefix(module), ty)))
}

/// The Rust function name for a top-level value: `module_value(["Main"],
/// "update")` → `main_update`.
///
/// The program entry `main` in module `Main` is special-cased to `sky_main`
/// (the fixed `fn main` entry-point in the epilogue calls `sky_main()`), matching
/// the Haskell `rustName` rule in `ModuleEmitter.hs`.
#[must_use]
pub fn module_value(module: &[&str], name: &str) -> String {
    if name == "main" && module == ["Main"] {
        return "sky_main".to_owned();
    }
    // A record type-alias auto-constructor (#82) is the ONLY top-level value
    // whose Sky name begins with an uppercase letter (the parser forces every
    // other value / function name lowercase). Snake-casing it would fold the
    // case and could COLLIDE with a same-spelled lowercase value in the same
    // module — e.g. `type alias Row = { … }` yields the constructor `Row`, and a
    // value `row` in the same module would both mangle to `main_row` (a
    // duplicate-definition / wrong-arity miscompile). Emit the constructor's
    // name VERBATIM (case-preserved) so the ident keeps an uppercase letter and
    // is therefore provably disjoint from every snake-cased (all-lowercase)
    // value name, while remaining injective across constructors (Sky names are
    // unique per module) and across modules (the snake-cased module prefix). The
    // module-level `#![allow(non_snake_case)]` suppresses the style lint.
    if name.chars().next().is_some_and(char::is_uppercase) {
        let prefix = to_snake_case(&module_prefix(module));
        return mangle_reserved(format!("{prefix}_{name}"));
    }
    mangle_reserved(to_snake_case(&format!(
        "{}_{}",
        module_prefix(module),
        name
    )))
}

/// The base Rust struct name for a synthesised record shape, derived from its
/// sorted field names: `record_struct_name(["x", "y"])` → `RecXY`.
///
/// Mirrors the Haskell Rust backend's `anonStructName` strategy (a name built
/// from the field set) but with a `Rec_` stem. The result is a *base* name; the
/// caller deduplicates across the program and appends a numeric suffix on the
/// rare event that two distinct field sets camel-case to the same string (e.g.
/// `["a_b"]` and `["a", "b"]`), keeping every synthesised struct collision-free.
/// A base that lands on a Rust keyword is mangled by [`mangle_reserved`].
#[must_use]
pub fn record_struct_name(field_names: &[String]) -> String {
    let joined = field_names.join("_");
    mangle_reserved(to_camel_case(&format!("Rec_{joined}")))
}

/// The Rust runtime function name for a kernel built-in (M0 subset). Mirrors
/// `Kernel.kernelToRust`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub const fn kernel_name(k: KernelFn) -> &'static str {
    match k {
        KernelFn::BasicsCompare => "basics_compare",
        KernelFn::StringFromInt => "string_from_int",
        KernelFn::StringFromFloat => "string_from_float",
        // ── String arity-1 ──────────────────────────────────────────────────
        KernelFn::StringLength => "string_length",
        KernelFn::StringIsEmpty => "string_is_empty",
        KernelFn::StringReverse => "string_reverse",
        KernelFn::StringToUpper => "string_to_upper",
        KernelFn::StringToLower => "string_to_lower",
        KernelFn::StringCasefold => "string_casefold",
        KernelFn::StringTrim => "string_trim",
        KernelFn::StringTrimStart => "string_trim_start",
        KernelFn::StringTrimEnd => "string_trim_end",
        KernelFn::StringToInt => "string_to_int",
        KernelFn::StringToFloat => "string_to_float",
        KernelFn::StringFromChar => "string_from_char",
        KernelFn::StringFromList => "string_from_list",
        KernelFn::StringConcat => "string_concat",
        KernelFn::StringWords => "string_words",
        KernelFn::StringLines => "string_lines",
        KernelFn::StringToList => "string_to_list",
        KernelFn::StringIsEmail => "string_is_email",
        KernelFn::StringIsUrl => "string_is_url",
        // ── String arity-2 ──────────────────────────────────────────────────
        KernelFn::StringAppend => "string_append",
        KernelFn::StringContains => "string_contains",
        KernelFn::StringStartsWith => "string_starts_with",
        KernelFn::StringEndsWith => "string_ends_with",
        KernelFn::StringEqualFold => "string_equal_fold",
        KernelFn::StringJoin => "string_join",
        KernelFn::StringSplit => "string_split",
        KernelFn::StringRepeat => "string_repeat",
        KernelFn::StringDropLeft => "string_drop_left",
        KernelFn::StringDropRight => "string_drop_right",
        // ── String arity-3 ──────────────────────────────────────────────────
        KernelFn::StringReplace => "string_replace",
        KernelFn::StringSlice => "string_slice",
        KernelFn::StringPadLeft => "string_pad_left",
        KernelFn::StringPadRight => "string_pad_right",
        KernelFn::StringContainsIn => "string_contains_in",
        KernelFn::StringStartsWithIn => "string_starts_with_in",
        KernelFn::StringEndsWithIn => "string_ends_with_in",
        // ── Char arity-1 ────────────────────────────────────────────────────
        KernelFn::CharIsAlpha => "char_is_alpha",
        KernelFn::CharIsDigit => "char_is_digit",
        KernelFn::CharIsLower => "char_is_lower",
        KernelFn::CharIsUpper => "char_is_upper",
        KernelFn::CharToLower => "char_to_lower",
        KernelFn::CharToUpper => "char_to_upper",
        KernelFn::CharToCode => "char_to_code",
        KernelFn::CharFromCode => "char_from_code",
        KernelFn::LogPrintln => "log_println",
        KernelFn::LogInfo => "log_info",
        KernelFn::LogDebug => "log_debug",
        KernelFn::LogWarn => "log_warn",
        KernelFn::LogError => "log_error",
        KernelFn::LogInfoWith => "log_info_with",
        KernelFn::LogDebugWith => "log_debug_with",
        KernelFn::LogWarnWith => "log_warn_with",
        KernelFn::LogErrorWith => "log_error_with",
        KernelFn::ListMap => "list_map_consume",
        KernelFn::ListFilter => "list_filter",
        KernelFn::ListFoldl => "list_foldl",
        KernelFn::ListFoldr => "list_foldr",
        KernelFn::ListLength => "list_length",
        KernelFn::ListHead => "list_head",
        KernelFn::ListTail => "list_tail",
        KernelFn::ListMember => "list_member",
        KernelFn::ListRange => "list_range",
        KernelFn::ListReverse => "list_reverse",
        KernelFn::ListAppend => "list_append",
        KernelFn::ListConcat => "list_concat",
        KernelFn::ListTake => "list_take",
        KernelFn::ListDrop => "list_drop",
        KernelFn::ListZip => "list_zip",
        KernelFn::ListCons => "sky_list_cons",
        KernelFn::ListIsEmpty => "list_is_empty",
        KernelFn::ListConcatMap => "list_concat_map",
        KernelFn::ListIndexedMap => "list_indexed_map",
        KernelFn::ListAny => "list_any",
        KernelFn::ListAll => "list_all",
        KernelFn::ListFind => "list_find",
        // ── List batch (#119) ────────────────────────────────────────────────
        KernelFn::ListFilterMap => "list_filter_map",
        KernelFn::ListSortBy => "list_sort_by",
        KernelFn::BasicsNot => "basics_not",
        KernelFn::BasicsIdentity => "basics_identity",
        KernelFn::BasicsAlways => "basics_always",
        KernelFn::BasicsFst => "basics_fst",
        KernelFn::BasicsSnd => "basics_snd",
        KernelFn::BasicsModBy => "basics_mod_by",
        KernelFn::BasicsClamp => "basics_clamp",
        KernelFn::BasicsToString => "basics_to_string",
        // ── Basics numerics (#115) ──────────────────────────────────────────
        KernelFn::BasicsNegate => "basics_negate",
        KernelFn::BasicsAbs    => "basics_abs",
        // BasicsSqrt / BasicsMin / BasicsMax reuse the existing Math runtime
        // helpers: `math_sqrt(f64->f64)`, `math_min<T:PartialOrd>`,
        // `math_max<T:PartialOrd>`. No new runtime symbol needed.
        // These arms are merged with their Math.* counterparts below.
        // ── end Basics numerics (#115) ──────────────────────────────────────
        // ── Error kernels (Sky.Core.Error — minimal `Error = String` slice, #86) ─
        // The eight message constructors share ONE identity runtime symbol: with
        // `SkyError = String`, `String -> Error` is the identity. `toString`
        // reuses the existing `errorToString` runtime (`basics_error_to_string`).
        KernelFn::ErrorUnexpected
        | KernelFn::ErrorInvalidInput
        | KernelFn::ErrorIo
        | KernelFn::ErrorNetwork
        | KernelFn::ErrorFfi
        | KernelFn::ErrorDecode
        | KernelFn::ErrorConflict
        | KernelFn::ErrorUnavailable => "sky_error_from_message",
        // ── CssSafety (Sky.Core.CssSafety — Std.Css leaf kernels, #47) ──────
        // The bare runtime fn names, re-exported at the `sky_runtime` root via
        // `pub use css::*`. `safe_value`/`safe_prop_name`/`safe_selector` return
        // `SkyMaybe<String>`; `strip_style_close_kernel` returns `String`.
        KernelFn::CssSafetySafeValue => "safe_value",
        KernelFn::CssSafetySafePropName => "safe_prop_name",
        KernelFn::CssSafetySafeSelector => "safe_selector",
        KernelFn::CssSafetyStripStyleClose => "strip_style_close_kernel",
        KernelFn::ErrorTimeout => "sky_error_timeout",
        KernelFn::ErrorNotFound => "sky_error_not_found",
        KernelFn::ErrorPermissionDenied => "sky_error_permission_denied",
        KernelFn::ErrorToString => "basics_error_to_string",
        KernelFn::ErrorWithMessage => "sky_error_with_message",
        KernelFn::MaybeWithDefault => "maybe_with_default",
        KernelFn::MaybeMap => "sky_maybe_map",
        KernelFn::MaybeAndThen => "sky_maybe_and_then",
        KernelFn::MaybeMap2 => "maybe_map2",
        KernelFn::MaybeMap3 => "maybe_map3",
        KernelFn::MaybeMap4 => "maybe_map4",
        KernelFn::MaybeMap5 => "maybe_map5",
        KernelFn::MaybeAndMap => "maybe_and_map",
        KernelFn::MaybeCombine => "maybe_combine",
        KernelFn::ResultWithDefault => "result_with_default",
        KernelFn::ResultMap => "sky_result_map",
        KernelFn::ResultAndThen => "sky_result_and_then",
        KernelFn::ResultMapError => "sky_result_map_error",
        KernelFn::ResultMap2 => "result_map2",
        KernelFn::ResultMap3 => "result_map3",
        KernelFn::ResultMap4 => "result_map4",
        KernelFn::ResultMap5 => "result_map5",
        KernelFn::ResultAndMap => "result_and_map",
        KernelFn::ResultCombine => "result_combine",
        KernelFn::ResultTraverse => "result_traverse",
        // `Math.min` / `Math.max` map to the runtime's generic
        // `math_min<T: PartialOrd>` / `math_max<T: PartialOrd>`: a real
        // polymorphic compare at the argument's actual type — NO `Int`
        // coercion, NO float truncation. Divergence from Sky (PR #136):
        // Sky routes args through AsInt; Sky-Rust follows Elm's polymorphic
        // comparable (a -> a -> a). Rationale: Elm-conformance.
        // ── Basics numerics (#115): BasicsSqrt / BasicsMin / BasicsMax merged here ──
        KernelFn::BasicsSqrt | KernelFn::MathSqrt => "math_sqrt",
        KernelFn::BasicsMin  | KernelFn::MathMin  => "math_min",
        KernelFn::BasicsMax  | KernelFn::MathMax  => "math_max",
        // ── Math constants ───────────────────────────────────────────────────
        KernelFn::MathPi => "math_pi",
        KernelFn::MathE => "math_e",
        KernelFn::MathPhi => "math_phi",
        KernelFn::MathSqrt2 => "math_sqrt2",
        KernelFn::MathInf => "math_inf",
        KernelFn::MathNan => "math_nan",
        KernelFn::MathIsNaN => "math_is_nan",
        // ── Math arity-1 (Int → Int) ─────────────────────────────────────────
        KernelFn::MathAbs => "math_abs",
        // ── Math arity-1 (Float → Float) ────────────────────────────────────
        // MathSqrt merged with BasicsSqrt above (Basics numerics #115).
        KernelFn::MathCbrt => "math_cbrt",
        KernelFn::MathExp => "math_exp",
        KernelFn::MathExp2 => "math_exp2",
        KernelFn::MathLog => "math_log",
        KernelFn::MathLog2 => "math_log2",
        KernelFn::MathLog10 => "math_log10",
        KernelFn::MathSin => "math_sin",
        KernelFn::MathCos => "math_cos",
        KernelFn::MathTan => "math_tan",
        KernelFn::MathAsin => "math_asin",
        KernelFn::MathAcos => "math_acos",
        KernelFn::MathAtan => "math_atan",
        KernelFn::MathSinh => "math_sinh",
        KernelFn::MathCosh => "math_cosh",
        KernelFn::MathTanh => "math_tanh",
        KernelFn::MathAsinh => "math_asinh",
        KernelFn::MathAcosh => "math_acosh",
        KernelFn::MathAtanh => "math_atanh",
        // ── Math arity-1 (Float → Int) ───────────────────────────────────────
        KernelFn::MathFloor => "math_floor",
        KernelFn::MathCeil => "math_ceil",
        KernelFn::MathRound => "math_round",
        KernelFn::MathTrunc => "math_trunc",
        // ── Math arity-2 (Float → Float → Float) ────────────────────────────
        KernelFn::MathPow => "math_pow",
        KernelFn::MathHypot => "math_hypot",
        KernelFn::MathAtan2 => "math_atan2",
        KernelFn::MathMod => "math_mod",
        KernelFn::MathRemainder => "math_remainder",
        KernelFn::ResultOkDefault => "ok_res",
        // ── Dict kernels ────────────────────────────────────────────────────
        KernelFn::DictEmpty => "dict_empty",
        KernelFn::DictIsEmpty => "dict_is_empty",
        KernelFn::DictSize => "dict_size",
        KernelFn::DictKeys => "dict_keys",
        KernelFn::DictValues => "dict_values",
        KernelFn::DictToList => "dict_to_list",
        KernelFn::DictFromList => "dict_from_list",
        KernelFn::DictGet => "dict_get",
        KernelFn::DictMember => "dict_member",
        KernelFn::DictRemove => "dict_remove",
        KernelFn::DictUnion => "dict_union",
        KernelFn::DictMap => "dict_map",
        KernelFn::DictInsert => "dict_insert",
        KernelFn::DictFoldl => "dict_foldl",
        // ── Set kernels ─────────────────────────────────────────────────────
        KernelFn::SetEmpty => "set_empty",
        KernelFn::SetSize => "set_size",
        KernelFn::SetToList => "set_to_list",
        KernelFn::SetFromList => "set_from_list",
        KernelFn::SetMember => "set_member",
        KernelFn::SetInsert => "set_insert",
        KernelFn::SetRemove => "set_remove",
        KernelFn::SetUnion => "set_union",
        KernelFn::SetIntersect => "set_intersect",
        KernelFn::SetDiff => "set_diff",
        // ── Bytes kernels (M4e) ─────────────────────────────────────────────
        KernelFn::BytesEmpty => "bytes_empty",
        KernelFn::BytesLength => "bytes_length",
        KernelFn::BytesIsEmpty => "bytes_is_empty",
        KernelFn::BytesFromString => "bytes_from_string",
        KernelFn::BytesToString => "bytes_to_string",
        KernelFn::BytesFromHex => "bytes_from_hex",
        KernelFn::BytesToHex => "bytes_to_hex",
        KernelFn::BytesFromBase64 => "bytes_from_base64",
        KernelFn::BytesToBase64 => "bytes_to_base64",
        KernelFn::BytesAppend => "bytes_append",
        KernelFn::BytesSlice => "bytes_slice",
        // ── Encoding kernels (M4f) ──────────────────────────────────────────
        // Encoders are total (infallible) — no type-inference issue.
        KernelFn::EncodingBase64Encode => "base64_encode",
        KernelFn::EncodingUrlEncode => "url_encode",
        KernelFn::EncodingHexEncode => "encoding_hex_encode",
        // Decoders return `Result Error String`. The upstream runtime uses a
        // generic `E: From<String>` bound for flexibility, but generated Sky code
        // always sets `SkyError = String`. Rust cannot infer `E` when the error
        // arm is `Err _ ->` (discarded), so we route to concrete aliases that pin
        // `E = String`, eliminating the ambiguity without changing semantics.
        KernelFn::EncodingBase64Decode => "sky_base64_decode",
        KernelFn::EncodingUrlDecode => "sky_url_decode",
        KernelFn::EncodingHexDecode => "sky_encoding_hex_decode",
        // ── JsonEnc kernels (M4g) ────────────────────────────────────────────
        KernelFn::JsonEncString => "json_enc_string",
        KernelFn::JsonEncInt => "json_enc_int",
        KernelFn::JsonEncFloat => "json_enc_float",
        KernelFn::JsonEncBool => "json_enc_bool",
        KernelFn::JsonEncNull => "json_enc_null",
        KernelFn::JsonEncList => "json_enc_list",
        KernelFn::JsonEncObject => "json_enc_object",
        KernelFn::JsonEncEncode => "json_enc_encode",
        // ── JsonDec kernels (M4h) ────────────────────────────────────────────
        KernelFn::JsonDecString => "json_decode_string",
        KernelFn::JsonDecInt => "json_decode_int",
        KernelFn::JsonDecFloat => "json_decode_float",
        KernelFn::JsonDecBool => "json_decode_bool",
        KernelFn::JsonDecDecodeString => "decode_from_json_string",
        KernelFn::JsonDecField => "decode_field",
        KernelFn::JsonDecAt => "decode_at",
        KernelFn::JsonDecIndex => "decode_index",
        KernelFn::JsonDecList => "decode_list",
        KernelFn::JsonDecMap | KernelFn::DbDecMap => "decode_map",
        KernelFn::JsonDecAndThen | KernelFn::DbDecAndThen => "decode_and_then",
        KernelFn::JsonDecSucceed | KernelFn::DbDecSucceed => "decode_succeed",
        KernelFn::JsonDecFail | KernelFn::DbDecFail => "decode_fail",
        KernelFn::JsonDecOneOf => "decode_one_of",
        KernelFn::JsonDecMap2 | KernelFn::DbDecMap2 => "decode_map2",
        KernelFn::JsonDecMap3 | KernelFn::DbDecMap3 => "decode_map3",
        KernelFn::JsonDecMap4 | KernelFn::DbDecMap4 => "decode_map4",
        // ── JsonDecP pipeline kernels (M4h) ──────────────────────────────────
        KernelFn::JsonDecPRequired => "decode_pipeline_required",
        KernelFn::JsonDecPOptional => "decode_pipeline_optional",
        KernelFn::JsonDecPCustom => "decode_pipeline_custom",
        KernelFn::JsonDecPRequiredAt => "decode_pipeline_required_at",
        // ── Crypto kernels (M5a) ─────────────────────────────────────────────
        KernelFn::CryptoSha256 => "crypto_sha256",
        KernelFn::CryptoSha512 => "crypto_sha512",
        KernelFn::CryptoSha1 => "crypto_sha1",
        KernelFn::CryptoMd5 => "crypto_md5",
        KernelFn::CryptoHmacSha256 => "crypto_hmac_sha256",
        KernelFn::CryptoHmacSha512 => "crypto_hmac_sha512",
        // Concrete alias pins E=String — avoids type-inference ambiguity when
        // the Err arm is discarded in generated Sky code.
        KernelFn::CryptoRsaSha256Sign => "sky_crypto_rsa_sha256_sign",
        KernelFn::CryptoRsaSha256Verify => "crypto_rsa_sha256_verify",
        KernelFn::CryptoConstantTimeEqual => "crypto_constant_time_equal",
        // Concrete aliases pin E=String for all AEAD Result-returning functions.
        KernelFn::CryptoAesGcmEncrypt => "sky_aes_gcm_encrypt",
        KernelFn::CryptoAesGcmDecrypt => "sky_aes_gcm_decrypt",
        KernelFn::CryptoChacha20Encrypt => "sky_chacha20_encrypt",
        KernelFn::CryptoChacha20Decrypt => "sky_chacha20_decrypt",
        KernelFn::CryptoAesKeyFromPassword => "crypto_aes_key_from_password",
        KernelFn::CryptoChachaKeyFromPassword => "crypto_chacha_key_from_password",
        KernelFn::CryptoRandomBytes => "crypto_random_bytes",
        KernelFn::CryptoRandomToken => "crypto_random_token",
        // ── Uuid kernels (M5b) ──────────────────────────────────────────────
        // uuid_v4 / uuid_v7 are `() -> Task Error String` (task #54): entropy is
        // an effect, so they return `SkyTask<E, String>` (E inferred from the
        // enclosing Task chain, like `crypto_random_token`).
        KernelFn::UuidV4 => "uuid_v4",
        KernelFn::UuidV7 => "uuid_v7",
        // uuid_parse returns SkyMaybe<String> — no E type, no concretisation.
        KernelFn::UuidParse => "uuid_parse",
        // ── Jwt kernels (M5b) ───────────────────────────────────────────────
        // Concrete aliases pin E=String so Rust can infer the error type at
        // call sites where the Err arm is discarded (matches the Crypto pattern).
        KernelFn::JwtEncodeHs256 => "sky_jwt_encode_hs256",
        KernelFn::JwtDecodeHs256 => "sky_jwt_decode_hs256",
        KernelFn::JwtEncodeRs256 => "sky_jwt_encode_rs256",
        KernelFn::JwtDecodeRs256 => "sky_jwt_decode_rs256",
        // ── Task combinators (M5a) ────────────────────────────────────────────
        KernelFn::TaskSucceed => "task_succeed",
        KernelFn::TaskFail => "task_fail",
        KernelFn::TaskMap => "task_map",
        KernelFn::TaskAndThen => "task_and_then",
        KernelFn::TaskMapError => "task_map_error",
        KernelFn::TaskOnError => "task_on_error",
        KernelFn::TaskFromResult => "task_from_result",
        KernelFn::TaskAndThenResult => "task_and_then_result",
        KernelFn::TaskSequence => "task_sequence",
        KernelFn::TaskParallel => "task_parallel",
        // `Task.perform` is a 1-arg legacy alias for `Task.run`; both lower to the
        // same runtime function.
        KernelFn::TaskRun | KernelFn::TaskPerform => "task_run",
        KernelFn::TaskLazy => "task_lazy",
        // ── Task retry surface — special-case emitter intercepts; names here
        // are only reached when the special-case path falls through (never in
        // practice, but needed for exhaustiveness).
        KernelFn::TaskRetryWith => "task_retry_with",
        KernelFn::TaskLinearBackoff => "task_linear_backoff",
        KernelFn::TaskExponentialBackoff => "task_exponential_backoff",
        KernelFn::TaskWithJitter => "task_with_jitter",
        KernelFn::TaskRetryOn => "task_retry_on",
        KernelFn::TaskWithRetryOn => "task_with_retry_on",
        KernelFn::TaskDefaultRetryPolicy => "task_default_retry_policy",
        KernelFn::TaskWithMaxAttempts => "task_with_max_attempts",
        KernelFn::TaskWithBaseMs => "task_with_base_ms",
        KernelFn::TaskWithKind => "task_with_kind",
        // ── Io kernels (M5a) ──────────────────────────────────────────────────
        KernelFn::IoReadLine => "io_read_line",
        KernelFn::IoWriteStdout => "io_write_stdout",
        KernelFn::IoWriteStderr => "io_write_stderr",
        // ── Time kernels (M5a) ────────────────────────────────────────────────
        KernelFn::TimeNow => "time_now",
        KernelFn::TimeSleep => "time_sleep",
        KernelFn::TimeUnixMillis => "time_unix_millis",
        KernelFn::TimeTimeString => "time_time_string",
        // ── System kernels (M5a) ──────────────────────────────────────────────
        KernelFn::SystemArgs => "system_args",
        KernelFn::SystemGetenv => "system_getenv",
        KernelFn::SystemGetenvOr => "system_getenv_or",
        KernelFn::SystemGetArg => "system_get_arg",
        KernelFn::SystemGetenvInt => "system_getenv_int",
        KernelFn::SystemGetenvBool => "system_getenv_bool",
        KernelFn::SystemSetenv => "system_setenv",
        KernelFn::SystemUnsetenv => "system_unsetenv",
        KernelFn::SystemCwd => "system_cwd",
        KernelFn::SystemLoadEnv => "system_load_env",
        KernelFn::SystemExit => "system_exit",
        // ── Random kernels (M5a) ──────────────────────────────────────────────
        KernelFn::RandomInt => "random_int",
        KernelFn::RandomFloat => "random_float",
        KernelFn::RandomChoice => "random_choice",
        // ── File kernels (M5a) ────────────────────────────────────────────────
        KernelFn::FileReadFile => "file_read_file",
        KernelFn::FileWriteFile => "file_write_file",
        KernelFn::FileExists => "file_exists",
        KernelFn::FileRemove => "file_remove",
        KernelFn::FileMkdirAll => "file_mkdir_all",
        KernelFn::FileReadFileLimit => "file_read_file_limit",
        KernelFn::FileReadFileBytes => "file_read_file_bytes",
        KernelFn::FileAppend => "file_append",
        KernelFn::FileReadDir => "file_read_dir",
        KernelFn::FileIsDir => "file_is_dir",
        KernelFn::FileTempFile => "file_temp_file",
        KernelFn::FileTempDir => "file_temp_dir",
        KernelFn::FileCopy => "file_copy",
        KernelFn::FileRename => "file_rename",
        KernelFn::FileDelete => "file_delete",
        // ── Http kernels (M5b) ──────────────────────────────────────────────
        // `HttpParseQuery` maps to `http_parse_query` (pure, no E type).
        // `HttpGet` / `HttpPost` / `HttpRequest` emit through `emit_http_call`
        // (not the standard callee_name path) because they need a `task_map`
        // conversion closure; these names are still registered here so
        // `kernel_name` is total over all KernelFn variants (no _ catch-all).
        // The five builder kernels (`HttpDefaultRequest` / `HttpWith*`) likewise
        // emit through `emit_http_builder_call` rather than the standard
        // `callee_name` path; their entries here keep the function total.
        KernelFn::HttpGet => "http_get",
        KernelFn::HttpPost => "http_post",
        KernelFn::HttpRequest => "http_request",
        KernelFn::HttpParseQuery => "http_parse_query",
        KernelFn::HttpDefaultRequest => "http_default_request",
        KernelFn::HttpWithMethod => "http_with_method",
        KernelFn::HttpWithTimeout => "http_with_timeout",
        KernelFn::HttpWithBody => "http_with_body",
        KernelFn::HttpWithHeader => "http_with_header",
        // ── Db kernels (M5b-db) ─────────────────────────────────────────────
        // `DbExec`/`DbQuery`/`DbQueryDecode` → `db_exec_params` / `db_query_params` /
        // `db_query_decode_params`.  The Sky surface type is polymorphic
        // (`exec : Db -> String -> List a -> Task Error Int`) so `a` may be
        // `String`, `Int`, `Float`, `Bool`, or `SqlValue`.  The emitter projects
        // the params list via `sky_runtime::db::SqlParam::from` which is
        // implemented for all of those types (runtime primitives + generated
        // `StdDbSqlValue`).  The untyped `db_exec(Vec<String>)` variant in the
        // runtime is retained for direct Rust test use but is no longer emitted
        // for Sky source.
        KernelFn::DbConnect => "db_connect",
        KernelFn::DbOpen => "db_open",
        KernelFn::DbClose => "db_close",
        KernelFn::DbExecRaw => "db_exec_raw",
        KernelFn::DbExec => "db_exec_params",
        KernelFn::DbQuery => "db_query_params",
        KernelFn::DbQueryDecode => "db_query_decode_params",
        KernelFn::DbGetString => "db_get_string",
        KernelFn::DbGetInt => "db_get_int",
        KernelFn::DbGetBool => "db_get_bool",
        KernelFn::DbGetField => "db_get_field",
        KernelFn::DbInsertRow => "db_insert_row",
        KernelFn::DbGetById => "db_get_by_id",
        KernelFn::DbUpdateById => "db_update_by_id",
        KernelFn::DbDeleteById => "db_delete_by_id",
        KernelFn::DbFindOneByField => "db_find_one_by_field",
        KernelFn::DbFindManyByField => "db_find_many_by_field",
        KernelFn::DbFindByConditions => "db_find_by_conditions",
        KernelFn::DbUnsafeFindWhere => "db_unsafe_find_where",
        KernelFn::DbInsertFields => "db_insert_fields",
        KernelFn::DbUpdateFields => "db_update_fields",
        KernelFn::DbInsertFieldsReturning => "db_insert_fields_returning",
        KernelFn::DbWithTransaction => "db_with_transaction",
        KernelFn::DbMigrate => "db_migrate_apply",
        // ── Db.Decode kernels (M5b-db) ───────────────────────────────────────
        KernelFn::DbDecString => "db_decode_string",
        KernelFn::DbDecInt => "db_decode_int",
        KernelFn::DbDecFloat => "db_decode_float",
        KernelFn::DbDecBool => "db_decode_bool",
        KernelFn::DbDecNullable => "db_decode_nullable",
        // DbDecMap / DbDecAndThen / DbDecFail / DbDecMap2/3/4 share the same
        // runtime function as their JsonDec* counterparts; the arms are merged
        // into the JsonDec* section above to satisfy clippy::match_same_arms.
        KernelFn::DbDecRequired => "db_decode_required",
        KernelFn::DbDecOptional => "db_decode_optional",
        // ── M5c: TEA Cmd / Sub / Time kernels (wired) ────────────────────────
        KernelFn::CmdNone => "cmd_none",
        KernelFn::CmdBatch => "cmd_batch",
        KernelFn::CmdPerform => "cmd_perform",
        KernelFn::SubNone => "sub_none",
        KernelFn::SubBatch => "sub_batch",
        KernelFn::SubEvery => "sub_every",
        KernelFn::TimeEvery => "time_every",
        // ── M6 reserved TEA kernels (NOT emittable; emit path returns CompilerBug) ──
        // kernel_name is still required for any exhaustive match on KernelFn.
        KernelFn::CmdPublish => "cmd_publish",
        KernelFn::CmdPublishNoEcho => "cmd_publish_no_echo",
        KernelFn::SubSubscribeTopic => "sub_subscribe_topic",
        KernelFn::PubSubPublish => "pubsub_publish",
        KernelFn::PubSubPublishNoEcho => "pubsub_publish_no_echo",
        // ── M6: Sky.Http.Server kernels (wired) ─────────────────────────────────
        KernelFn::ServerGet => "server_get",
        KernelFn::ServerPost => "server_post",
        KernelFn::ServerPut => "server_put",
        KernelFn::ServerDelete => "server_delete",
        KernelFn::ServerAny => "server_any",
        KernelFn::ServerApi => "server_api",
        KernelFn::ServerStatic => "server_static",
        KernelFn::ServerListen => "server_listen",
        KernelFn::ServerText => "server_text",
        KernelFn::ServerJson => "server_json",
        KernelFn::ServerHtml => "server_html",
        KernelFn::ServerWithStatus => "server_with_status",
        KernelFn::ServerWithHeader => "server_with_header",
        KernelFn::ServerRedirect => "server_redirect",
        KernelFn::ServerParam => "server_param",
        KernelFn::ServerQueryParam => "server_query_param",
        KernelFn::ServerHeader => "server_header",
        KernelFn::ServerGetCookie => "server_get_cookie",
        KernelFn::ServerBody => "server_body",
        KernelFn::ServerPath => "server_path",
        KernelFn::ServerMethod => "server_method",
        KernelFn::ServerCookieNew => "server_cookie",
        KernelFn::ServerWithCookie => "server_with_cookie",
        KernelFn::MiddlewareWithCors => "middleware_with_cors",
        KernelFn::MiddlewareWithLogging => "middleware_with_logging",
        KernelFn::MiddlewareWithBasicAuth => "middleware_with_basic_auth",
        KernelFn::MiddlewareWithRateLimit => "middleware_with_rate_limit",
        KernelFn::RateLimitAllow => "rate_limit_allow",
        // ── M7: Std.Ui / Std.Html render kernels (Phase 0 — fully wired) ────
        KernelFn::UiLayout => "ui_layout",
        KernelFn::UiLayoutWith => "ui_layout_with",
        KernelFn::HtmlRender => "html_render_",
        KernelFn::HtmlEscapeText => "html_escape_text_",
        KernelFn::HtmlEscapeAttr => "html_escape_attr_",
        KernelFn::HtmlAttrToString => "html_attr_to_string_",
        // ── M7: Std.Live app-entry kernels (Phase 0 — stubs, emit CompilerBug) ──
        KernelFn::LiveApp => "live_app",
        KernelFn::LiveAppRouted => "live_app_routed",
        KernelFn::LiveRoute => "live_route",
        KernelFn::LiveRenderStatic => "live_render_static",
        // ── M7: Std.Tui app-entry kernels (Phase 0 — stubs) ─────────────────
        KernelFn::TuiProgram => "tui_app",
        KernelFn::TuiApp => "tui_app_ui",
        // ── M7: Std.Webview app-entry kernel (Phase 0 — stub) ───────────────
        KernelFn::WebviewApp => "webview_app",
        // ── M7: Std.Ui element builders ──────────────────────────────────────
        KernelFn::UiNone => "ui_none_",
        KernelFn::UiText => "ui_text_",
        KernelFn::UiHtml => "ui_html_",
        KernelFn::UiEl => "ui_el_",
        KernelFn::UiRow => "ui_row_",
        KernelFn::UiColumn => "ui_column_",
        KernelFn::UiWrappedRow => "ui_wrapped_row_",
        KernelFn::UiGrid => "ui_grid_",
        KernelFn::UiParagraph => "ui_paragraph_",
        KernelFn::UiTextColumn => "ui_text_column_",
        KernelFn::UiButton => "ui_button_",
        // ── M7: Std.Ui attribute builders ────────────────────────────────────
        KernelFn::UiSpacing => "ui_spacing_",
        KernelFn::UiPadding => "ui_padding_",
        KernelFn::UiPaddingXY => "ui_padding_xy_",
        KernelFn::UiWidth => "ui_width_",
        KernelFn::UiHeight => "ui_height_",
        KernelFn::UiCenterX => "ui_center_x_",
        KernelFn::UiCenterY => "ui_center_y_",
        KernelFn::UiAlignLeft => "ui_align_left_",
        KernelFn::UiAlignRight => "ui_align_right_",
        KernelFn::UiAlignTop => "ui_align_top_",
        KernelFn::UiAlignBottom => "ui_align_bottom_",
        KernelFn::UiPointer => "ui_pointer_",
        KernelFn::UiClip => "ui_clip_",
        KernelFn::UiScrollbars => "ui_scrollbars_",
        KernelFn::UiGridColumns => "ui_grid_columns_",
        // ── M7: Std.Ui Length builders ───────────────────────────────────────
        KernelFn::UiPx => "ui_px_",
        KernelFn::UiFill => "ui_fill_",
        KernelFn::UiContent => "ui_content_",
        KernelFn::UiShrink => "ui_shrink_",
        KernelFn::UiFillPortion => "ui_fill_portion_",
        KernelFn::UiVh => "ui_vh_",
        KernelFn::UiVw => "ui_vw_",
        KernelFn::UiMinimum => "ui_minimum_",
        KernelFn::UiMaximum => "ui_maximum_",
        // ── M7: Std.Ui Color builders ────────────────────────────────────────
        KernelFn::UiRgb => "ui_rgb_",
        KernelFn::UiRgba => "ui_rgba_",
        KernelFn::UiWhite => "ui_white_",
        KernelFn::UiBlack => "ui_black_",
        KernelFn::UiTransparent => "ui_transparent_",
        KernelFn::UiColorCss => "ui_color_css_",
        // ── M7: Background / Border / Font sub-modules ───────────────────────
        KernelFn::BackgroundColor => "ui_background_color_",
        KernelFn::BackgroundImage => "ui_background_image_",
        KernelFn::BorderWidth => "ui_border_width_",
        KernelFn::BorderRounded => "ui_border_rounded_",
        KernelFn::BorderColor => "ui_border_color_",
        KernelFn::FontSize => "ui_font_size_",
        KernelFn::FontColor => "ui_font_color_",
        KernelFn::FontFamily => "ui_font_family_",
        KernelFn::FontBold => "ui_font_bold_",
        KernelFn::FontItalic => "ui_font_italic_",
        // ── #76 Tier 1: extended Std.Ui / Font / Background / Border builders ──
        KernelFn::UiSquare => "ui_square_",
        KernelFn::UiWidescreen => "ui_widescreen_",
        KernelFn::UiCinemascope => "ui_cinemascope_",
        KernelFn::UiAspectRatio => "ui_aspect_ratio_",
        KernelFn::UiAspectRatioWH => "ui_aspect_ratio_wh_",
        KernelFn::UiHtmlAttribute => "ui_html_attribute_",
        KernelFn::UiName => "ui_name_",
        KernelFn::UiStyle => "ui_style_",
        KernelFn::BackgroundHoverColor => "ui_bg_hover_color_",
        KernelFn::BackgroundFocusColor => "ui_bg_focus_color_",
        KernelFn::BackgroundActiveColor => "ui_bg_active_color_",
        KernelFn::BackgroundDisabledColor => "ui_bg_disabled_color_",
        KernelFn::BorderSolid => "ui_border_solid_",
        KernelFn::BorderDashed => "ui_border_dashed_",
        KernelFn::BorderDotted => "ui_border_dotted_",
        KernelFn::BorderHoverColor => "ui_border_hover_color_",
        KernelFn::BorderFocusColor => "ui_border_focus_color_",
        KernelFn::BorderActiveColor => "ui_border_active_color_",
        KernelFn::BorderHoverWidth => "ui_border_hover_width_",
        KernelFn::BorderHoverRounded => "ui_border_hover_rounded_",
        KernelFn::FontWeight => "ui_font_weight_",
        KernelFn::FontSemiBold => "ui_font_semi_bold_",
        KernelFn::FontRegular => "ui_font_regular_",
        KernelFn::FontLight => "ui_font_light_",
        KernelFn::FontExtraBold => "ui_font_extra_bold_",
        KernelFn::FontBlack => "ui_font_black_",
        KernelFn::FontUnderline => "ui_font_underline_",
        KernelFn::FontNoDecoration => "ui_font_no_decoration_",
        KernelFn::FontLineThrough => "ui_font_line_through_",
        KernelFn::FontLetterSpacing => "ui_font_letter_spacing_",
        KernelFn::FontWordSpacing => "ui_font_word_spacing_",
        KernelFn::FontAlignLeft => "ui_font_align_left_",
        KernelFn::FontAlignRight => "ui_font_align_right_",
        KernelFn::FontAlignCenter => "ui_font_align_center_",
        KernelFn::FontCenter => "ui_font_center_",
        KernelFn::FontJustify => "ui_font_justify_",
        KernelFn::FontSansSerif => "ui_font_sans_serif_",
        KernelFn::FontSerif => "ui_font_serif_",
        KernelFn::FontMonospace => "ui_font_monospace_",
        KernelFn::FontHoverColor => "ui_font_hover_color_",
        KernelFn::FontFocusColor => "ui_font_focus_color_",
        KernelFn::FontActiveColor => "ui_font_active_color_",
        KernelFn::FontDisabledColor => "ui_font_disabled_color_",
        KernelFn::FontHoverSize => "ui_font_hover_size_",
        KernelFn::HtmlAttrTabindex => "html_attr_tabindex_",
        // ── Std.Ui.Region (#117) ──────────────────────────────────────────────
        KernelFn::RegionMainContent => "ui_region_main_content_",
        KernelFn::RegionNavigation => "ui_region_navigation_",
        KernelFn::RegionFooter => "ui_region_footer_",
        KernelFn::RegionAside => "ui_region_aside_",
        KernelFn::RegionHeading => "ui_region_heading_",
        KernelFn::RegionLabel => "ui_region_label_",
        KernelFn::RegionAnnounce => "ui_region_announce_",
        KernelFn::RegionAnnounceUrgently => "ui_region_announce_urgently_",
        // ── Ui.input + Ui.describe + desc* constructors ───────────────────────
        KernelFn::UiInput => "ui_input_",
        KernelFn::UiDescribe => "ui_describe_",
        KernelFn::UiDescMain => "ui_desc_main_",
        KernelFn::UiDescNavigation => "ui_desc_navigation_",
        KernelFn::UiDescContentInfo => "ui_desc_content_info_",
        KernelFn::UiDescComplementary => "ui_desc_complementary_",
        KernelFn::UiDescLivePolite => "ui_desc_live_polite_",
        KernelFn::UiDescLiveAssertive => "ui_desc_live_assertive_",
        KernelFn::UiDescHeading => "ui_desc_heading_",
        KernelFn::UiDescLabel => "ui_desc_label_",
        // ── Std.Ui.Input (#124) ───────────────────────────────────────────────
        KernelFn::InputLabelAbove => "input_label_above_",
        KernelFn::InputLabelBelow => "input_label_below_",
        KernelFn::InputLabelLeft => "input_label_left_",
        KernelFn::InputLabelRight => "input_label_right_",
        KernelFn::InputLabelHidden => "input_label_hidden_",
        KernelFn::InputPlaceholder => "input_placeholder_",
        KernelFn::InputText => "input_text_",
        KernelFn::InputMultiline => "input_multiline_",
        KernelFn::InputEmail => "input_email_",
        KernelFn::InputUsername => "input_username_",
        KernelFn::InputSearch => "input_search_",
        KernelFn::InputCurrentPassword => "input_current_password_",
        KernelFn::InputNewPassword => "input_new_password_",
        KernelFn::InputCheckbox => "input_checkbox_",
        // ── M7: Html element builders ────────────────────────────────────────
        KernelFn::HtmlTextNode => "html_text_node_",
        KernelFn::HtmlRawNode => "html_raw_node_",
        KernelFn::HtmlStyleNode => "html_style_node_",
        KernelFn::HtmlDiv => "html_div_",
        KernelFn::HtmlSpan => "html_span_",
        KernelFn::HtmlA => "html_a_",
        KernelFn::HtmlButton => "html_button_",
        KernelFn::HtmlP => "html_p_",
        KernelFn::HtmlInput => "html_input_",
        KernelFn::HtmlImg => "html_img_",
        // #76 batch 2: Std.Html element builders — tag-as-data, all share the
        // generic `html_node_` sink (the wire tag is injected by `emit_ui_call`).
        // `Html.node` itself shares this bare helper name (same sink).
        KernelFn::HtmlNode
        | KernelFn::HtmlH1
        | KernelFn::HtmlH2
        | KernelFn::HtmlH3
        | KernelFn::HtmlH4
        | KernelFn::HtmlH5
        | KernelFn::HtmlH6
        | KernelFn::HtmlNav
        | KernelFn::HtmlSection
        | KernelFn::HtmlArticle
        | KernelFn::HtmlHeader
        | KernelFn::HtmlFooter
        | KernelFn::HtmlMain
        | KernelFn::HtmlAside
        | KernelFn::HtmlUl
        | KernelFn::HtmlOl
        | KernelFn::HtmlLi
        | KernelFn::HtmlTable
        | KernelFn::HtmlThead
        | KernelFn::HtmlTbody
        | KernelFn::HtmlTfoot
        | KernelFn::HtmlTr
        | KernelFn::HtmlTh
        | KernelFn::HtmlTd
        | KernelFn::HtmlTextarea
        | KernelFn::HtmlSelect
        | KernelFn::HtmlOption
        | KernelFn::HtmlLabel
        | KernelFn::HtmlForm
        | KernelFn::HtmlFieldset
        | KernelFn::HtmlLegend
        | KernelFn::HtmlPre
        | KernelFn::HtmlCode
        | KernelFn::HtmlStrong
        | KernelFn::HtmlEm
        | KernelFn::HtmlSmall
        | KernelFn::HtmlBlockquote
        | KernelFn::HtmlFigure
        | KernelFn::HtmlFigcaption
        | KernelFn::HtmlDetails
        | KernelFn::HtmlSummary
        | KernelFn::HtmlDialog
        | KernelFn::HtmlVideo
        | KernelFn::HtmlAudio
        | KernelFn::HtmlCanvas
        | KernelFn::HtmlIframe
        | KernelFn::HtmlProgress
        | KernelFn::HtmlMeter
        | KernelFn::HtmlScript
        | KernelFn::HtmlBody
        | KernelFn::HtmlTitle
        | KernelFn::HtmlBr
        | KernelFn::HtmlHr
        | KernelFn::HtmlMeta
        | KernelFn::HtmlLink
        | KernelFn::HtmlArea
        | KernelFn::HtmlBase
        | KernelFn::HtmlCol
        | KernelFn::HtmlEmbed
        | KernelFn::HtmlSource
        | KernelFn::HtmlTrack
        | KernelFn::HtmlWbr => "html_node_",
        // #76: Std.Html.Attributes builders. The full call (including the fixed
        // key literal) is produced by `emit_ui_call`; these names are the bare
        // runtime helpers, kept for the exhaustive match / any generic path.
        KernelFn::HtmlAttrClass
        | KernelFn::HtmlAttrId
        | KernelFn::HtmlAttrHref
        | KernelFn::HtmlAttrSrc
        | KernelFn::HtmlAttrAlt
        | KernelFn::HtmlAttrValue
        | KernelFn::HtmlAttrName
        | KernelFn::HtmlAttrPlaceholder
        | KernelFn::HtmlAttrType
        | KernelFn::HtmlAttrFor
        | KernelFn::HtmlAttrStyle
        | KernelFn::HtmlAttrTitle
        | KernelFn::HtmlAttrAutocomplete
        | KernelFn::HtmlAttribute => "html_named_attr_",
        KernelFn::HtmlAttrChecked
        | KernelFn::HtmlAttrDisabled
        | KernelFn::HtmlAttrReadonly
        | KernelFn::HtmlAttrRequired
        | KernelFn::HtmlAttrMultiple
        | KernelFn::HtmlAttrSelected
        | KernelFn::HtmlAttrAutofocus
        | KernelFn::HtmlBoolAttribute => "html_bool_named_attr_",
        KernelFn::HtmlNoAttr => "html_no_attr_",
        // Phase-1a event-attribute builders
        KernelFn::UiOnClick => "ui_on_click_",
        KernelFn::UiOnFocus => "ui_on_focus_",
        KernelFn::UiOnBlur => "ui_on_blur_",
        KernelFn::UiOnMouseOver => "ui_on_mouse_over_",
        KernelFn::UiOnMouseOut => "ui_on_mouse_out_",
        KernelFn::UiOnInput => "ui_on_input_",
        KernelFn::UiOnChange => "ui_on_change_",
        KernelFn::UiOnKeyDown => "ui_on_key_down_",
        KernelFn::UiOnKeyUp => "ui_on_key_up_",
        KernelFn::UiOnBool => "ui_on_bool_",
        // #107: Std.Html.Events builders — emitted via the dedicated
        // `emit_ui_call` arm (`html_event_shape().is_some()`), so this generic
        // name map is not consulted at emit time; the arms are here to keep the
        // match exhaustive and the name faithful to each runtime constructor.
        KernelFn::HtmlOnClick
        | KernelFn::HtmlOnFocus
        | KernelFn::HtmlOnBlur
        | KernelFn::HtmlOnMouseOver
        | KernelFn::HtmlOnMouseOut => "html_on_msg_",
        KernelFn::HtmlOnInput
        | KernelFn::HtmlOnChange
        | KernelFn::HtmlOnKeyDown
        | KernelFn::HtmlOnKeyUp => "html_on_string_",
        KernelFn::HtmlOnBool => "html_on_bool_",
        KernelFn::HtmlOnSubmit => "html_on_raw_",
        // ── #111: Cli app-entry ───────────────────────────────────────────────
        // CliProgram is emitted via the dedicated emit_cli_call path;
        // kernel_name is kept for match exhaustiveness.
        KernelFn::CliProgram => "sky_cli_program_",
        // ── #111: Std.Auth runtime function names (auth.rs) ──────────────────
        KernelFn::AuthHashPassword => "auth_hash_password",
        KernelFn::AuthHashPasswordCost => "auth_hash_password_cost",
        KernelFn::AuthVerifyPassword => "auth_verify_password",
        KernelFn::AuthPasswordStrength => "auth_password_strength",
        KernelFn::AuthSignToken => "auth_sign_token",
        KernelFn::AuthVerifyToken => "auth_verify_token",
        KernelFn::AuthRegister => "auth_register",
        KernelFn::AuthLogin => "auth_login",
        KernelFn::AuthSetRole => "auth_set_role",
        // ── #111: Sky.Http.Server.Stream runtime function names (server_stream.rs)
        KernelFn::StreamStream => "server_stream_stream",
        KernelFn::StreamEmit => "server_stream_emit",
        KernelFn::StreamFinish => "server_stream_finish",
        KernelFn::StreamWithContentType => "server_stream_with_content_type",
        // ── #111: Sky.Core.Http.Stream runtime function names (http_stream.rs) ─
        KernelFn::HttpStreamOpen => "http_stream_open",
        KernelFn::HttpStreamForEachChunk => "http_stream_for_each_chunk",
        KernelFn::HttpStreamClose => "http_stream_close",
        KernelFn::HttpStreamChunks => "sub_subscribe_stream",
        // ── #127: Sky.Http.Server.WebSocket runtime function names (server.rs) ─
        KernelFn::WsDefaultCfg => "ws_server_default_cfg",
        KernelFn::WsWithOnConnect => "ws_server_with_on_connect",
        KernelFn::WsWithOnMessage => "ws_server_with_on_message",
        KernelFn::WsWithOnClose => "ws_server_with_on_close",
        KernelFn::WsWithOnError => "ws_server_with_on_error",
        KernelFn::WsWithMaxMessageBytes => "ws_server_with_max_message_bytes",
        KernelFn::WsWithOriginPatterns => "ws_server_with_origin_patterns",
        KernelFn::WsUpgrade => "server_web_socket_upgrade",
        KernelFn::WsSendToClient => "ws_server_send_to_client",
        KernelFn::WsSendBinaryToClient => "ws_server_send_binary_to_client",
        KernelFn::WsBroadcast => "ws_server_broadcast",
        KernelFn::WsCloseClient => "ws_server_close_client",
    }
}

#[cfg(test)]
mod tests {
    use super::{enum_name, kernel_name, module_value, to_camel_case, to_snake_case};
    use sky_ir::KernelFn;

    #[test]
    fn camel_case_module_prefixed() {
        assert_eq!(to_camel_case("Main_Msg"), "MainMsg");
        assert_eq!(to_camel_case("Sky_Core_Error_Error"), "SkyCoreErrorError");
    }

    #[test]
    fn snake_case_module_prefixed() {
        assert_eq!(to_snake_case("Main_update"), "main_update");
        assert_eq!(to_snake_case("Sky_Core_List_map"), "sky_core_list_map");
    }

    #[test]
    fn enum_and_value_names() {
        assert_eq!(enum_name(&["Main"], "Msg"), "MainMsg");
        assert_eq!(module_value(&["Main"], "update"), "main_update");
    }

    #[test]
    fn entry_main_is_sky_main() {
        assert_eq!(module_value(&["Main"], "main"), "sky_main");
        // `main` outside the `Main` module is NOT the entry.
        assert_eq!(module_value(&["Other"], "main"), "other_main");
    }

    #[test]
    fn record_ctor_name_is_case_preserved_and_collision_free() {
        // #82: a record-alias auto-constructor's uppercase Sky name must NOT
        // snake-case-fold into a same-spelled lowercase value's ident.
        assert_eq!(module_value(&["Main"], "Row"), "main_Row");
        assert_eq!(module_value(&["Main"], "row"), "main_row");
        assert_ne!(
            module_value(&["Main"], "Row"),
            module_value(&["Main"], "row"),
            "ctor `Row` and value `row` must be distinct idents"
        );
        // A multi-word ctor stays verbatim (never `main_user_profile`, which a
        // value `userProfile` could also produce).
        assert_eq!(
            module_value(&["Main"], "UserProfile"),
            "main_UserProfile"
        );
        assert_ne!(
            module_value(&["Main"], "UserProfile"),
            module_value(&["Main"], "userProfile"),
        );
        // Multi-segment module prefix is still snake-cased; only the ctor name
        // is preserved.
        assert_eq!(module_value(&["Lib", "State"], "Widget"), "lib_state_Widget");
    }

    #[test]
    fn kernel_names() {
        assert_eq!(kernel_name(KernelFn::StringFromInt), "string_from_int");
        assert_eq!(kernel_name(KernelFn::StringFromFloat), "string_from_float");
        assert_eq!(kernel_name(KernelFn::LogPrintln), "log_println");
        // ── Http kernels (M5b) ──────────────────────────────────────────────
        assert_eq!(kernel_name(KernelFn::HttpGet), "http_get");
        assert_eq!(kernel_name(KernelFn::HttpPost), "http_post");
        assert_eq!(kernel_name(KernelFn::HttpRequest), "http_request");
        assert_eq!(kernel_name(KernelFn::HttpParseQuery), "http_parse_query");
        assert_eq!(
            kernel_name(KernelFn::HttpDefaultRequest),
            "http_default_request"
        );
        assert_eq!(kernel_name(KernelFn::HttpWithMethod), "http_with_method");
        assert_eq!(kernel_name(KernelFn::HttpWithTimeout), "http_with_timeout");
        assert_eq!(kernel_name(KernelFn::HttpWithBody), "http_with_body");
        assert_eq!(kernel_name(KernelFn::HttpWithHeader), "http_with_header");
    }

    #[test]
    fn record_struct_names_from_field_sets() {
        use super::record_struct_name;
        assert_eq!(
            record_struct_name(&["x".to_owned(), "y".to_owned()]),
            "RecXY"
        );
        assert_eq!(
            record_struct_name(&["name".to_owned(), "age".to_owned()]),
            "RecNameAge"
        );
        // A single field.
        assert_eq!(record_struct_name(&["count".to_owned()]), "RecCount");
        // Different field sets that camel-case to the SAME base name — the
        // caller disambiguates with a numeric suffix; the base collision is a
        // documented possibility, not a panic.
        assert_eq!(record_struct_name(&["a_b".to_owned()]), "RecAB");
        assert_eq!(
            record_struct_name(&["a".to_owned(), "b".to_owned()]),
            "RecAB"
        );
    }

    #[test]
    fn non_reserved_names_pass_through() {
        assert_eq!(super::mangle_reserved("count".to_owned()), "count");
        assert_eq!(super::mangle_reserved("MainMsg".to_owned()), "MainMsg");
        assert_eq!(
            super::mangle_reserved("main_update".to_owned()),
            "main_update"
        );
    }

    #[test]
    fn reserved_names_get_a_trailing_underscore() {
        for kw in [
            "type", "fn", "match", "self", "Self", "crate", "super", "become", "priv", "typeof",
            "unsized", "virtual", "macro", "gen", "async", "await", "dyn", "true", "false",
        ] {
            assert_eq!(super::mangle_reserved(kw.to_owned()), format!("{kw}_"));
        }
    }

    #[test]
    fn reserved_collisions_mangle_at_emit_helpers() {
        // `main_loop` is not itself reserved, so the keyword segment passes
        // through unchanged once module-prefixed.
        assert_eq!(module_value(&["Main"], "loop"), "main_loop");
        // A bare module value whose snake form *is* a keyword gets mangled.
        assert_eq!(module_value(&["Type"], ""), "type_");
        // The enum-name path routes its camel result through `mangle_reserved`:
        // a camel output that lands on the keyword `Self` is mangled to `Self_`.
        assert_eq!(super::mangle_reserved(to_camel_case("Self")), "Self_");
    }
}
