//! Target-neutral DOM data path: structural diff (`Html` → `Vec<Patch>`),
//! the per-commit handler index, and typed form decoding.
//!
//! These are pure over `crate::html` (no tokio, no server dependency) and are
//! shared by every patch consumer: the Ipe.Web SSE wire, the WebView IPC
//! bridge, and the browser-WASM client sink. `dom::mod` re-exports them so
//! `dom::diff::Patch`-style paths resolve.

pub mod diff;
pub use diff::*;
pub mod dispatch;
pub use dispatch::*;
pub mod form;
pub use form::*;
pub mod req;
pub use req::*;
