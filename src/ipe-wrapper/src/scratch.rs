//! The jail's writable scratch directory, re-exported from the one shared primitive.
//!
//! [`ipe_sandbox::scratch`] creates it private (0700, owned by the effective
//! user, under a verified base) and removes it on drop.

pub use ipe_sandbox::scratch::ScratchDir;
