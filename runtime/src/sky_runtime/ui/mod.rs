//! `Std.Ui` shared element tree — the general UI abstraction that Sky.Live,
//! Sky.Tui, and Sky.Webview each render to their own target. The codegen maps the
//! Sky `Std.Ui.*` types onto these via `runtimeOpaqueTypes` (qualified path
//! `sky_runtime::ui::*`), so this module is intentionally NOT glob-re-exported at
//! the crate root (its `Attribute` would collide with `html::Attribute`).

pub mod element;
pub use element::*;

pub mod render;

/// Kernel-dispatch helpers — called directly by skyc-emitted code.  Each
/// function corresponds to a `KernelFn` variant in `sky_ir` and is named
/// with the `naming.rs` trailing-underscore convention.
pub mod helpers;

/// `Std.Ui.Input` kernel helpers — typed form controls.
pub mod input;

/// `Std.Ui.Lazy` kernel helpers — eager evaluation in v1.
pub mod lazy;

/// `Std.Ui.Keyed` kernel helpers — sky-key diff identity (key discarded in v1).
pub mod keyed;
