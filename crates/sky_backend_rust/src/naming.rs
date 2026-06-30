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
        KernelFn::MaybeWithDefault => "maybe_with_default",
        KernelFn::MaybeMap => "sky_maybe_map",
        KernelFn::MaybeAndThen => "sky_maybe_and_then",
        KernelFn::ResultWithDefault => "result_with_default",
        KernelFn::ResultMap => "sky_result_map",
        // `Math.min` / `Math.max` map to the runtime's generic
        // `math_min<T: PartialOrd>` / `math_max<T: PartialOrd>`: a real
        // polymorphic compare at the argument's actual type — NO `Int`
        // coercion, NO float truncation. Divergence from Sky (PR #136):
        // Sky routes args through AsInt; Sky-Rust follows Elm's polymorphic
        // comparable (a -> a -> a). Rationale: Elm-conformance.
        KernelFn::MathMin => "math_min",
        KernelFn::MathMax => "math_max",
        // ── Math constants ───────────────────────────────────────────────────
        KernelFn::MathPi => "math_pi",
        KernelFn::MathE => "math_e",
        KernelFn::MathPhi => "math_phi",
        KernelFn::MathSqrt2 => "math_sqrt2",
        KernelFn::MathInf => "math_inf",
        KernelFn::MathNan => "math_nan",
        // ── Math arity-1 (Int → Int) ─────────────────────────────────────────
        KernelFn::MathAbs => "math_abs",
        // ── Math arity-1 (Float → Float) ────────────────────────────────────
        KernelFn::MathSqrt => "math_sqrt",
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
        KernelFn::JsonDecMap => "decode_map",
        KernelFn::JsonDecAndThen => "decode_and_then",
        KernelFn::JsonDecSucceed => "decode_succeed",
        KernelFn::JsonDecFail => "decode_fail",
        KernelFn::JsonDecOneOf => "decode_one_of",
        KernelFn::JsonDecMap2 => "decode_map2",
        KernelFn::JsonDecMap3 => "decode_map3",
        KernelFn::JsonDecMap4 => "decode_map4",
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
        // uuid_v4 / uuid_v7 return String directly — no E type, no concretisation.
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
    fn kernel_names() {
        assert_eq!(kernel_name(KernelFn::StringFromInt), "string_from_int");
        assert_eq!(kernel_name(KernelFn::StringFromFloat), "string_from_float");
        assert_eq!(kernel_name(KernelFn::LogPrintln), "log_println");
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
