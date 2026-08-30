//! `WebReq` — the typed request context passed to an `Ipe.Web` `init`.
//!
//! `req.path` / `req.query` / `req.method` are strings;
//! `req.params` / `req.headers` / `req.cookies` are `Dict String String`.

use crate::dict::IpeDict;

pub use crate::dom::req::WebReq;

/// Build a `WebReq` from the incoming request parts + the matched route params.
pub fn web_req(
    method: &axum::http::Method,
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
    params: IpeDict<String>,
) -> WebReq {
    let mut hdrs: IpeDict<String> = IpeDict::new();
    for (k, v) in headers.iter() {
        if let Ok(val) = v.to_str() {
            // First-value-wins on duplicate header keys, matching Go's
            // `headersToDict` (`vs[0]`). axum yields multi-valued headers in
            // arrival order, so the first `iter()` entry is the first value.
            hdrs.entry(crate::http_header::canonical_header(k.as_str()))
                .or_insert_with(|| val.to_string());
        }
    }
    let mut cookies: IpeDict<String> = IpeDict::new();
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
    WebReq {
        path: uri.path().to_string(),
        query: uri.query().unwrap_or("").to_string(),
        method: method.as_str().to_string(),
        params,
        headers: hdrs,
        cookies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_req_parses_headers_and_cookies() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            axum::http::header::COOKIE,
            "ipe_sid=abc; theme=dark".parse().unwrap(),
        );
        h.insert("x-custom", "v".parse().unwrap());
        let uri: axum::http::Uri = "/apps/ipe?q=1".parse().unwrap();
        let req = web_req(
            &axum::http::Method::GET,
            &uri,
            &h,
            crate::dict::dict_empty(),
        );
        assert_eq!(req.path, "/apps/ipe");
        assert_eq!(req.query, "q=1");
        assert_eq!(req.method, "GET");
        assert_eq!(req.cookies.get("ipe_sid").map(String::as_str), Some("abc"));
        assert_eq!(req.cookies.get("theme").map(String::as_str), Some("dark"));
        assert_eq!(req.headers.get("X-Custom").map(String::as_str), Some("v"));
    }
}
