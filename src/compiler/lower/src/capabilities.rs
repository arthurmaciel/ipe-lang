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
        let program: Program = crate::lower(&m, &types, &mut i, "", "").ok()?;
        Some(program_capabilities(&program))
    }

    #[test]
    fn pure_program_has_no_capabilities() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.String\nmain : Task ()\nmain =\n    Io.println (String.toUpper \"hi\")\n",
        );
        assert_eq!(caps, Some(std::collections::BTreeSet::new()));
    }

    #[test]
    fn http_program_infers_network() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Http\nimport Ipe.Io\nimport Ipe.Task\nmain : Task ()\nmain =\n    Task.andThen (\\_ -> Io.println \"done\") (Http.get \"http://example.com\")\n",
        );
        assert_eq!(
            caps,
            Some(std::collections::BTreeSet::from([Capability::Network]))
        );
    }

    /// An EXPOSED library module with no `main`: its exposed functions are the
    /// reachability roots a downstream consumer can call, so their capabilities
    /// must be inferred even though nothing local calls them. The dead-function
    /// prune must never drop an exposed API's effect (fail-closed: no `main` ⇒
    /// every function is a root). This is the honesty invariant the package
    /// audit's sibling-capability check relies on.
    #[test]
    fn exposed_library_module_without_main_infers_network() {
        let caps = caps_of(
            "module Extra exposing (fetch)\nimport Ipe.Http\nimport Ipe.Io\nimport Ipe.Task\nfetch : Task ()\nfetch =\n    Task.andThen (\\_ -> Io.println \"done\") (Http.get \"http://example.com\")\n",
        );
        assert_eq!(
            caps,
            Some(std::collections::BTreeSet::from([Capability::Network]))
        );
    }

    /// The `unsafe` capability is import-derived, disclosed IFF the program
    /// imports an `Ipe.<M>.Unsafe` submodule. This is the load-bearing half of
    /// the bidirectional partition: importing the escape-hatch submodule MUST
    /// scan the `unsafe` capability. Fails-before if the scan omits the
    /// `program.imports_unsafe_submodule` insert.
    #[test]
    fn importing_an_unsafe_submodule_discloses_unsafe() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.Db.Unsafe\nmain : Task ()\nmain =\n    Io.println \"hi\"\n",
        );
        assert!(
            caps.as_ref()
                .is_some_and(|c| c.contains(&Capability::Unsafe)),
            "a program importing an `Ipe.<M>.Unsafe` submodule must disclose the `unsafe` capability, got {caps:?}"
        );
    }

    /// A REAL relocated member's home: importing the shipped `Ipe.Html.Unsafe`
    /// submodule — the raw-HTML XSS escape hatch relocated out of `Ipe.Html` —
    /// discloses `unsafe`. Exercises the import-derived `unsafe` scan on an actual
    /// shipped `.Unsafe` module rather than a bare name, proving the disclosure
    /// fires for the raw-HTML boundary specifically. (Member resolution + the
    /// behaviour-identical `html_raw_node_` output is proved end-to-end in the
    /// negative-suite full-pipeline test, which injects the compiled stdlib.)
    #[test]
    fn importing_ipe_html_unsafe_discloses_unsafe() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.Html.Unsafe\nmain : Task ()\nmain =\n    Io.println \"hi\"\n",
        );
        assert!(
            caps.as_ref()
                .is_some_and(|c| c.contains(&Capability::Unsafe)),
            "a program importing `Ipe.Html.Unsafe` must disclose the `unsafe` capability, got {caps:?}"
        );
    }

    /// A REAL relocated member's home: importing the shipped `Ipe.Web.Head.Unsafe`
    /// submodule — the verbatim JSON-LD `<script>` hatch relocated out of
    /// `Ipe.Web.Head` — discloses `unsafe`. Proves the disclosure fires for the
    /// raw-script boundary specifically.
    #[test]
    fn importing_ipe_web_head_unsafe_discloses_unsafe() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.Web.Head.Unsafe\nmain : Task ()\nmain =\n    Io.println \"hi\"\n",
        );
        assert!(
            caps.as_ref()
                .is_some_and(|c| c.contains(&Capability::Unsafe)),
            "a program importing `Ipe.Web.Head.Unsafe` must disclose the `unsafe` capability, got {caps:?}"
        );
    }

    /// A REAL relocated member's home: importing the shipped `Ipe.Secret.Unsafe`
    /// submodule — the raw secret-reveal hatch relocated out of `Ipe.Secret` —
    /// discloses `unsafe`. Proves the disclosure fires for the secret-leak
    /// boundary specifically. The scoped `Secret.use` counterpart stays on the
    /// native `Ipe.Secret` surface and discloses nothing (covered by the
    /// capability-neutral negative-suite test).
    #[test]
    fn importing_ipe_secret_unsafe_discloses_unsafe() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.Secret.Unsafe\nmain : Task ()\nmain =\n    Io.println \"hi\"\n",
        );
        assert!(
            caps.as_ref()
                .is_some_and(|c| c.contains(&Capability::Unsafe)),
            "a program importing `Ipe.Secret.Unsafe` must disclose the `unsafe` capability, got {caps:?}"
        );
    }

    /// The scoped `Secret.use` is CAPABILITY-NEUTRAL: reaching it off a plain
    /// `import Ipe.Secret` (no `.Unsafe` submodule) discloses nothing on the
    /// `unsafe` axis. This is the whole point of the scoped API — the common
    /// case stays off the `unsafe` axis, so only the blunt `unsafeReveal`
    /// discloses.
    #[test]
    fn using_secret_use_off_plain_secret_discloses_no_unsafe() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.Secret\nimport Ipe.System as System\nmain : Task ()\nmain =\n    Io.println (Secret.use (Secret.fromString (System.getenvOr \"K\" \"sk\")) (\\p -> p))\n",
        );
        assert!(
            caps.as_ref()
                .is_some_and(|c| !c.contains(&Capability::Unsafe)),
            "a program that reaches `Secret.use` off a plain `import Ipe.Secret` must NOT disclose `unsafe` — the scoped consume is capability-neutral, got {caps:?}"
        );
    }

    /// The other half of the partition: a program that imports NO `.Unsafe`
    /// submodule discloses nothing on the `unsafe` axis. Guards against the scan
    /// firing `unsafe` unconditionally — the default path must be untouched.
    #[test]
    fn a_program_without_an_unsafe_import_discloses_no_unsafe() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.String\nmain : Task ()\nmain =\n    Io.println (String.toUpper \"hi\")\n",
        );
        assert_eq!(
            caps,
            Some(std::collections::BTreeSet::new()),
            "a program that imports no `.Unsafe` submodule must disclose no capability, least of all `unsafe`"
        );
    }

    /// The `Unsafe` segment is matched at the END of a dotted `Ipe.<M>.Unsafe`
    /// path, not anywhere: an unrelated stdlib import whose name merely contains
    /// no trailing `Unsafe` segment does not trip the disclosure. A bare
    /// `import Ipe.Html` (a safe surface) discloses nothing.
    #[test]
    fn a_plain_stdlib_import_is_not_mistaken_for_unsafe() {
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.Io\nimport Ipe.Html\nmain : Task ()\nmain =\n    Io.println \"hi\"\n",
        );
        assert!(
            caps.as_ref()
                .is_some_and(|c| !c.contains(&Capability::Unsafe)),
            "a plain `import Ipe.Html` must not disclose `unsafe`, got {caps:?}"
        );
    }

    #[test]
    fn file_and_env_program_infers_both() {
        // `main` reaches both a filesystem kernel (`File.writeFile`) and an env
        // kernel (`System.getenvOr`), so the scan must infer both capabilities.
        // The `Path` arg is built with the `path "…"` compile-time literal, which
        // needs no `Ipe.Path` injection — so `main` is a plain zero-argument
        // `Task ()` entry that reaches both kernels directly.
        let caps = caps_of(
            "module Main exposing (main)\nimport Ipe.File\nimport Ipe.System\nmain : Task ()\nmain =\n    File.writeFile (path \"/tmp/ipe-cap-probe\") (System.getenvOr \"HOME\" \"/\")\n",
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
