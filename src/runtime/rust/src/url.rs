//! `Ipe.Url` — a typed, opaque URL (parse-don't-validate).
//!
//! The ONLY way to obtain a `Url` is through [`url_from_string`] (the seal): it
//! parses the raw string with the `url` crate — the SAME parser
//! `ipe_runtime::ssrf` uses to validate outbound-request targets (`reqwest::Url`
//! is `pub use url::Url`), so the type boundary and the SSRF guard share one
//! canonical parse and cannot diverge on what counts as a valid URL.
//!
//! A `Url` is always ABSOLUTE: `url::Url::parse` rejects a scheme-less / relative
//! string (`"/path"`, `"example.com"`), so a value of this type always carries a
//! scheme and (for a hierarchical scheme) a host. This is the property the SSRF
//! guard depends on — a host to check — so making it a construction invariant
//! means downstream code never re-encounters a scheme-confused or hostless URL.
//!
//! The `Url.Builder` primitive [`url_build_query`] percent-encodes every key and
//! value through the `url` crate's `form_urlencoded` serializer, so a caller
//! building a query string cannot forget to encode a metacharacter (`&`, `=`,
//! ` `, `#`) — closing the query-injection footgun that raw string-concatenation
//! leaves open.
//!
//! # Trust model — what `Url` does and does NOT guarantee
//!
//! `Url` guarantees the string is a syntactically valid absolute URL with a
//! scheme. It deliberately does NOT decide whether that URL is SAFE to fetch —
//! an `http://169.254.169.254/` is a perfectly valid `Url`. The SSRF
//! scheme-allowlist and private-IP-deny policy (`ipe_runtime::ssrf`) is the
//! separate runtime authority over which validated URLs an outbound request may
//! actually reach; `Url` is the syntactic parse boundary that feeds it.

use super::IpeResult;
use crate::core::IpeMaybe;
use url::{form_urlencoded, Url as UrlCrate};

/// `Ipe.Url`'s opaque, validated newtype. See the module doc for the
/// construction contract. The wrapped `url::Url` is always an absolute,
/// scheme-carrying URL produced by [`url_from_string`].
///
/// `Clone` / `Debug` / `PartialEq` / `Eq` are derived on `url::Url` and safe: a
/// URL is not a secret, so printing or comparing it leaks nothing the caller did
/// not already hand in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url(UrlCrate);

impl super::stringify::IpeStringify for Url {
    /// Backs Ipê's `toString` / interpolation on a `Url`: the serialized URL
    /// string. Identical to [`url_to_string`].
    fn ipe_show(&self) -> String {
        self.0.as_str().to_string()
    }
}

/// `Ipe.Url.fromString : String -> Result Error Url` — THE seal. The only public
/// constructor: every `Url` value in a Ipê program traces back to one of these
/// calls, so a reviewer can `grep` this one symbol to audit every place a raw
/// string becomes a typed URL.
///
/// Fails closed (`Err`) when the string is not a syntactically valid ABSOLUTE
/// URL — a relative reference (`"/path"`), a scheme-less host (`"example.com"`),
/// or otherwise unparseable input all surface as a typed `Err`, never a silent
/// accept. Succeeds with the parsed, normalised URL otherwise.
#[must_use]
pub fn url_from_string<E: From<String>>(s: String) -> IpeResult<E, Url> {
    match UrlCrate::parse(&s) {
        Ok(u) => IpeResult::Ok(Url(u)),
        Err(e) => IpeResult::Err(format!("Ipe.Url: not a valid absolute URL: {s:?} ({e})").into()),
    }
}

/// `Ipe.Url.toString : Url -> String` — THE un-parse: recover the serialized URL
/// string. Consumes the `Url` (the typed proof is spent when the raw string
/// comes back out).
#[must_use]
pub fn url_to_string(u: Url) -> String {
    u.0.into()
}

/// `Ipe.Url.scheme : Url -> String` — the URL's scheme (`"https"`, `"http"`, …),
/// always lowercase and always present (an absolute URL has one).
#[must_use]
pub fn url_scheme(u: Url) -> String {
    u.0.scheme().to_string()
}

/// `Ipe.Url.host : Url -> Maybe String` — the host component (a registered name
/// or IP literal), or `Nothing` for a scheme whose URLs have no host (e.g.
/// `mailto:` / `data:`).
#[must_use]
pub fn url_host(u: Url) -> IpeMaybe<String> {
    match u.0.host_str() {
        Some(h) => IpeMaybe::Just(h.to_string()),
        None => IpeMaybe::Nothing,
    }
}

/// `Ipe.Url.port : Url -> Maybe Int` — the port, taking the scheme's known
/// default into account (`https://x` → `443`), or `Nothing` when neither an
/// explicit port nor a known default exists.
#[must_use]
pub fn url_port(u: Url) -> IpeMaybe<i64> {
    match u.0.port_or_known_default() {
        Some(p) => IpeMaybe::Just(i64::from(p)),
        None => IpeMaybe::Nothing,
    }
}

/// `Ipe.Url.path : Url -> String` — the path component (begins with `/` for a
/// hierarchical URL; `""` for a URL with no path).
#[must_use]
pub fn url_path(u: Url) -> String {
    u.0.path().to_string()
}

/// `Ipe.Url.query : Url -> Maybe String` — the raw query string (WITHOUT the
/// leading `?`), or `Nothing` when the URL has no query.
#[must_use]
pub fn url_query(u: Url) -> IpeMaybe<String> {
    match u.0.query() {
        Some(q) => IpeMaybe::Just(q.to_string()),
        None => IpeMaybe::Nothing,
    }
}

/// `Ipe.Url.fragment : Url -> Maybe String` — the fragment (WITHOUT the leading
/// `#`), or `Nothing` when the URL has no fragment.
#[must_use]
pub fn url_fragment(u: Url) -> IpeMaybe<String> {
    match u.0.fragment() {
        Some(f) => IpeMaybe::Just(f.to_string()),
        None => IpeMaybe::Nothing,
    }
}

/// `Ipe.Url.Builder.queryString : List (String, String) -> String` — the
/// injection-safe query-string builder. Percent-encodes EVERY key and value
/// through the `url` crate's `form_urlencoded` serializer, so a caller cannot
/// forget to encode a metacharacter: an `&` / `=` / space / `#` in a value is
/// encoded, never emitted raw where it would split off a new parameter (a
/// query-injection). Returns the encoded string WITHOUT a leading `?`; the empty
/// list yields `""`.
#[must_use]
pub fn url_build_query(pairs: Vec<(String, String)>) -> String {
    let mut ser = form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(&k, &v);
    }
    ser.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Url {
        match url_from_string::<String>(s.to_string()) {
            IpeResult::Ok(u) => u,
            IpeResult::Err(e) => panic!("expected {s:?} to be a valid Url, got Err: {e}"),
        }
    }

    // ── (a) a valid URL round-trips through parse → accessors → build ─────────

    #[test]
    fn valid_url_parses_and_exposes_typed_components() {
        let u = parse("https://user@example.com:8443/a/b?q=1#frag");
        assert_eq!(url_scheme(u.clone()), "https");
        assert_eq!(url_host(u.clone()), IpeMaybe::Just("example.com".to_string()));
        assert_eq!(url_port(u.clone()), IpeMaybe::Just(8443));
        assert_eq!(url_path(u.clone()), "/a/b");
        assert_eq!(url_query(u.clone()), IpeMaybe::Just("q=1".to_string()));
        assert_eq!(url_fragment(u), IpeMaybe::Just("frag".to_string()));
    }

    #[test]
    fn to_string_round_trips_the_parsed_url() {
        let raw = "https://example.com/a?x=1";
        assert_eq!(url_to_string(parse(raw)), raw);
    }

    #[test]
    fn port_falls_back_to_the_scheme_default() {
        // No explicit port → the known default for `https` is 443.
        assert_eq!(url_port(parse("https://example.com/")), IpeMaybe::Just(443));
    }

    // ── (b) an invalid / relative URL is a typed absence (Err) ───────────────

    #[test]
    fn relative_reference_is_rejected() {
        let r: IpeResult<String, Url> = url_from_string("/just/a/path".to_string());
        assert!(
            matches!(r, IpeResult::Err(_)),
            "a scheme-less relative reference must be a typed Err, never a silent accept"
        );
    }

    #[test]
    fn scheme_less_host_is_rejected() {
        let r: IpeResult<String, Url> = url_from_string("example.com/x".to_string());
        assert!(
            matches!(r, IpeResult::Err(_)),
            "a bare host with no scheme is not an absolute URL"
        );
    }

    #[test]
    fn garbage_is_rejected() {
        let r: IpeResult<String, Url> = url_from_string("not a url at all".to_string());
        assert!(matches!(r, IpeResult::Err(_)));
    }

    #[test]
    fn mailto_has_no_host() {
        // A `mailto:` URL is a valid absolute URL but carries no host component.
        assert_eq!(url_host(parse("mailto:a@b.com")), IpeMaybe::Nothing);
    }

    // ── (c) the builder percent-encodes metacharacters (no injection) ────────

    #[test]
    fn builder_encodes_query_metacharacters_no_injection() {
        // A value containing `&`, `=`, ` ` and `#` must NOT be able to split off a
        // new parameter or terminate the query — every metacharacter is encoded.
        let q = url_build_query(vec![
            ("q".to_string(), "a&b=c d#e".to_string()),
            ("next".to_string(), "/dashboard".to_string()),
        ]);
        // No raw `&`/`=` from the value leaks into a parameter boundary: the only
        // `&` is the ONE the serializer put between the two pairs, and the only
        // `=` are the two key/value separators.
        assert_eq!(q.matches('&').count(), 1, "exactly one pair separator");
        assert_eq!(q.matches('=').count(), 2, "exactly one `=` per pair");
        assert!(
            !q.contains(' ') && !q.contains('#'),
            "space and `#` must be percent-encoded, never raw: {q}"
        );
        // The encoded value round-trips back to the original via a re-parse — the
        // proof that encoding is lossless, not lossy sanitisation.
        let round: std::collections::HashMap<String, String> =
            form_urlencoded::parse(q.as_bytes()).into_owned().collect();
        assert_eq!(round.get("q").map(String::as_str), Some("a&b=c d#e"));
        assert_eq!(round.get("next").map(String::as_str), Some("/dashboard"));
    }

    #[test]
    fn builder_empty_list_is_empty_string() {
        assert_eq!(url_build_query(Vec::new()), "");
    }

    #[test]
    fn built_query_composes_into_a_valid_url() {
        // End-to-end: build a query, splice it, re-parse — the whole loop closes.
        let q = url_build_query(vec![("name".to_string(), "a b&c".to_string())]);
        let u = parse(&format!("https://example.com/search?{q}"));
        assert_eq!(url_query(u), IpeMaybe::Just(q));
    }
}
