//! URL routing for `Web.tea` — `Route<Page>` + matching.
//!
//! Each `Web.route pattern ctor` lowers (codegen peephole) to a `Route` whose
//! `build` closure applies the captured `:param` strings to the page
//! constructor. `match_routes` picks the first matching route in declaration
//! order and builds its page, falling back to `not_found`. Each path segment is
//! decoded exactly once, here, by the strict core ([`DecodedPath`]); a builder
//! receives decoded values and a malformed segment matches no route.
//!
//! The builder returns `Option<Page>` so that a `:param` segment that fails to
//! decode into the expected payload type (e.g. `"abc"` for an `Int` param)
//! returns `None` and `match_routes` falls through to `not_found` rather than
//! silently substituting a default value. Sanctioned divergence §B-route-param.

use std::sync::Arc;

use crate::encoding::{DecodeRefusal, decode_path_segments};

/// A declared route: a URL pattern + a builder that applies the captured
/// `:param` strings (in pattern order) to the page constructor.
///
/// `build` returns `Option<Page>` — `None` when a `:param` segment cannot be
/// decoded into the constructor's expected payload type (e.g. `"abc"` for an
/// `Int` slot). `match_routes` treats `None` as a miss and falls through to the
/// next route or `not_found`.
///
/// `Page: Clone` at the match site because `not_found` is cloned on a miss.
#[derive(Clone)]
pub struct Route<Page> {
    pub pattern: String,
    pub build: Arc<dyn Fn(Vec<String>) -> Option<Page> + Send + Sync>,
}

impl<Page> Route<Page> {
    pub fn new(
        pattern: &str,
        build: impl Fn(Vec<String>) -> Option<Page> + Send + Sync + 'static,
    ) -> Self {
        Route {
            pattern: pattern.to_string(),
            build: Arc::new(build),
        }
    }
}

/// Split a URL/path into raw segments: trim surrounding `/` (so `/a/b/` and
/// `/a/b` match the same), empty → no segments.
fn split_path(p: &str) -> Vec<&str> {
    let t = p.trim_matches('/');
    if t.is_empty() {
        Vec::new()
    } else {
        t.split('/').collect()
    }
}

/// A request path split on its raw `/` separators, each segment decoded once
/// under the RFC 3986 path grammar by the strict core
/// (`crate::encoding::decode_path_segments`).
///
/// Splitting precedes decoding, so an encoded `%2F` stays inside its segment
/// and never becomes a separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedPath(Vec<String>);

impl DecodedPath {
    /// Split `path` and decode every segment.
    ///
    /// # Errors
    ///
    /// The `DecodeRefusal` of the first segment that is not a well-formed,
    /// UTF-8 percent-encoding, or `TooLong` for an oversized path.
    pub fn parse(path: &str) -> Result<Self, DecodeRefusal> {
        decode_path_segments(path).map(Self)
    }

    /// The decoded segments, in path order.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.0
    }
}

/// The decoded values a route pattern's `:param` segments captured, in
/// pattern order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteParams(Vec<String>);

impl RouteParams {
    /// The `i`-th captured value, if the pattern has that many params.
    #[must_use]
    pub fn get(&self, i: usize) -> Option<&str> {
        self.0.get(i).map(String::as_str)
    }

    /// The captured values, for a route builder.
    #[must_use]
    pub fn into_segments(self) -> Vec<String> {
        self.0
    }
}

/// Match a decoded `path` against `pattern`: equal segment counts; a `:name`
/// segment captures the corresponding decoded segment; a literal segment must
/// equal it. Returns the captured params in pattern order, or `None`.
#[must_use]
pub fn match_route(pattern: &str, path: &DecodedPath) -> Option<RouteParams> {
    let pat = split_path(pattern);
    let segs = path.segments();
    if pat.len() != segs.len() {
        return None;
    }
    let mut params = Vec::new();
    for (ps, us) in pat.iter().zip(segs) {
        if ps.starts_with(':') {
            params.push(us.clone());
        } else if *ps != us.as_str() {
            return None;
        }
    }
    Some(RouteParams(params))
}

/// First route (declaration order) whose pattern matches `path` AND whose
/// builder successfully decodes all `:param` segments → its built page; else
/// `not_found` (cloned).
///
/// A route whose pattern matches but whose builder returns `None` (a `:param`
/// segment failed to decode into the expected type, e.g. `"abc"` for an `Int`
/// slot) is skipped and matching continues. This mirrors how `match_routes`
/// handles a pattern-level miss, routing the user to `not_found` instead of
/// silently substituting a zero-value default. A path whose segments do not
/// decode ([`DecodedPath::parse`]) matches no route.
pub fn match_routes<Page: Clone>(routes: &[Route<Page>], not_found: &Page, path: &str) -> Page {
    let Ok(decoded) = DecodedPath::parse(path) else {
        return not_found.clone();
    };
    for rt in routes {
        if let Some(params) = match_route(&rt.pattern, &decoded)
            && let Some(page) = (rt.build)(params.into_segments())
        {
            return page;
        }
    }
    not_found.clone()
}

/// Does `path` match ANY declared route? With no
/// routes only `/` is a page URL (the single-page `Web.tea` shape). The page
/// handler uses this to keep unrouted GETs (browser noise like
/// `/favicon.ico`, asset probes, unknown paths) from re-routing a live
/// session's model — an unrouted re-route would rebuild the handler index
/// from the `notFound` view and orphan every handler on the page the browser
/// is actually showing. A path whose segments do not decode matches nothing.
pub fn matches_any<Page>(routes: &[Route<Page>], path: &str) -> bool {
    if routes.is_empty() {
        return path == "/";
    }
    let Ok(decoded) = DecodedPath::parse(path) else {
        return false;
    };
    routes
        .iter()
        .any(|rt| match_route(&rt.pattern, &decoded).is_some())
}

/// Name→value params for the first route matching `path` — for `req.params`.
/// Zips the matched pattern's `:name` segments with the decoded captured
/// values. A path whose segments do not decode yields no params.
pub fn match_params<Page>(routes: &[Route<Page>], path: &str) -> crate::dict::IpeDict<String> {
    use crate::dict::IpeDict;
    let Ok(decoded) = DecodedPath::parse(path) else {
        return IpeDict::new();
    };
    for rt in routes {
        if let Some(values) = match_route(&rt.pattern, &decoded) {
            let names = split_path(&rt.pattern)
                .into_iter()
                .filter_map(|s| s.strip_prefix(':').map(str::to_string));
            let mut d: IpeDict<String> = IpeDict::new();
            for (n, v) in names.zip(values.into_segments()) {
                d.insert(n, v);
            }
            return d;
        }
    }
    IpeDict::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Page {
        Home,
        App(String),
        Two(String, String),
        NF,
    }

    fn routes() -> Vec<Route<Page>> {
        vec![
            Route::new("/", |_| Some(Page::Home)),
            Route::new("/apps/:slug", |p| Some(Page::App(p[0].clone()))),
            Route::new("/x/:a/:b", |p| Some(Page::Two(p[0].clone(), p[1].clone()))),
        ]
    }

    #[test]
    fn matches_static_and_param_in_order() {
        let rs = routes();
        assert_eq!(match_routes(&rs, &Page::NF, "/"), Page::Home);
        assert_eq!(
            match_routes(&rs, &Page::NF, "/apps/foo"),
            Page::App("foo".into())
        );
        assert_eq!(
            match_routes(&rs, &Page::NF, "/apps/foo/"),
            Page::App("foo".into())
        ); // trailing slash
        assert_eq!(
            match_routes(&rs, &Page::NF, "/x/1/2"),
            Page::Two("1".into(), "2".into())
        );
        assert_eq!(match_routes(&rs, &Page::NF, "/nope"), Page::NF); // notFound
        assert_eq!(match_routes(&rs, &Page::NF, "/apps"), Page::NF); // arity mismatch
        assert_eq!(match_routes(&rs, &Page::NF, "/apps/"), Page::NF); // trailing slash trims -> 1 seg
    }

    /// A builder returning `None` (simulates a failed `:param` decode, e.g.
    /// `"abc"` for an `Int` slot) causes `match_routes` to fall through to
    /// `not_found` rather than returning a zero-value default.
    #[test]
    fn build_none_routes_to_not_found() {
        // Route whose builder always returns None (decode failure).
        let routes: Vec<Route<Page>> = vec![
            Route::new("/items/:id", |_p| None), // always fails decode
            Route::new("/items/:id", |p| Some(Page::App(p[0].clone()))), // fallback
        ];
        // The first route matches the pattern but returns None; the second
        // matches and succeeds.
        assert_eq!(
            match_routes(&routes, &Page::NF, "/items/42"),
            Page::App("42".into())
        );
        // No route succeeds → not_found.
        let only_failing: Vec<Route<Page>> = vec![Route::new("/items/:id", |_p| None)];
        assert_eq!(
            match_routes(&only_failing, &Page::NF, "/items/abc"),
            Page::NF
        );
    }

    #[test]
    fn matches_any_routed_and_empty_table() {
        let rs = routes();
        assert!(matches_any(&rs, "/"));
        assert!(matches_any(&rs, "/apps/foo"));
        assert!(matches_any(&rs, "/apps/foo/")); // trailing slash tolerated
        assert!(!matches_any(&rs, "/favicon.ico"));
        assert!(!matches_any(&rs, "/nope"));

        // Empty route table (single-page `Web.tea`): only `/` is a page URL.
        let none: Vec<Route<Page>> = Vec::new();
        assert!(matches_any(&none, "/"));
        assert!(!matches_any(&none, "/favicon.ico"));
        assert!(!matches_any(&none, "/about"));
    }

    fn user_routes() -> Vec<Route<Page>> {
        vec![
            Route::new("/u/:id", |p| p.first().cloned().map(Page::App)),
            Route::new("/u/:a/:b", |p| {
                Some(Page::Two(p.first()?.clone(), p.get(1)?.clone()))
            }),
        ]
    }

    /// A captured `:param` reaches the builder decoded under the path grammar:
    /// `%20` is a space and `+` stays a literal `+`.
    #[test]
    fn param_is_decoded_once_under_path_grammar() {
        let rs = user_routes();
        assert_eq!(
            match_routes(&rs, &Page::NF, "/u/a%20b"),
            Page::App("a b".into())
        );
        assert_eq!(
            match_routes(&rs, &Page::NF, "/u/a+b"),
            Page::App("a+b".into())
        );
        // One decode only: `%2541` is the text `%41`, never `A`.
        assert_eq!(
            match_routes(&rs, &Page::NF, "/u/%2541"),
            Page::App("%41".into())
        );
        let params = match_params(&rs, "/u/a%20b");
        assert_eq!(params.get("id").map(String::as_str), Some("a b"));
    }

    /// An encoded `/` stays inside its segment: `/u/a%2Fb` is the one-param
    /// route with value `a/b`, never the two-param route.
    #[test]
    fn encoded_slash_is_one_segment() {
        let rs = user_routes();
        assert_eq!(
            match_routes(&rs, &Page::NF, "/u/a%2Fb"),
            Page::App("a/b".into())
        );
        let decoded = DecodedPath::parse("/u/a%2Fb").ok();
        assert_eq!(
            decoded.as_ref().map(DecodedPath::segments),
            Some(&["u".to_owned(), "a/b".to_owned()][..])
        );
        let params = decoded.and_then(|d| match_route("/u/:id", &d));
        assert_eq!(params.as_ref().and_then(|p| p.get(0)), Some("a/b"));
        assert_eq!(params.as_ref().and_then(|p| p.get(1)), None);
    }

    /// A malformed escape or a non-UTF-8 decode is refused by the parse and
    /// matches no route: `not_found`, unrouted, no params.
    #[test]
    fn malformed_segment_matches_no_route() {
        let rs = user_routes();
        for bad in ["/u/%zz", "/u/%", "/u/%4", "/u/%C0%AF", "/u/%FF"] {
            assert!(DecodedPath::parse(bad).is_err(), "{bad} must be refused");
            assert_eq!(match_routes(&rs, &Page::NF, bad), Page::NF, "{bad}");
            assert!(!matches_any(&rs, bad), "{bad}");
            assert!(match_params(&rs, bad).is_empty(), "{bad}");
        }
        assert!(matches!(
            DecodedPath::parse("/u/%zz"),
            Err(DecodeRefusal::MalformedEscape { .. })
        ));
        // A malformed segment in a literal position is refused too.
        assert!(DecodedPath::parse("/%zz/x").is_err());
    }
}
