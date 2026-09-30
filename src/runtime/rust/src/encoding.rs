//! Encoding kernels for Ipe.Encoding — base64 / url-percent / hex.
//! Each fn backs an Ipê-side signature declared in `src/stdlib/Ipe/Encoding.ipe`.

use super::IpeResult;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// The set of bytes `urlEncode` percent-encodes, matching
/// `url.QueryEscape` (`encodeQueryComponent`): every byte is escaped EXCEPT
/// the ASCII alphanumerics and the four unreserved marks `-` `_` `.` `~`
/// (RFC 3986 §2.3). Space is handled separately (`%20` → `+`) below.
const QUERY: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

// ── Bytes-on-Rust convention ──────────────────────────────────────────────
//
// TEXT path: the `Encoding.*` kernels below treat their `String` argument as
// TEXT and go through its UTF-8 bytes (`s.as_bytes()` on encode,
// `String::from_utf8` on decode). This avoids silent truncation (`c as u8`
// dropping every codepoint > 255) and makes `decode(encode s) == Ok s` for
// every `String`. Non-ASCII goes through the correct UTF-8 bytes, not Latin-1.
//
// BYTE path: the binary pipelines (compression / email / websocket) operate on
// `Vec<u8>` end-to-end and need no String↔bytes bridge. The `Encoding.*` text
// path and the JWT path (jwt.rs, which owns its own raw-byte base64/hex) are
// unaffected.

/// Decode one `application/x-www-form-urlencoded` field (key or value):
/// `+` → space, `%XX` → byte.
///
/// # Contract: deliberately lenient
///
/// This decoder is **intentionally permissive** about malformed input:
///
/// - A `%` not followed by exactly two hex digits (stray `%`, truncated `%A`,
///   non-hex `%ZZ`) is copied through as a literal `%` byte rather than
///   producing an error.
/// - Decoded bytes that are not valid UTF-8 are replaced with U+FFFD via
///   [`String::from_utf8_lossy`] rather than returning an error.
///
/// # Why this differs from `url_decode` / the `urlDecode` kernel
///
/// `url_decode` (the `Encoding.urlDecode` kernel) is **strict**: it rejects
/// malformed percent-escapes with `Err` at the boundary, as required for
/// user-supplied strings that will be re-encoded, used as path components, or
/// passed to security-sensitive sinks. That strictness is appropriate for
/// *application* data whose well-formedness must be guaranteed before use.
///
/// HTTP servers and clients are conventionally permissive about query strings
/// on *incoming requests*: real-world browsers and libraries emit malformed
/// escapes, and a 400 on every such request is not the right tradeoff.
/// Leniency here is correct — the decoded values feed **application logic
/// only** (a `Dict String String` handed to the route handler, or the
/// `Http.parseQuery` result in the client), never a security-sensitive
/// re-encode, path join, SQL string, or outgoing header.
///
/// # Safety boundary (invariant that makes leniency safe)
///
/// This function is called **only** at the query-string splitting layer, where
/// its output becomes a plain key/value dictionary for the application to
/// inspect. It is NOT used anywhere a malformed escape could escape the value
/// boundary: the decoded string never flows into a file path, a SQL query, an
/// outgoing HTTP header, or any re-encoding path. Callers that need a strict
/// contract must use `url_decode` instead.
///
/// Shared by `server::parse_query` (incoming request query strings) and
/// `http_client::http_parse_query` (`Http.parseQuery`) so both stay consistent.
//
// NOT cfg-gated: generated projects compile the runtime WITHOUT cargo features
// (their server.rs is always included), so a `#[cfg(feature=…)]` gate would drop
// this from generated server builds and break them. In the standalone crate it
// only looks dead under a feature subset, hence `allow(dead_code)`.
#[allow(dead_code)]
pub(crate) fn form_url_decode(s: &str) -> String {
    // A percent-escape is "%XX": a '%' marker followed by two hex digits, e.g.
    // "%20" → 0x20 (space). RFC 3986 §2.1.
    const PCT: u8 = b'%';
    const HEX: u32 = 16;
    const HEX_DIGITS: usize = 2;
    const ESCAPE_LEN: usize = 1 + HEX_DIGITS; // '%' + two hex digits

    let s = s.replace('+', " ");
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == PCT {
            // The two hex digits sit at [i+1, i+1+HEX_DIGITS). `str::get(range)`
            // is total — None when out of bounds OR not on a char boundary (e.g.
            // a stray '%' before a multi-byte char) — so we fall through and copy
            // the literal '%' rather than panicking.
            let hex = s.get(i + 1..i + 1 + HEX_DIGITS);
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, HEX).ok()) {
                out.push(byte);
                i += ESCAPE_LEN;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The RFC grammar a URL component is decoded under.
///
/// The two grammars differ in exactly one byte: under `Form`
/// (`application/x-www-form-urlencoded`, a query key or value) a `+` means a
/// space; under `Path` (an RFC 3986 path segment) a `+` is a literal `+`. Both
/// decode `%XX` to the byte `0xXX` and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlGrammar {
    /// An RFC 3986 path segment: `+` is literal.
    Path,
    /// A form-encoded query key or value: `+` is a space.
    Form,
}

/// A byte position inside the raw (still-encoded) component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteOffset(usize);

impl ByteOffset {
    /// The position as a plain byte index into the raw component.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// A component length in bytes, kept apart from positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentLen(usize);

impl ComponentLen {
    /// The length as a plain byte count.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// The largest raw component `decode_component` accepts.
///
/// Equal to the server's default request-body ceiling, so a form field that
/// fits in a body is never refused for length, while no input can make the
/// decoder allocate without a bound.
pub const MAX_COMPONENT_LEN: ComponentLen = ComponentLen(32 * 1024 * 1024);

/// Why a URL component was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeRefusal {
    /// A `%` at this raw offset is not followed by two hex digits.
    MalformedEscape { at: ByteOffset },
    /// The decoded bytes stop being UTF-8 at the escape or byte at this raw offset.
    InvalidUtf8 { at: ByteOffset },
    /// The raw component is longer than `cap` bytes.
    TooLong { cap: ComponentLen },
}

impl std::fmt::Display for DecodeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedEscape { at } => write!(
                f,
                "malformed percent-escape at byte {} (a '%' must be followed by two hex digits)",
                at.get()
            ),
            Self::InvalidUtf8 { at } => {
                write!(
                    f,
                    "decoded bytes are not valid UTF-8 (at byte {})",
                    at.get()
                )
            }
            Self::TooLong { cap } => write!(f, "component longer than {} bytes", cap.get()),
        }
    }
}

/// Decode one URL component under `grammar`, refusing anything malformed.
///
/// This is the single percent-decoder of the runtime: every URL component (a
/// path parameter, a query key or value, `Encoding.urlDecode`,
/// `Encoding.pathDecode`, `Http.parseQuery`) is decoded here. It is total and
/// strict: a `%` not followed by two hex digits, decoded bytes that are not
/// UTF-8 (overlong forms such as `%C0%AF` included), and a component longer
/// than `MAX_COMPONENT_LEN` are each a typed refusal, never a lossy or
/// pass-through success.
///
/// # Errors
///
/// Returns the `DecodeRefusal` naming the first defect found.
pub fn decode_component(raw: &str, grammar: UrlGrammar) -> Result<String, DecodeRefusal> {
    decode_component_within(raw, grammar, MAX_COMPONENT_LEN)
}

/// `decode_component` under an explicit length cap.
fn decode_component_within(
    raw: &str,
    grammar: UrlGrammar,
    cap: ComponentLen,
) -> Result<String, DecodeRefusal> {
    if raw.len() > cap.get() {
        return Err(DecodeRefusal::TooLong { cap });
    }
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&c) = bytes.get(i) {
        match c {
            b'%' => {
                let hi = bytes.get(i + 1).copied().and_then(hex_value);
                let lo = bytes.get(i + 2).copied().and_then(hex_value);
                let (Some(hi), Some(lo)) = (hi, lo) else {
                    return Err(DecodeRefusal::MalformedEscape { at: ByteOffset(i) });
                };
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'+' if grammar == UrlGrammar::Form => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|e| DecodeRefusal::InvalidUtf8 {
        at: raw_offset_of(bytes, e.utf8_error().valid_up_to()),
    })
}

/// The value of one ASCII hex digit, or `None` for any other byte.
const fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The raw offset that produced decoded byte number `decoded`.
///
/// Only called after a scan that accepted every escape, so each `%` starts a
/// three-byte escape yielding one decoded byte and every other byte yields one.
fn raw_offset_of(raw: &[u8], decoded: usize) -> ByteOffset {
    let mut i = 0;
    let mut produced = 0;
    while let Some(&c) = raw.get(i) {
        if produced == decoded {
            break;
        }
        i += if c == b'%' { 3 } else { 1 };
        produced += 1;
    }
    ByteOffset(i)
}

/// Ipê `base64Encode : String -> String` — encodes the input's UTF-8 bytes
/// )`). Non-ASCII
///
#[must_use]
pub fn base64_encode(s: String) -> String {
    B64.encode(s.as_bytes())
}

/// Ipê `base64Decode : String -> Result Error String` — decodes to bytes, then
/// requires them to be valid UTF-8 (the Ipê `String` invariant), so
/// `base64Decode (base64Encode s) == Ok s` for every `String s`. Non-UTF-8
/// payloads surface as `Err` (raw-byte round-tripping lives on `Ipe.Bytes`).
#[must_use]
pub fn base64_decode<E: From<String>>(s: String) -> IpeResult<E, String> {
    match B64.decode(s.as_bytes()) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => IpeResult::Ok(text),
            Err(e) => {
                IpeResult::Err(format!("base64: decoded bytes are not valid UTF-8: {e}").into())
            }
        },
        Err(e) => IpeResult::Err(format!("base64: {e}").into()),
    }
}

/// Ipê `urlEncode : String -> String` — space becomes `+` (not %20); the
/// ASCII unreserved set (`A-Za-z0-9` plus `-_.~`) is left verbatim; every
/// other byte is percent-encoded.
#[must_use]
pub fn url_encode(s: String) -> String {
    // QUERY encodes space as %20 (it is in the set); QueryEscape uses '+'.
    // '+' itself is not in the unreserved set, so it encodes to %2B first —
    // making the %20 → '+' swap unambiguous on decode.
    utf8_percent_encode(&s, QUERY)
        .to_string()
        .replace("%20", "+")
}

/// True when every `%` in `s` is followed by exactly two hex digits — the only
/// well-formed percent-escape shape (`%XX`, RFC 3986 §2.1). A stray `%`, a
/// truncated `%A`, or a non-hex `%ZZ` is malformed. The `percent-encoding`
/// decoder passes such input through as literal bytes and never errors, so
/// `urlDecode` scans FIRST and rejects malformed input at this untrusted
/// boundary (fail-closed) rather than silently returning the raw text.
fn is_well_formed_percent(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'%' {
            // Both trailing bytes must exist AND be ASCII hex digits.
            match (b.get(i + 1), b.get(i + 2)) {
                (Some(h1), Some(h2)) if h1.is_ascii_hexdigit() && h2.is_ascii_hexdigit() => {
                    i += 3;
                    continue;
                }
                _ => return false,
            }
        }
        i += 1;
    }
    true
}

/// Ipê `urlDecode : String -> Result Error String` — `QueryUnescape`: `+` -> space,
/// then percent-decode (so a literal `%2B` round-trips back to `+`). Fails closed
/// with `Err` on a malformed percent-escape (a `%` not followed by two hex
/// digits) and on a decode that is not valid UTF-8.
#[must_use]
pub fn url_decode<E: From<String>>(s: String) -> IpeResult<E, String> {
    let spaced = s.replace('+', " ");
    if !is_well_formed_percent(&spaced) {
        return IpeResult::Err(
            "urlDecode: malformed percent-escape (a '%' must be followed by two hex digits)"
                .to_string()
                .into(),
        );
    }
    match percent_decode_str(&spaced).decode_utf8() {
        Ok(cow) => IpeResult::Ok(cow.into_owned()),
        Err(e) => IpeResult::Err(format!("urlDecode: {e}").into()),
    }
}

/// Ipê `hexEncode : String -> String` — encodes the input's UTF-8 bytes
/// )`). Non-ASCII
/// than truncating codepoints > 255.
#[must_use]
pub fn encoding_hex_encode(s: String) -> String {
    hex::encode(s.as_bytes())
}

/// Ipê `hexDecode : String -> Result Error String` — decodes to bytes, then
/// requires them to be valid UTF-8 (the Ipê `String` invariant), so
/// `hexDecode (hexEncode s) == Ok s` for every `String s`. Non-UTF-8 payloads
/// (e.g. the hex of a raw digest) surface as `Err`; use `Ipe.Bytes.fromHex` to
/// round-trip arbitrary bytes. (jwt.rs owns its own `hex::decode` on raw
/// `&[u8]` and never routes through this kernel.)
#[must_use]
pub fn encoding_hex_decode<E: From<String>>(s: String) -> IpeResult<E, String> {
    match hex::decode(&s) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => IpeResult::Ok(text),
            Err(e) => {
                IpeResult::Err(format!("hexDecode: decoded bytes are not valid UTF-8: {e}").into())
            }
        },
        Err(e) => IpeResult::Err(format!("hexDecode: {e}").into()),
    }
}

// ── Concrete (non-generic) wrappers for generated Ipê code ─────────────
//
// The generic `base64_decode<E>`, `url_decode<E>`, `encoding_hex_decode<E>` above
// use a flexible `E: From<String>` bound so the error type can be inferred from
// surrounding context. Generated Ipê code sets `IpeError = ipe_runtime::error::
// IpeError`, but Rust's type inference cannot pin `E` when
// the error arm discards the value (e.g. `Err _ ->` in a case expression).
// These concrete aliases pin `E = IpeError` up-front, eliminating the
// ambiguity without changing the runtime semantics — construction still
// routes through `IpeError: From<String>` (classified `Unexpected`).

/// Generated-code alias for `base64_decode` with `E = IpeError`.
#[must_use]
pub fn ipe_base64_decode(s: String) -> IpeResult<crate::error::IpeError, String> {
    base64_decode(s)
}

/// Generated-code alias for `url_decode` with `E = IpeError`.
#[must_use]
pub fn ipe_url_decode(s: String) -> IpeResult<crate::error::IpeError, String> {
    url_decode(s)
}

/// Generated-code alias for `encoding_hex_decode` with `E = IpeError`.
#[must_use]
pub fn ipe_encoding_hex_decode(s: String) -> IpeResult<crate::error::IpeError, String> {
    encoding_hex_decode(s)
}

// ── Ipe.Bytes kernels ─────────────────────────────────────────
//
// `Bytes` is a distinct primitive (`Vec<u8>`); its kernel implementations
// (`bytes_to_hex`, `bytes_from_hex`, `bytes_to_base64`, `bytes_from_base64`,
// `bytes_to_string`, `bytes_length`) live in `bytes.rs`, not on a
// `type alias Bytes = String` convention. The `ipe_bytes` / `bytes_to_ipe`
// helpers below serve the Latin-1 byte-pipeline needs of `encoding.rs`,
// `compression.rs`, `ws_client.rs`, `server.rs`, and `email.rs`.

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Round-trip properties over ARBITRARY Unicode strings, co-located with the
    // kernels whose doc comments promise `decode (encode s) == Ok s` for every
    // `String s`. The example tests below pin a few fixed strings; these cover
    // the whole `String` domain — the regression class they guard is a decode
    // that stops being the exact inverse of its encoder for some input the
    // fixed cases miss. Concretely, a "fast" rewrite to Latin-1 byte coercion
    // (`c as u8`) truncates every codepoint > 255, so it would still pass the
    // ASCII fixed tests yet map distinct inputs to the same bytes and fail
    // round-trip for any non-Latin-1 char — the credential-confusion hazard the
    // `base64_no_collision_above_255` test warns about, promoted to a universal.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn base64_roundtrip_any_string(s in ".*") {
            let decoded: IpeResult<String, String> = base64_decode(base64_encode(s.clone()));
            prop_assert_eq!(decoded, IpeResult::Ok(s));
        }

        #[test]
        fn hex_roundtrip_any_string(s in ".*") {
            let decoded: IpeResult<String, String> =
                encoding_hex_decode(encoding_hex_encode(s.clone()));
            prop_assert_eq!(decoded, IpeResult::Ok(s));
        }

        // `url_encode`/`url_decode` must round-trip despite the `%20` -> `+`
        // rewrite: a literal `+`, space, and `%` are the ambiguous bytes, and the
        // encoder resolves them by emitting `%2B` for a literal `+` before the
        // swap. Any input that already contains `+`/space/`%` is exactly where a
        // naive `+`<->space swap breaks, so covering the full `String` domain
        // pins that the two are honest inverses.
        #[test]
        fn url_roundtrip_any_string(s in ".*") {
            let decoded: IpeResult<String, String> = url_decode(url_encode(s.clone()));
            prop_assert_eq!(decoded, IpeResult::Ok(s));
        }
    }

    #[test]
    fn test_base64_roundtrip() {
        let encoded = base64_encode("Hello, Ipe!".to_string());
        assert_eq!(encoded, "SGVsbG8sIElwZSE=");
        let decoded: IpeResult<String, String> = base64_decode(encoded);
        assert!(matches!(decoded, IpeResult::Ok(ref s) if s == "Hello, Ipe!"));
    }

    // non-ASCII goes through UTF-8 , not Latin-1 truncation.
    #[test]
    fn base64_hex_nonascii_utf8_bytes() {
        // base64/hex of "café" = UTF-8 bytes 63 61 66 C3 A9.
        assert_eq!(base64_encode("café".to_string()), "Y2Fmw6k=");
        assert_eq!(encoding_hex_encode("café".to_string()), "636166c3a9");
    }

    #[test]
    fn base64_hex_roundtrip_nonascii() {
        let b64: IpeResult<String, String> = base64_decode(base64_encode("café €".to_string()));
        assert!(matches!(b64, IpeResult::Ok(ref s) if s == "café €"));
        let hx: IpeResult<String, String> =
            encoding_hex_decode(encoding_hex_encode("café €".to_string()));
        assert!(matches!(hx, IpeResult::Ok(ref s) if s == "café €"));
    }

    // SECURITY: two strings that differ only ABOVE codepoint 255 must NOT
    // collide after base64 (a truncating `c as u8` would map both to 0xAC →
    // identical Basic-auth headers = credential confusion). '€'=U+20AC,
    // '¬'=U+00AC.
    #[test]
    fn base64_no_collision_above_255() {
        let euro = base64_encode("p€".to_string());
        let neg = base64_encode("p¬".to_string());
        assert_ne!(euro, neg, "distinct inputs must produce distinct base64");
    }

    #[test]
    fn test_base64_decode_invalid() {
        let bad: IpeResult<String, String> = base64_decode("not-valid-base64!@#".to_string());
        assert!(matches!(bad, IpeResult::Err(_)));
    }

    #[test]
    fn test_url_roundtrip() {
        let encoded = url_encode("hello world/foo?bar=baz&q=á".to_string());
        assert!(encoded.contains('+')); // space -> '+'
        assert!(!encoded.contains("%20"));
        assert!(encoded.contains("%2F")); // slash
        let decoded: IpeResult<String, String> = url_decode(encoded);
        assert!(matches!(decoded, IpeResult::Ok(ref s) if s == "hello world/foo?bar=baz&q=á"));
    }

    // A malformed percent-escape (a `%` not followed by two hex digits) is
    // turned away at the boundary — the documented fail-closed contract. The
    // stray/truncated/non-hex cases are the ones the raw `percent-encoding`
    // decoder passes through as literal bytes, so they must be caught by the
    // pre-scan, not the decoder.
    #[test]
    fn test_url_decode_malformed_escape() {
        for bad in ["a%ZZb", "100%done", "trailing%", "%A", "%G0", "%2"] {
            let got: IpeResult<String, String> = url_decode(bad.to_string());
            assert!(
                matches!(got, IpeResult::Err(_)),
                "malformed percent-escape {bad:?} must be rejected"
            );
        }
    }

    // The non-UTF-8 decode path stays an `Err` (a well-formed `%C0` escape whose
    // decoded byte is not valid UTF-8).
    #[test]
    fn test_url_decode_invalid_utf8() {
        let bad: IpeResult<String, String> = url_decode("bad-utf8-%C0".to_string());
        assert!(matches!(bad, IpeResult::Err(_)));
    }

    // Well-formed input — plain ASCII, `%XX` (any case), and a literal `+` —
    // must NOT be rejected by the strict scan.
    #[test]
    fn test_url_decode_well_formed_ok() {
        let space: IpeResult<String, String> = url_decode("%20".to_string());
        assert!(matches!(space, IpeResult::Ok(ref s) if s == " "));
        let plus: IpeResult<String, String> = url_decode("a+b".to_string());
        assert!(matches!(plus, IpeResult::Ok(ref s) if s == "a b"));
        let slash_lower: IpeResult<String, String> = url_decode("%2f".to_string());
        assert!(matches!(slash_lower, IpeResult::Ok(ref s) if s == "/"));
        let slash_upper: IpeResult<String, String> = url_decode("%2F".to_string());
        assert!(matches!(slash_upper, IpeResult::Ok(ref s) if s == "/"));
        let ascii: IpeResult<String, String> = url_decode("plain-ascii_1.0~".to_string());
        assert!(matches!(ascii, IpeResult::Ok(ref s) if s == "plain-ascii_1.0~"));
    }

    #[test]
    fn test_hex_roundtrip() {
        let encoded = encoding_hex_encode("Hi!".to_string());
        assert_eq!(encoded, "486921");
        let decoded: IpeResult<String, String> = encoding_hex_decode(encoded);
        assert!(matches!(decoded, IpeResult::Ok(ref s) if s == "Hi!"));
    }

    #[test]
    fn test_encoding_hex_decode_invalid() {
        let bad: IpeResult<String, String> = encoding_hex_decode("zz".to_string());
        assert!(matches!(bad, IpeResult::Err(_)));
        let odd: IpeResult<String, String> = encoding_hex_decode("a".to_string());
        assert!(matches!(odd, IpeResult::Err(_)));
    }

    // Pin the lenient contract of `form_url_decode`: malformed percent-escapes
    // pass through as literal bytes (contrast: `url_decode` rejects them with
    // `Err`). Stray `%`, truncated `%A`, non-hex `%ZZ` all survive unchanged.
    // This test guards against a future "fix" that accidentally routes
    // form_url_decode through the strict url_decode path and breaks query parsing.
    #[test]
    fn form_url_decode_lenient_contract() {
        // Normal well-formed input still decodes correctly.
        assert_eq!(form_url_decode("hello+world"), "hello world");
        assert_eq!(form_url_decode("a%20b"), "a b");
        assert_eq!(form_url_decode("foo%3Dbar"), "foo=bar");

        // Malformed percent-escapes pass through as literal bytes — lenient.
        assert_eq!(form_url_decode("100%done"), "100%done");
        assert_eq!(form_url_decode("trailing%"), "trailing%");
        assert_eq!(form_url_decode("%A"), "%A");
        assert_eq!(form_url_decode("a%ZZb"), "a%ZZb");

        // Mixed: well-formed escapes decode; malformed ones pass through.
        assert_eq!(form_url_decode("ok%20and%ZZbad"), "ok and%ZZbad");
    }

    // ── decode_component: the single strict core ──────────────────────────

    fn refusal(raw: &str, grammar: UrlGrammar) -> Option<DecodeRefusal> {
        decode_component(raw, grammar).err()
    }

    #[test]
    fn decode_component_refuses_malformed_escape() {
        for grammar in [UrlGrammar::Path, UrlGrammar::Form] {
            assert_eq!(
                refusal("a%zzb", grammar),
                Some(DecodeRefusal::MalformedEscape { at: ByteOffset(1) })
            );
            assert_eq!(
                refusal("trailing%", grammar),
                Some(DecodeRefusal::MalformedEscape { at: ByteOffset(8) })
            );
            assert_eq!(
                refusal("%A", grammar),
                Some(DecodeRefusal::MalformedEscape { at: ByteOffset(0) })
            );
            assert_eq!(
                refusal("ok%20and%ZZbad", grammar),
                Some(DecodeRefusal::MalformedEscape { at: ByteOffset(8) })
            );
            // A `%` before a multi-byte char is malformed, not a boundary panic.
            assert_eq!(
                refusal("%é", grammar),
                Some(DecodeRefusal::MalformedEscape { at: ByteOffset(0) })
            );
        }
    }

    #[test]
    fn decode_component_refuses_invalid_utf8() {
        for grammar in [UrlGrammar::Path, UrlGrammar::Form] {
            // A truncated two-byte sequence.
            assert_eq!(
                refusal("%C3", grammar),
                Some(DecodeRefusal::InvalidUtf8 { at: ByteOffset(0) })
            );
            // A lead byte followed by a non-continuation byte.
            assert_eq!(
                refusal("x%C3%28", grammar),
                Some(DecodeRefusal::InvalidUtf8 { at: ByteOffset(1) })
            );
            // The overlong encoding of `/` — the classic path-traversal smuggle.
            assert_eq!(
                refusal("a%C0%AF", grammar),
                Some(DecodeRefusal::InvalidUtf8 { at: ByteOffset(1) })
            );
        }
    }

    #[test]
    fn decode_component_plus_depends_on_grammar() {
        assert_eq!(
            decode_component("a+b", UrlGrammar::Path),
            Ok("a+b".to_string())
        );
        assert_eq!(
            decode_component("a+b", UrlGrammar::Form),
            Ok("a b".to_string())
        );
        // `%2B` is a literal `+` under both grammars, so it round-trips.
        for grammar in [UrlGrammar::Path, UrlGrammar::Form] {
            assert_eq!(decode_component("a%2Bb", grammar), Ok("a+b".to_string()));
            assert_eq!(
                decode_component("caf%C3%A9", grammar),
                Ok("café".to_string())
            );
        }
    }

    #[test]
    fn decode_component_length_cap() {
        let cap = ComponentLen(8);
        assert_eq!(
            decode_component_within("aaaaaaaa", UrlGrammar::Path, cap),
            Ok("aaaaaaaa".to_string())
        );
        assert_eq!(
            decode_component_within("aaaaaaaaa", UrlGrammar::Path, cap),
            Err(DecodeRefusal::TooLong { cap })
        );
        // The shipped cap: one byte past it is refused before any decoding.
        let past = "a".repeat(MAX_COMPONENT_LEN.get() + 1);
        assert_eq!(
            refusal(&past, UrlGrammar::Form),
            Some(DecodeRefusal::TooLong {
                cap: MAX_COMPONENT_LEN
            })
        );
    }
}
