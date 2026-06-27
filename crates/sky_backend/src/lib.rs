#![forbid(unsafe_code)]
//! The backend boundary for the Sky compiler. A backend consumes the
//! backend-agnostic typed [`sky_ir::Program`] and produces an
//! [`EmittedProject`] — an in-memory project tree ready to be written to disk.
//!
//! This crate is the *only* contract a backend sees. It deliberately depends on
//! nothing from the frontend (parser / canonicaliser / type-checker / lowerer):
//! the only inputs are [`sky_ir`] and [`sky_diagnostics`]. A backend that names
//! a frontend crate breaks the architecture's acyclic boundary.

use std::collections::BTreeMap;

use sky_diagnostics::DResult;
use sky_ir::Program;

/// An emitted project, fully materialised in memory.
///
/// `files` maps a project-relative path (forward-slash separated, e.g.
/// `"src/main.rs"`) to its file contents. A [`BTreeMap`] is used so iteration
/// order is deterministic — emission must never depend on hash ordering.
/// `cargo_toml` is the manifest emitted at the project root.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EmittedProject {
    /// Project-relative path -> file contents. Deterministic iteration order.
    pub files: BTreeMap<String, String>,
    /// The contents of the project-root `Cargo.toml`.
    pub cargo_toml: String,
}

/// A code-generation backend: turns a typed IR [`Program`] into an
/// [`EmittedProject`].
///
/// Implementations must be pure functions of their input `program` and must not
/// observe non-deterministic ordering. Failures are reported as a typed
/// [`sky_diagnostics::Diagnostic`] — never a panic or a `String` error.
pub trait Backend {
    /// A stable, human-readable identifier for this backend (e.g. `"rust"`).
    fn name(&self) -> &'static str;

    /// Emit the project for `program`, or a typed diagnostic on failure.
    ///
    /// # Errors
    ///
    /// Returns a [`sky_diagnostics::Diagnostic`] when the program cannot be
    /// emitted (for example, an internal invariant violation surfaces as
    /// [`sky_diagnostics::Diagnostic::CompilerBug`]).
    fn emit(&self, program: &Program) -> DResult<EmittedProject>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopBackend;

    impl Backend for NoopBackend {
        fn name(&self) -> &'static str {
            "noop"
        }

        fn emit(&self, _program: &Program) -> DResult<EmittedProject> {
            Ok(EmittedProject::default())
        }
    }

    #[test]
    fn noop_backend_name_and_empty_emit() {
        let backend = NoopBackend;
        assert_eq!(backend.name(), "noop");

        let program = Program {
            modules: Vec::new(),
        };
        let emitted = backend.emit(&program);
        assert_eq!(emitted, Ok(EmittedProject::default()));
        assert!(matches!(&emitted, Ok(project) if project.files.is_empty()));
        assert!(matches!(emitted, Ok(project) if project.cargo_toml.is_empty()));
    }
}
