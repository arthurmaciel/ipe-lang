//! URL routing for `Live.app` — `Route<Page>` + matching, mirroring Go's
//! `matchRoute` / `applyRouteWithParams` (runtime-go/rt/live.go).
//!
//! Each `Live.route pattern ctor` lowers (codegen peephole) to a `Route` whose
//! `build` closure applies the captured `:param` strings to the page
//! constructor. `match_routes` picks the first matching route in declaration
//! order and builds its page, falling back to `not_found`.

use std::sync::Arc;

/// A declared route: a URL pattern + a builder that applies the captured
/// `:param` strings (in pattern order) to the page constructor. `Page: Clone`
/// at the match site because `not_found` is cloned on a miss.
#[derive(Clone)]
pub struct Route<Page> {
    pub pattern: String,
    pub build: Arc<dyn Fn(Vec<String>) -> Page + Send + Sync>,
}

impl<Page> Route<Page> {
    pub fn new(pattern: &str, build: impl Fn(Vec<String>) -> Page + Send + Sync + 'static) -> Self {
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

/// First route (declaration order) whose pattern matches `path` → its built
/// page; else `not_found` (cloned). Go `applyRouteWithParams` parity.
pub fn match_routes<Page: Clone>(routes: &[Route<Page>], not_found: &Page, path: &str) -> Page {
    for rt in routes {
        if let Some(params) = match_route(&rt.pattern, path) {
            return (rt.build)(params);
        }
    }
    not_found.clone()
}

/// Name→value params for the first route matching `path` — for `req.params`.
/// Zips the matched pattern's `:name` segments with the captured values.
pub fn match_params<Page>(
    routes: &[Route<Page>],
    path: &str,
) -> crate::sky_runtime::dict::SkyDict<String> {
    use crate::sky_runtime::dict::SkyDict;
    for rt in routes {
        if let Some(values) = match_route(&rt.pattern, path) {
            let names = split_path(&rt.pattern)
                .into_iter()
                .filter_map(|s| s.strip_prefix(':').map(str::to_string));
            let mut d: SkyDict<String> = SkyDict::new();
            for (n, v) in names.zip(values) {
                d.insert(n, v);
            }
            return d;
        }
    }
    SkyDict::new()
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
            Route::new("/", |_| Page::Home),
            Route::new("/apps/:slug", |p| Page::App(p[0].clone())),
            Route::new("/x/:a/:b", |p| Page::Two(p[0].clone(), p[1].clone())),
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
}
