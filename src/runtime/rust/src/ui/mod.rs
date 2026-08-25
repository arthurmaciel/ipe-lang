//! `Ipe.Ui` shared element tree — the general UI abstraction that Ipe.Web,
//! Ipe.Tui, and Ipe.WebView each render to their own target. The codegen maps the
//! Ipê `Ipe.Ui.*` types onto these via `runtimeOpaqueTypes` (qualified path
//! `ipe_runtime::ui::*`), so this module is intentionally NOT glob-re-exported at
//! the crate root (its `Attribute` would collide with `html::Attribute`).

pub mod element;
pub use element::*;

pub mod render;

/// Kernel-dispatch helpers — called directly by ipe-emitted code.  Each
/// function corresponds to a `KernelFn` variant in `ipe_ir` and is named
/// with the `naming.rs` trailing-underscore convention.
pub mod helpers;

/// `Ipe.Ui.Input` kernel helpers — typed form controls.
pub mod input;

/// `Ipe.Ui.Lazy` kernel helpers — eager evaluation in v1.
pub mod lazy;

/// `Ipe.Ui.Keyed` kernel helpers — ipe-key diff identity (key discarded in v1).
pub mod keyed;

/// `Ui.widget` server-driven custom-element boundary — the opaque handle, the
/// reserved constructor, and the fail-closed down-encode / up-decode emission.
pub mod widget;
