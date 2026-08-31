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
