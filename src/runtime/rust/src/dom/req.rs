//! `WebReq` — the typed request context passed to a TEA `init`.
//!
//! Target-neutral: the server builds it from the incoming HTTP parts
//! (`web::req::web_req`), the browser-WASM client synthesises it from
//! `location` + `document.cookie`. Fields mirror the  record.

use crate::dict::IpeDict;

#[derive(Clone, Debug)]
pub struct WebReq {
    pub path: String,
    pub query: String,
    pub method: String,
    pub params: IpeDict<String>,
    pub headers: IpeDict<String>,
    pub cookies: IpeDict<String>,
}

impl WebReq {
    /// The initial-load request for a host with no incoming HTTP request — a
    /// native window (`web desktop` webview) whose app opens at its root. The
    /// same `WebReq` a browser tab reports on a fresh `GET /` load: a `GET`
    /// method, the root path, and no query, params, headers, or cookies. A
    /// `Web.app` `init : WebReq -> …` receives this so the webview host runs the
    /// SAME init as the served and WASM hosts, never a separate `()` shape.
    #[must_use]
    pub fn local_root() -> Self {
        Self {
            path: "/".to_owned(),
            query: String::new(),
            method: "GET".to_owned(),
            params: IpeDict::new(),
            headers: IpeDict::new(),
            cookies: IpeDict::new(),
        }
    }
}
