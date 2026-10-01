//! URL routing for `Web.tea` — `Route<Page>` + matching.
//!
//! Each `Web.route pattern ctor` lowers (codegen peephole) to a `Route` whose
//! `build` closure applies the captured `:param` strings to the page
//! constructor. `match_routes` picks the first matching route in declaration
//! order and builds its page, falling back to `not_found`.
//!
//! The builder returns `Option<Page>` so that a `:param` segment that fails to
//! decode into the expected payload type (e.g. `"abc"` for an `Int` param)
//! returns `None` and `match_routes` falls through to `not_found` rather than
//! silently substituting a default value. Sanctioned divergence §B-route-param.

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

/// Split a URL/path into segments: trim surrounding `/` (so `/a/b/` and
/// `/a/b` match the same), empty → no segments.
fn split_path(p: &str) -> Vec<&str> {
    let t = p.trim_matches('/');
    if t.is_empty() {
        Vec::new()
    } else {
        t.split('/').collect()
    }
}

/// A path in the matcher's canonical form: one leading `/`, no trailing `/`.
///
/// Two paths [`split_path`] splits alike (`/a/b` and `a/b/`, `` and `/`)
/// build one `RoutePath`, so comparing entered paths agrees with matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutePath(String);

impl RoutePath {
    /// The canonical form of `path`.
    pub fn of(path: &str) -> Self {
        RoutePath(format!("/{}", path.trim_matches('/')))
    }

    /// The canonical path text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Match `path` against `pattern`: equal segment counts; a `:name` segment
/// captures the corresponding path segment; a literal segment must equal it.
/// Returns captured params in pattern order, or `None`.
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
/// `not_found` (cloned).
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

/// Does `path` match ANY declared route? With no
/// routes only `/` is a page URL (the single-page `Web.tea` shape). The page
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

/// The result of entering a routed page: the model to commit and the Cmd to run once.
///
/// `#[must_use]` so a caller that drops the entry (and with it the page's
/// load Cmd) is a denied warning, never a silently skipped page load.
///
/// Generic over the Cmd carrier `C` (the platform's `IpeCmd<Msg>`), so the
/// server-free render core shares it without the TEA loop.
#[must_use]
pub struct Entered<M, C> {
    pub model: M,
    pub cmd: C,
}

/// Enter `path`: match it against `routes` and apply the app's entry fn to the matched page.
///
/// The single URL-to-model entry every platform shares (server GET, SSE
/// reconnect, wasm mount, popstate, in-app navigation). The entry fn is the
/// app's `set_page` (`onNavigate` routed through `update`, or the implicit
/// `{ model | page }` paired with `Cmd.none`); its Cmd is returned, never dropped.
pub fn enter<Page: Clone, M, C>(
    routes: &[Route<Page>],
    not_found: &Page,
    path: &str,
    model: M,
    entry: impl Fn(Page, M) -> (M, C),
) -> Entered<M, C> {
    let (model, cmd) = entry(match_routes(routes, not_found, path), model);
    Entered { model, cmd }
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

    /// `enter` hands back the entry fn's Cmd beside its model, so no caller can
    /// commit the page and lose the page's load.
    #[test]
    fn enter_returns_the_entry_fns_cmd() {
        let rs = routes();
        let entry = |page: Page, count: u32| {
            let cmd = match page {
                Page::App(slug) => format!("load {slug}"),
                _ => String::new(),
            };
            (count + 1, cmd)
        };
        let entered = enter(&rs, &Page::NF, "/apps/abc", 0_u32, entry);
        assert_eq!(entered.model, 1);
        assert_eq!(
            entered.cmd, "load abc",
            "enter must return the entry fn's Cmd for the matched page"
        );
        let missed = enter(&rs, &Page::NF, "/nope", 5_u32, entry);
        assert_eq!(missed.model, 6, "an unknown path enters notFound");
        assert_eq!(missed.cmd, "");
    }

    /// Paths the matcher splits alike share one `RoutePath`; paths it splits
    /// apart never do.
    #[test]
    fn route_path_equality_agrees_with_split_path() {
        let alike = [
            ("/", ""),
            ("/", "//"),
            ("/items/5", "items/5/"),
            ("/a//b", "a//b/"),
        ];
        for (a, b) in alike {
            assert_eq!(split_path(a), split_path(b));
            assert_eq!(RoutePath::of(a), RoutePath::of(b), "{a:?} vs {b:?}");
        }
        let apart = [("/items", "/items/5"), ("/a/b", "/a//b"), ("/", "/x")];
        for (a, b) in apart {
            assert_ne!(split_path(a), split_path(b));
            assert_ne!(RoutePath::of(a), RoutePath::of(b), "{a:?} vs {b:?}");
        }
        assert_eq!(RoutePath::of("").as_str(), "/");
    }
}
