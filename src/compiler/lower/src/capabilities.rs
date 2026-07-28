//! Whole-program capability inference.
//!
//! [`program_capabilities`] computes, from a lowered [`ipe_ir::Program`] alone,
//! the exact set of security capabilities it exercises: the union over the
//! program's transitively-reachable kernels of [`ipe_ir::KernelFn::capability`],
//! plus [`ipe_ir::Capability::NativeFfi`] when the program crosses into `Rust.`
//! code. The set is generated, not declared, and cannot drift — a kernel with no
//! capability classification is a compile error in `ipe_kernels`.

use std::collections::BTreeSet;

use ipe_ir::{Capability, Program};

/// The security capabilities a program exercises, inferred from its reachable
/// kernels and any `Rust.` crossing.
///
/// Deterministic: the [`BTreeSet`] orders capabilities by their declared
/// [`Capability`] order, so the reported set is reproducible.
#[must_use]
pub fn program_capabilities(program: &Program) -> BTreeSet<Capability> {
    crate::lower::program_capabilities_scan(program)
}

#[cfg(test)]
mod tests {
    use ipe_intern::Interner;
    use ipe_ir::{Capability, Program};

    use super::program_capabilities;

    /// Lower a free-standing single-module program, or `None` if any pipeline
    /// stage rejects it (the caller's assertions then fail, per the no-panic
    /// gate).
    fn caps_of(source: &str) -> Option<std::collections::BTreeSet<Capability>> {
        let mut i = Interner::new();
        let src = ipe_parse::parse_module(source, &mut i).ok()?;
        let m = ipe_canon::canonicalise(&src, &mut i).ok()?;
        let types = ipe_types::infer(&m, &mut i).ok()?;
        let program: Program = crate::lower(&m, &types, &mut i).ok()?;
        Some(program_capabilities(&program))
    }

    #[test]
    fn pure_program_has_no_capabilities() {
        let caps = caps_of(
            "module Main exposing (main)\nmain : Task ()\nmain =\n    Io.println (String.toUpper \"hi\")\n",
        );
        assert_eq!(caps, Some(std::collections::BTreeSet::new()));
    }

    #[test]
    fn http_program_infers_network() {
        let caps = caps_of(
            "module Main exposing (main)\nmain : Task ()\nmain =\n    Task.andThen (\\_ -> Io.println \"done\") (Http.get \"http://example.com\")\n",
        );
        assert_eq!(
            caps,
            Some(std::collections::BTreeSet::from([Capability::Network]))
        );
    }

    #[test]
    fn file_and_env_program_infers_both() {
        let caps = caps_of(
            "module Main exposing (main)\nmain : Task ()\nmain =\n    Task.andThen (\\_ -> File.writeFile \"/tmp/x\" (System.getenvOr \"HOME\" \"/\")) (Io.println \"go\")\n",
        );
        assert_eq!(
            caps,
            Some(std::collections::BTreeSet::from([
                Capability::Filesystem,
                Capability::Env,
            ]))
        );
    }
}
