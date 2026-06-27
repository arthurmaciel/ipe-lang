#![forbid(unsafe_code)]
//! The backend boundary for the Sky compiler. A backend consumes the
//! backend-agnostic typed [`sky_ir::Program`] and produces an
//! [`EmittedProject`] — an in-memory project tree ready to be written to disk.
//!
//! This crate is the *only* contract a backend sees. It deliberately depends on
//! nothing from the frontend (parser / canonicaliser / type-checker / lowerer):
//! the only inputs are [`sky_ir`] and [`sky_diagnostics`]. A backend that names
//! a frontend crate breaks the architecture's acyclic boundary.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::path::Path;

use sky_diagnostics::{DResult, Diagnostic};
use sky_ir::Program;

/// A validated project-relative path — the key type for [`EmittedProject::files`].
///
/// This newtype is the trust boundary between in-memory emission and the on-disk
/// materialiser. Parse-don't-validate: [`RelPath::new`] is the *only* way to
/// construct one, and it rejects any path that could escape the output directory
/// when fed to `out_dir.join(rel)` — an absolute path, a leading `/` or `\`, a
/// Windows drive-letter prefix (`C:`), or any `..` parent-directory component.
/// Once a `RelPath` exists the writer trusts it unconditionally: a bare `String`
/// key with an absolute or `..`-bearing value can no longer reach `fs::write`,
/// because `EmittedProject::files` cannot hold one.
///
/// The inner string is forward-slash separated (e.g. `"src/main.rs"`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    /// Construct a validated relative path, or a diagnostic if `path` could
    /// escape the output directory.
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] (a backend emitting an unsafe key is
    /// an internal invariant violation, not a property of the user's program)
    /// when `path` is empty, absolute, root-/drive-rooted, or contains a `..`
    /// component. The offending path travels in `detail` — the only free-form
    /// channel — so the rejection is greppable without leaking a stringly error.
    pub fn new(path: impl Into<String>) -> DResult<Self> {
        let path = path.into();
        Self::validate(&path)?;
        Ok(Self(path))
    }

    /// Borrow the validated path as a forward-slash string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(path: &str) -> DResult<()> {
        let reject = |reason: &str| -> DResult<()> {
            Err(Diagnostic::CompilerBug {
                where_: "backend.unsafe_rel_path",
                detail: format!("refusing unsafe emitted file key {path:?}: {reason}"),
            })
        };

        if path.is_empty() {
            return reject("empty path");
        }
        // A leading separator (either flavour) makes the path root-relative —
        // absolute on Unix, drive-root-relative on Windows.
        if path.starts_with('/') || path.starts_with('\\') {
            return reject("leading path separator (absolute / root-relative)");
        }
        // Windows drive-letter prefix, e.g. `C:` or `C:\\foo`. Checked
        // explicitly because `Path::is_absolute` does not flag it on a Unix host.
        // Byte iteration avoids raw indexing.
        let mut bytes = path.bytes();
        if let (Some(first), Some(second)) = (bytes.next(), bytes.next())
            && first.is_ascii_alphabetic()
            && second == b':'
        {
            return reject("Windows drive-letter prefix");
        }
        // Platform's own absolute check, for any root form the above misses.
        if Path::new(path).is_absolute() {
            return reject("absolute path");
        }
        // No `..` component may appear. Split on BOTH separators so a
        // Windows-style `..\\` is caught even on a Unix host, where `Path`'s
        // component iterator would not treat `\\` as a separator.
        if path.split(['/', '\\']).any(|comp| comp == "..") {
            return reject("parent-directory (`..`) component");
        }
        Ok(())
    }
}

/// Borrowing as `str` lets `files.get("src/main.rs")` look up by literal without
/// allocating a `RelPath`. `RelPath`'s derived `Ord` compares the inner string,
/// so the borrowed `str` ordering is consistent — the `BTreeMap` invariant the
/// `Borrow`/`Ord` contract requires.
impl Borrow<str> for RelPath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// An emitted project, fully materialised in memory.
///
/// `files` maps a validated project-relative [`RelPath`] to its file contents. A
/// [`BTreeMap`] is used so iteration order is deterministic — emission must never
/// depend on hash ordering. The [`RelPath`] key type enforces that no key can
/// escape the output directory at the disk boundary (see [`RelPath`]).
/// `cargo_toml` is the manifest emitted at the project root.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EmittedProject {
    /// Validated relative path -> file contents. Deterministic iteration order.
    pub files: BTreeMap<RelPath, String>,
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
    fn relpath_accepts_ordinary_relative_paths() -> DResult<()> {
        for ok in [
            "src/main.rs",
            "Cargo.toml",
            "a/b/c.rs",
            "src/sky_runtime/mod.rs",
        ] {
            let rel = RelPath::new(ok)?;
            assert_eq!(rel.as_str(), ok);
        }
        Ok(())
    }

    #[test]
    fn relpath_rejects_escaping_keys() {
        // Absolute, root-/drive-rooted, parent-dir, and empty keys are the
        // path-traversal / absolute-write vectors the newtype must close.
        for bad in [
            "",
            "/etc/passwd",
            "/abs.rs",
            "\\windows\\system32",
            "C:/Windows/System32",
            "c:relative",
            "../escape.rs",
            "src/../../escape.rs",
            "a/b/../../../etc/passwd",
            "..",
            "foo/..\\bar",
        ] {
            assert!(
                matches!(RelPath::new(bad), Err(Diagnostic::CompilerBug { .. })),
                "RelPath::new({bad:?}) should be rejected as unsafe"
            );
        }
    }

    #[test]
    fn relpath_keyed_map_looks_up_by_str() -> DResult<()> {
        let mut files: BTreeMap<RelPath, String> = BTreeMap::new();
        files.insert(RelPath::new("src/main.rs")?, "fn main() {}".to_owned());
        assert_eq!(
            files.get("src/main.rs").map(String::as_str),
            Some("fn main() {}")
        );
        assert_eq!(files.get("nope.rs"), None);
        Ok(())
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
