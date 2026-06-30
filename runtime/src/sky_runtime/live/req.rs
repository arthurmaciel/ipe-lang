//! `LiveReq` — the typed request context passed to a Sky.Live `init`.
//!
//! Mirrors the modern Go `req` record: `req.path` / `req.query` /
//! `req.method` are strings; `req.params` / `req.headers` / `req.cookies` are
//! `Dict String String`. (Go's older heterogeneous-Dict form — `Dict.get "path"
//! req` over a `map[string]any` — doesn't port to Rust's no-`any` runtime, so
//! the Rust backend uses the typed-record form only.)

use crate::sky_runtime::dict::SkyDict;

#[derive(Clone, Debug)]
pub struct LiveReq {
    pub path: String,
    pub query: String,
    pub method: String,
    pub params: SkyDict<String>,
    pub headers: SkyDict<String>,
    pub cookies: SkyDict<String>,
}

/// Build a `LiveReq` from the incoming request parts + the matched route params.
pub fn live_req(
    method: &axum::http::Method,
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
    params: SkyDict<String>,
) -> LiveReq {
    let mut hdrs: SkyDict<String> = SkyDict::new();
    for (k, v) in headers.iter() {
        if let Ok(val) = v.to_str() {
            // First-value-wins on duplicate header keys, matching Go's
            // `headersToDict` (`vs[0]`). axum yields multi-valued headers in
            // arrival order, so the first `iter()` entry is the first value.
            hdrs.entry(canonical_header(k.as_str()))
                .or_insert_with(|| val.to_string());
        }
    }
    let mut cookies: SkyDict<String> = SkyDict::new();
    if let Some(c) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        for pair in c.split(';') {
            if let Some((k, v)) = pair.trim().split_once('=') {
                cookies.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    LiveReq {
        path: uri.path().to_string(),
        query: uri.query().unwrap_or("").to_string(),
        method: method.as_str().to_string(),
        params,
        headers: hdrs,
        cookies,
    }
}

/// Title-Case a `-`-separated header name: "content-type" -> "Content-Type"
/// (axum lower-cases header keys; Go reports canonical case).
fn canonical_header(k: &str) -> String {
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
    use super::*;

    #[test]
    fn live_req_parses_headers_and_cookies() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            axum::http::header::COOKIE,
            "sky_sid=abc; theme=dark".parse().unwrap(),
        );
        h.insert("x-custom", "v".parse().unwrap());
        let uri: axum::http::Uri = "/apps/sky?q=1".parse().unwrap();
        let req = live_req(
            &axum::http::Method::GET,
            &uri,
            &h,
            crate::sky_runtime::dict::dict_empty(),
        );
        assert_eq!(req.path, "/apps/sky");
        assert_eq!(req.query, "q=1");
        assert_eq!(req.method, "GET");
        assert_eq!(req.cookies.get("sky_sid").map(String::as_str), Some("abc"));
        assert_eq!(req.cookies.get("theme").map(String::as_str), Some("dark"));
        assert_eq!(req.headers.get("X-Custom").map(String::as_str), Some("v"));
    }
}
