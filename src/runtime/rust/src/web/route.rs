//! URL routing for `Web.app` — `Route<Page>` + matching, mirroring Go's
//! `matchRoute` / `applyRouteWithParams` (runtime-go/rt/live.go).
//!
//! Each `Web.route pattern ctor` lowers (codegen peephole) to a `Route` whose
//! `build` closure applies the captured `:param` strings to the page
//! constructor. `match_routes` picks the first matching route in declaration
//! order and builds its page, falling back to `not_found`.
//!
//! The builder returns `Option<Page>` so that a `:param` segment that fails to
//! decode into the expected payload type (e.g. `"abc"` for an `Int` param)
//! returns `None` and `match_routes` falls through to `not_found` rather than
//! silently substituting a default value. See `docs/divergences-from-sky.md
//! §B-route-param`.

use std::sync::Arc;

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

/// Split a URL/path into segments — Go `splitPath` parity: trim surrounding
/// `/` (so `/a/b/` and `/a/b` match the same), empty → no segments.
fn split_path(p: &str) -> Vec<&str> {
    let t = p.trim_matches('/');
    if t.is_empty() {
        Vec::new()
    } else {
        t.split('/').collect()
    }
}

/// Match `path` against `pattern` (Go `matchRoute` parity): equal segment
/// counts; a `:name` segment captures the corresponding path segment; a literal
/// segment must equal it. Returns captured params in pattern order, or `None`.
pub fn match_route(pattern: &str, path: &str) -> Option<Vec<String>> {
    let pat = split_path(pattern);
    let segs = split_path(path);
    if pat.len() != segs.len() {
        return None;
    }
    let mut params = Vec::new();
    for (ps, us) in pat.iter().zip(segs.iter()) {
        if ps.starts_with(':') {
            params.push((*us).to_string());
        } else if ps != us {
            return None;
        }
    }
    Some(params)
}

/// First route (declaration order) whose pattern matches `path` AND whose
/// builder successfully decodes all `:param` segments → its built page; else
/// `not_found` (cloned). Go `applyRouteWithParams` parity.
///
/// A route whose pattern matches but whose builder returns `None` (a `:param`
/// segment failed to decode into the expected type, e.g. `"abc"` for an `Int`
/// slot) is skipped and matching continues. This mirrors how `match_routes`
/// handles a pattern-level miss, routing the user to `not_found` instead of
/// silently substituting a zero-value default.
pub fn match_routes<Page: Clone>(routes: &[Route<Page>], not_found: &Page, path: &str) -> Page {
    for rt in routes {
        if let Some(params) = match_route(&rt.pattern, path)
            && let Some(page) = (rt.build)(params)
        {
            return page;
        }
    }
    not_found.clone()
}

/// Go `matchAnyRoute` parity: does `path` match ANY declared route? With no
/// routes only `/` is a page URL (the single-page `Web.app` shape). The page
/// handler uses this to keep unrouted GETs (browser noise like
/// `/favicon.ico`, asset probes, unknown paths) from re-routing a live
/// session's model — an unrouted re-route would rebuild the handler index
/// from the `notFound` view and orphan every handler on the page the browser
/// is actually showing.
pub fn matches_any<Page>(routes: &[Route<Page>], path: &str) -> bool {
    if routes.is_empty() {
        return path == "/";
    }
    routes
        .iter()
        .any(|rt| match_route(&rt.pattern, path).is_some())
}

/// Name→value params for the first route matching `path` — for `req.params`.
/// Zips the matched pattern's `:name` segments with the captured values.
pub fn match_params<Page>(routes: &[Route<Page>], path: &str) -> crate::dict::IpeDict<String> {
    use crate::dict::IpeDict;
    for rt in routes {
        if let Some(values) = match_route(&rt.pattern, path) {
            let names = split_path(&rt.pattern)
                .into_iter()
                .filter_map(|s| s.strip_prefix(':').map(str::to_string));
            let mut d: IpeDict<String> = IpeDict::new();
            for (n, v) in names.zip(values) {
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

        // Empty route table (single-page `Web.app`): only `/` is a page URL.
        let none: Vec<Route<Page>> = Vec::new();
        assert!(matches_any(&none, "/"));
        assert!(!matches_any(&none, "/favicon.ico"));
        assert!(!matches_any(&none, "/about"));
    }
}
