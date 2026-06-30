//! Encoding kernels for Sky.Core.Encoding — base64 / url-percent / hex
//! All fns mirror the Go runtime's `stdlib_extra.go` Encoding kernel behaviour
//! and the Sky-side signatures declared in `sky-stdlib/Sky/Core/Encoding.sky`.

use super::SkyResult;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// The set of bytes `urlEncode` percent-encodes, matching Go's
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
// Sky models raw bytes as `String` (`type alias Bytes = String`), relying on
// Go strings being arbitrary byte sequences. A Rust `String` must be valid
// UTF-8, so raw bytes (HMAC digests, hexDecode output) can't be stored as their
// literal bytes. We use a LATIN-1 convention: a "bytes" String holds one char
// per byte (codepoints U+0000..U+00FF, always valid UTF-8). The hex/base64
// kernels read input char-as-byte and emit decoded bytes byte-as-char, so the
// byte pipeline is lossless and self-consistent — `base64(hexDecode(hmac))`
// (the JWT signature path) now produces the correct bytes.
//
// Divergence from the Go backend: for NON-ASCII *text*, char-as-byte differs
// from UTF-8 bytes (e.g. 'é' -> 0xE9 here vs 0xC3 0xA9 on Go). Encode/decode
// still round-trip within the Rust backend; only the encoded string compared
// against an externally-/Go-computed value diverges. ASCII is identical to Go.

/// Interpret a (Latin-1) Sky byte-string as raw bytes: one char -> one byte.
/// Shared with other byte-handling kernels (compression, …).
pub(crate) fn sky_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u8).collect()
}

/// Wrap raw bytes as a (Latin-1) Sky byte-string: one byte -> one char.
pub(crate) fn bytes_to_sky(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

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

/// Sky `base64Encode : String -> String`
pub fn base64_encode(s: String) -> String {
    B64.encode(sky_bytes(&s))
}

/// Sky `base64Decode : String -> Result Error String`
pub fn base64_decode<E: From<String>>(s: String) -> SkyResult<E, String> {
    match B64.decode(s.as_bytes()) {
        Ok(bytes) => SkyResult::Ok(bytes_to_sky(&bytes)),
        Err(e) => SkyResult::Err(format!("base64: {}", e).into()),
    }
}

/// Sky `urlEncode : String -> String` — Go url.QueryEscape semantics: space
/// becomes `+` (not %20); the ASCII unreserved set (`A-Za-z0-9` plus `-_.~`)
/// is left verbatim; every other byte is percent-encoded.
pub fn url_encode(s: String) -> String {
    // QUERY encodes space as %20 (it is in the set); QueryEscape uses '+'.
    // '+' itself is not in the unreserved set, so it encodes to %2B first —
    // making the %20 → '+' swap unambiguous on decode.
    utf8_percent_encode(&s, QUERY)
        .to_string()
        .replace("%20", "+")
}

/// Sky `urlDecode : String -> Result Error String` — QueryUnescape: `+` -> space,
/// then percent-decode (so a literal `%2B` round-trips back to `+`).
pub fn url_decode<E: From<String>>(s: String) -> SkyResult<E, String> {
    let spaced = s.replace('+', " ");
    match percent_decode_str(&spaced).decode_utf8() {
        Ok(cow) => SkyResult::Ok(cow.into_owned()),
        Err(e) => SkyResult::Err(format!("urlDecode: {}", e).into()),
    }
}

/// Sky `hexEncode : String -> String`
pub fn encoding_hex_encode(s: String) -> String {
    hex::encode(sky_bytes(&s))
}

/// Sky `hexDecode : String -> Result Error String` — decoded bytes are returned
/// as a Latin-1 byte-string (never errors on non-UTF-8 — that's the whole point
/// of the bytes convention; the JWT signature path depends on it).
pub fn encoding_hex_decode<E: From<String>>(s: String) -> SkyResult<E, String> {
    match hex::decode(&s) {
        Ok(bytes) => SkyResult::Ok(bytes_to_sky(&bytes)),
        Err(e) => SkyResult::Err(format!("hexDecode: {}", e).into()),
    }
}

// ── Concrete (non-generic) wrappers for generated Sky code (M4f) ─────────────
//
// The generic `base64_decode<E>`, `url_decode<E>`, `encoding_hex_decode<E>` above
// use a flexible `E: From<String>` bound so the error type can be inferred from
// surrounding context. Generated Sky code always sets `SkyError = String`, but
// Rust's type inference cannot pin `E` when the error arm discards the value
// (e.g. `Err _ ->` in a case expression). These concrete aliases pin `E = String`
// up-front, eliminating the ambiguity without changing the runtime semantics.

/// Generated-code alias for `base64_decode` with `E = String`.
pub fn sky_base64_decode(s: String) -> SkyResult<String, String> {
    base64_decode(s)
}

/// Generated-code alias for `url_decode` with `E = String`.
pub fn sky_url_decode(s: String) -> SkyResult<String, String> {
    url_decode(s)
}

/// Generated-code alias for `encoding_hex_decode` with `E = String`.
pub fn sky_encoding_hex_decode(s: String) -> SkyResult<String, String> {
    encoding_hex_decode(s)
}

// ── Sky.Core.Bytes kernels (M4e) ─────────────────────────────────────────
//
// Removed: the Latin-1 String-based Bytes kernel implementations
// (`bytes_to_hex`, `bytes_from_hex`, `bytes_to_base64`, `bytes_from_base64`,
// `bytes_to_string`, `bytes_length`) that backed the OLD `type alias Bytes =
// String` convention are superseded by M4e. Sky-Rust now makes `Bytes` a
// distinct primitive (`Vec<u8>`); the new implementations live in `bytes.rs`.
// The `sky_bytes` / `bytes_to_sky` helpers below are KEPT because they are
// still used by `encoding.rs`, `compression.rs`, `ws_client.rs`, `server.rs`,
// and `email.rs` for their own Latin-1 byte-pipeline needs.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_roundtrip() {
        let encoded = base64_encode("Hello, Sky!".to_string());
        assert_eq!(encoded, "SGVsbG8sIFNreSE=");
        let decoded: SkyResult<String, String> = base64_decode(encoded);
        assert!(matches!(decoded, SkyResult::Ok(ref s) if s == "Hello, Sky!"));
    }

    #[test]
    fn test_base64_decode_invalid() {
        let bad: SkyResult<String, String> = base64_decode("not-valid-base64!@#".to_string());
        assert!(matches!(bad, SkyResult::Err(_)));
    }

    #[test]
    fn test_url_roundtrip() {
        let encoded = url_encode("hello world/foo?bar=baz&q=á".to_string());
        assert!(encoded.contains('+')); // space -> '+' (Go QueryEscape)
        assert!(!encoded.contains("%20"));
        assert!(encoded.contains("%2F")); // slash
        let decoded: SkyResult<String, String> = url_decode(encoded);
        assert!(matches!(decoded, SkyResult::Ok(ref s) if s == "hello world/foo?bar=baz&q=á"));
    }

    #[test]
    fn test_url_decode_invalid() {
        let bad: SkyResult<String, String> = url_decode("bad-utf8-%C0".to_string());
        assert!(matches!(bad, SkyResult::Err(_)));
    }

    #[test]
    fn test_hex_roundtrip() {
        let encoded = encoding_hex_encode("Hi!".to_string());
        assert_eq!(encoded, "486921");
        let decoded: SkyResult<String, String> = encoding_hex_decode(encoded);
        assert!(matches!(decoded, SkyResult::Ok(ref s) if s == "Hi!"));
    }

    #[test]
    fn test_encoding_hex_decode_invalid() {
        let bad: SkyResult<String, String> = encoding_hex_decode("zz".to_string());
        assert!(matches!(bad, SkyResult::Err(_)));
        let odd: SkyResult<String, String> = encoding_hex_decode("a".to_string());
        assert!(matches!(odd, SkyResult::Err(_)));
    }
}
