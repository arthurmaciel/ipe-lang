//! The `ipe` driver: typed errors, the build/compile pipeline, and the command
//! dispatch. Split out of the crate-root `lib.rs`; every item is re-exported at
//! the crate root so existing `crate::…` paths resolve unchanged.

mod build_pipeline;
mod commands;
mod commands_pkg;
mod driver_error;
#[cfg(test)]
mod tests;

pub use build_pipeline::*;
pub use commands::*;
pub use commands_pkg::*;
pub use driver_error::*;
