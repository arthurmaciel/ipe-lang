//! Encoding kernels for Ipe.Encoding — base64 / url-percent / hex
//! All fns mirror the Go runtime's `stdlib_extra.go` Encoding kernel behaviour
//! and the Ipê-side signatures declared in `ipe-stdlib/Ipê/Core/Encoding.ipe`.

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
// `String::from_utf8` on decode) — byte-for-byte with the Go backend, which
// encodes `[]byte(goString)` (UTF-8). This avoids silent truncation (`c as u8`
// dropping every codepoint > 255) and makes `decode(encode s) == Ok s` for
// every `String`. Non-ASCII goes through the correct UTF-8 bytes, not Latin-1.
//
// BYTE path: the binary pipelines (compression / email / websocket) operate on
// `Vec<u8>` end-to-end and need no String↔bytes bridge. The `Encoding.*` text
// path and the JWT path (jwt.rs, which owns its own raw-byte base64/hex) are
// unaffected.

/// Decode an application/x-www-form-urlencoded component: `+` -> space, `%XX` ->
/// byte (best-effort). Shared by the HTTP server's query parser and the HTTP
/// client's parseQuery so they stay consistent.
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

/// Ipê `urlEncode : String -> String` — Go url.QueryEscape semantics: space
/// becomes `+` (not %20); the ASCII unreserved set (`A-Za-z0-9` plus `-_.~`)
/// is left verbatim; every other byte is percent-encoded.
#[must_use]
pub fn url_encode(s: String) -> String {
    // QUERY encodes space as %20 (it is in the set); QueryEscape uses '+'.
    // '+' itself is not in the unreserved set, so it encodes to %2B first —
    // making the %20 → '+' swap unambiguous on decode.
    utf8_percent_encode(&s, QUERY)
        .to_string()
        .replace("%20", "+")
}

/// Ipê `urlDecode : String -> Result Error String` — `QueryUnescape`: `+` -> space,
/// then percent-decode (so a literal `%2B` round-trips back to `+`).
#[must_use]
pub fn url_decode<E: From<String>>(s: String) -> IpeResult<E, String> {
    let spaced = s.replace('+', " ");
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
    fn base64_hex_nonascii_match_go_utf8_bytes() {
        // Go: base64/hex of []byte("café") = UTF-8 bytes 63 61 66 C3 A9.
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
        assert!(encoded.contains('+')); // space -> '+' (Go QueryEscape)
        assert!(!encoded.contains("%20"));
        assert!(encoded.contains("%2F")); // slash
        let decoded: IpeResult<String, String> = url_decode(encoded);
        assert!(matches!(decoded, IpeResult::Ok(ref s) if s == "hello world/foo?bar=baz&q=á"));
    }

    #[test]
    fn test_url_decode_invalid() {
        let bad: IpeResult<String, String> = url_decode("bad-utf8-%C0".to_string());
        assert!(matches!(bad, IpeResult::Err(_)));
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
}
