//! Sky.Tui — terminal (ANSI cell) backend for the Rust target.
//!
//! TEA-shaped (`Std.Tui.app cfg`): the same `view : Model -> Element msg` that
//! Sky.Live / Sky.Webview render, lowered to ANSI cells. See
//! `docs/superpowers/specs/2026-06-12-s4-sky-tui-design.md`.

pub mod app;
pub mod cell;
pub mod diff; // accessed qualified (tui::diff::diff) — `diff` collides with live's
pub mod focus; // input registry + focusable model + key editing
pub mod key;
pub mod layout; // structured Element → ANSI cells (Go-parity; replaces the CSS-reparsing render.rs)
pub use app::{tui_app, tui_app_ui};
pub use cell::*;
