//! `ipe_path_core` — the single source of truth for Ipê's lexical path
//! validation.
//!
//! Both the runtime `Path.fromString` seal (`ipe_runtime::path`) and the
//! compiler's `path "…"` literal gate (`ipe_diagnostics::path_check`) validate
//! the SAME way, so the algorithm lives ONCE and both consumers use it. Neither
//! keeps its own copy. The crate is dependency-free (std only) so the compiler
//! can validate a literal without pulling in the runtime's heavy optional
//! dependencies (tokio, serde, sqlx, …).
//!
//! # One file, two consumers, no drift
//!
//! The algorithm's SOURCE lives in the runtime's own tree at
//! `src/runtime/rust/src/path_core.rs`, so it vendors automatically with the
//! runtime module (`mod ipe_runtime`) that emitted apps source-copy — a
//! standalone `extern crate ipe_path_core` would not survive that copy. This
//! crate `include!`s that same file, so `ipe_diagnostics` still consumes the
//! ONE source of truth: there is a single definition of `validate` / `clean_with`
//! / `escapes_root` / `volume_name_len` / `has_disguised_dotdot` / `has_nul`,
//! and the runtime seal and the compile-time gate cannot drift.
//!
//! # Two entry points, one algorithm
//!
//! * [`validate`] — the COMPILE-TIME gate. The compiler does not know the final
//!   target OS, so it rejects a path that would traverse under EITHER separator
//!   regime (Unix `/` or Windows `\`/`/`). This is deliberately stricter than
//!   the runtime's target-specific check: a compile-time reject can only ever be
//!   a superset of what the runtime rejects, so nothing the runtime would refuse
//!   is ever emitted as a validated literal.
//! * [`clean_with`] / [`escapes_root`] / [`has_disguised_dotdot`] / [`has_nul`]
//!   — the target-specific primitives the runtime seal drives with its own
//!   host separator regime (`clean_with(s, cfg!(windows))`), keeping the runtime
//!   behaviour byte-identical per platform.

// Splice in the ONE source of truth, which physically lives in the runtime's
// source tree so it vendors with `mod ipe_runtime` into every emitted app.
include!("../../../runtime/rust/src/path_core.rs");
