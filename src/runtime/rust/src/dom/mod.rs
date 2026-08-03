//! Target-neutral DOM data path: structural diff (`Html` → `Vec<Patch>`),
//! the per-commit handler index, and typed form decoding.
//!
//! These are pure over `crate::html` (no tokio, no server dependency) and are
//! shared by every patch consumer: the Ipe.Web SSE wire, the Webview IPC
//! bridge, and the browser-WASM client sink. `web::mod` re-exports them so
//! existing `web::diff::Patch`-style paths stay valid.

pub mod diff;
pub use diff::*;
pub mod dispatch;
pub use dispatch::*;
// `form.rs` decodes typed form records through `serde_urlencoded`. Its only
// consumers are the Live web wire (`web::mod`) and the browser-WASM sink
// (`wasm::mod`) — both behind `web`/`wasm-client`, which is where
// `serde_urlencoded` now lives. Gated on the same union so a non-web program
// drops the module and the `serde_urlencoded`/`form_urlencoded` crates.
#[cfg(any(feature = "web", feature = "wasm-client"))]
pub mod form;
#[cfg(any(feature = "web", feature = "wasm-client"))]
pub use form::*;
pub mod req;
pub use req::*;
