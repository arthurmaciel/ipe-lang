//! The Ipê playground library surface.
//!
//! The binary ([`main`](../ipe_playground/index.html)) is the axum server; this
//! library exposes the security-critical sandboxed build+run machinery so it can
//! be exercised by integration tests (the jail-holds proofs) independently of the
//! HTTP layer.

#![allow(clippy::missing_errors_doc)] // internal helpers, not public API

pub mod run_jailed;
