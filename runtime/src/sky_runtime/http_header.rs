//! Canonical HTTP header-name casing, shared by Sky.Live and Sky.Http.Server.
//!
//! Go stores request header names in canonical MIME case
//! (`textproto.CanonicalMIMEHeaderKey`: `content-type` -> `Content-Type`,
//! `x-sky-live` -> `X-Sky-Live`) and `r.Header.Get` canonicalises the lookup
//! key, so a handler asking for either `"content-type"` or `"Content-Type"`
//! resolves. hyper/axum expose request header names lower-cased, so the Rust
//! runtime must re-derive the canonical form at the request boundary and use it
//! for both storage and lookup. This module is the single source of truth for
//! that transformation so the Live request builder and the Server request
//! builder + `Server.header` lookup can never drift to two divergent casings.

/// Canonicalise a `-`-separated header name (`content-type` -> `Content-Type`).
///
/// Upper-cases the first ASCII letter of each `-`-separated segment and
/// lower-cases the rest, matching Go's `textproto.CanonicalMIMEHeaderKey` for
/// every well-formed (valid-token) header name: valid token bytes are ASCII,
/// only `-` triggers the next-uppercase in both implementations, and `_`/`.`/
/// digits are non-triggers in both.
///
/// Accepted divergence (see the parity test): Go returns the name **unchanged**
/// when it contains a byte outside the header-token set (e.g. a space or a
/// non-ASCII byte), whereas this always title-cases per segment. Such names
/// cannot reach the request boundary — hyper/axum reject invalid header names
/// on parse — and for `Server.header` lookups the observable result (Just /
/// Nothing) is identical to Go regardless, because the canonical form is never
/// surfaced to Sky, only used to key an already-canonical map.
pub(crate) fn canonical_header(k: &str) -> String {
    k.split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::canonical_header;

    /// Well-formed header names — byte-identical to Go's
    /// `textproto.CanonicalMIMEHeaderKey` (oracle probed against Go 2026-07-02).
    #[test]
    fn canonical_header_matches_go_canonical_mime_key() {
        for (input, want) in [
            ("content-type", "Content-Type"),
            ("CONTENT-TYPE", "Content-Type"),
            ("Content-Type", "Content-Type"),
            ("x-forwarded-for", "X-Forwarded-For"),
            ("x-sky-live", "X-Sky-Live"),
            ("etag", "Etag"),
            ("www-authenticate", "Www-Authenticate"),
            ("host", "Host"),
            ("a", "A"),
            ("", ""),
            ("MiXeD-cAsE", "Mixed-Case"),
            ("x_custom", "X_custom"),
            ("123-abc", "123-Abc"),
            ("x--y", "X--Y"),
            ("-x", "-X"),
            ("x-", "X-"),
        ] {
            assert_eq!(canonical_header(input), want, "input {input:?}");
        }
    }

    /// Invalid-token names are the accepted divergence: Go returns them
    /// unchanged, we title-case them. These names cannot reach the request
    /// boundary (hyper/axum reject them on parse), so the divergence is
    /// unobservable in practice; this test pins our behaviour so any future
    /// change to the canonicaliser is caught.
    #[test]
    fn canonical_header_invalid_token_is_accepted_divergence() {
        assert_eq!(canonical_header("foo bar"), "Foo bar");
        assert_eq!(canonical_header("über-key"), "über-Key");
    }
}
