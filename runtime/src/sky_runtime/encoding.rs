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
// TEXT path (task #55a): the `Encoding.*` kernels below treat their `String`
// argument as TEXT and go through its UTF-8 bytes (`s.as_bytes()` on encode,
// `String::from_utf8` on decode) — byte-for-byte with the Go backend, which
// encodes `[]byte(goString)` (UTF-8). This closes the old silent-truncation hole
// (`c as u8` dropped every codepoint > 255) and makes `decode(encode s) == Ok s`
// for every `String`. ASCII is unchanged; only non-ASCII moves from the old
// Latin-1 bytes to the correct UTF-8 bytes.
//
// BYTE path (task #55b, completed): the Latin-1 `sky_bytes` / `bytes_to_sky`
// helpers that the old binary pipelines (compression / email / websocket) used
// have been deleted. Those pipelines now operate on `Vec<u8>` end-to-end and no
// longer need a String↔bytes bridge. The `Encoding.*` text path and the JWT path
// (jwt.rs, which owns its own raw-byte base64/hex) are unaffected.

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

/// Sky `base64Encode : String -> String` — encodes the input's UTF-8 bytes
/// (Go parity: `base64.StdEncoding.EncodeToString([]byte(s))`). ASCII is
/// byte-identical to the old path; non-ASCII now matches Go instead of silently
/// truncating codepoints > 255 (task #55a).
pub fn base64_encode(s: String) -> String {
    B64.encode(s.as_bytes())
}

/// Sky `base64Decode : String -> Result Error String` — decodes to bytes, then
/// requires them to be valid UTF-8 (the Sky `String` invariant), so
/// `base64Decode (base64Encode s) == Ok s` for every `String s`. Non-UTF-8
/// payloads surface as `Err` (raw-byte round-tripping lives on `Std.Bytes`),
/// task #55a.
pub fn base64_decode<E: From<String>>(s: String) -> SkyResult<E, String> {
    match B64.decode(s.as_bytes()) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => SkyResult::Ok(text),
            Err(e) => {
                SkyResult::Err(format!("base64: decoded bytes are not valid UTF-8: {e}").into())
            }
        },
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

/// Sky `hexEncode : String -> String` — encodes the input's UTF-8 bytes
/// (Go parity: `hex.EncodeToString([]byte(s))`). ASCII byte-identical to the old
/// path; non-ASCII now matches Go instead of truncating codepoints > 255
/// (task #55a).
pub fn encoding_hex_encode(s: String) -> String {
    hex::encode(s.as_bytes())
}

/// Sky `hexDecode : String -> Result Error String` — decodes to bytes, then
/// requires them to be valid UTF-8 (the Sky `String` invariant), so
/// `hexDecode (hexEncode s) == Ok s` for every `String s`. Non-UTF-8 payloads
/// (e.g. the hex of a raw digest) surface as `Err`; use `Std.Bytes.fromHex` to
/// round-trip arbitrary bytes. Task #55a. (jwt.rs owns its own `hex::decode` on
/// raw `&[u8]` and never routed through this kernel.)
pub fn encoding_hex_decode<E: From<String>>(s: String) -> SkyResult<E, String> {
    match hex::decode(&s) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => SkyResult::Ok(text),
            Err(e) => {
                SkyResult::Err(format!("hexDecode: decoded bytes are not valid UTF-8: {e}").into())
            }
        },
        Err(e) => SkyResult::Err(format!("hexDecode: {}", e).into()),
    }
}

// ── Concrete (non-generic) wrappers for generated Sky code (M4f) ─────────────
//
// The generic `base64_decode<E>`, `url_decode<E>`, `encoding_hex_decode<E>` above
// use a flexible `E: From<String>` bound so the error type can be inferred from
// surrounding context. Generated Sky code sets `SkyError = sky_runtime::error::
// SkyError` (backlog #85/#160), but Rust's type inference cannot pin `E` when
// the error arm discards the value (e.g. `Err _ ->` in a case expression).
// These concrete aliases pin `E = SkyError` up-front, eliminating the
// ambiguity without changing the runtime semantics — construction still
// routes through `SkyError: From<String>` (classified `Unexpected`).

/// Generated-code alias for `base64_decode` with `E = SkyError`.
pub fn sky_base64_decode(s: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
    base64_decode(s)
}

/// Generated-code alias for `url_decode` with `E = SkyError`.
pub fn sky_url_decode(s: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
    url_decode(s)
}

/// Generated-code alias for `encoding_hex_decode` with `E = SkyError`.
pub fn sky_encoding_hex_decode(s: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
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

    // #55a — non-ASCII goes through UTF-8 (Go parity), not Latin-1 truncation.
    #[test]
    fn base64_hex_nonascii_match_go_utf8_bytes() {
        // Go: base64/hex of []byte("café") = UTF-8 bytes 63 61 66 C3 A9.
        assert_eq!(base64_encode("café".to_string()), "Y2Fmw6k=");
        assert_eq!(encoding_hex_encode("café".to_string()), "636166c3a9");
    }

    #[test]
    fn base64_hex_roundtrip_nonascii() {
        let b64: SkyResult<String, String> = base64_decode(base64_encode("café €".to_string()));
        assert!(matches!(b64, SkyResult::Ok(ref s) if s == "café €"));
        let hx: SkyResult<String, String> =
            encoding_hex_decode(encoding_hex_encode("café €".to_string()));
        assert!(matches!(hx, SkyResult::Ok(ref s) if s == "café €"));
    }

    // SECURITY (#55a): two strings that differ only ABOVE codepoint 255 must NOT
    // collide after base64 (the old `c as u8` truncated both to 0xAC → identical
    // Basic-auth headers = credential confusion). '€'=U+20AC, '¬'=U+00AC.
    #[test]
    fn base64_no_collision_above_255() {
        let euro = base64_encode("p€".to_string());
        let neg = base64_encode("p¬".to_string());
        assert_ne!(euro, neg, "distinct inputs must produce distinct base64");
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
