//! Stdlib import aliases must not silently merge member tables or unlock the
//! must-import gate for an unrelated qualifier — while two bare imports whose
//! last path segment merely coincides stay legitimate.
//!
//! Defects on the `import Ipe.* as Alias` path:
//! - two stdlib imports EXPLICITLY aliased to one name (or an explicit alias
//!   naming a different module's canonical qualifier) used to extend-merge member
//!   tables last-wins with no diagnostic;
//! - marking that raw alias as imported let `import Ipe.Json.Encode as Crypto`
//!   unlock the unrelated `Crypto` gate, resolving `Crypto.hmacSha256` without
//!   ever importing `Ipe.Crypto`.
//!
//! The collision gate is keyed on the EXPLICIT alias only: a bare
//! `import Ipe.Json.Decode` speaks under its canonical `JsonDec`, never the last
//! segment `Decode`, so it does not collide with a bare `import Ipe.Db.Decode`.
//!
//! These use `ipe_canon::canonicalise` directly, so a body may reference only
//! qualifiers the base environment pre-installs (`JsonEnc` via `J`, the gated
//! `Crypto`); compiled-source modules such as `Ipe.Io` are injected by the CLI
//! project pipeline this harness does not run.

use ipe_diagnostics::{Diagnostic, NameError};
use ipe_intern::Interner;

fn canon(src: &str) -> Result<(), Diagnostic> {
    let mut interner = Interner::new();
    let parsed = ipe_parse::parse_module(src, &mut interner)?;
    ipe_canon::canonicalise(&parsed, &mut interner).map(drop)
}

/// Two distinct stdlib modules EXPLICITLY aliased to one name must be rejected
/// with `DuplicateQualifier`, never silently resolve `J.member` last-wins.
#[test]
fn two_stdlib_imports_sharing_an_alias_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Encode as J\n\
               import Ipe.Json.Decode as J\n\n\
               main =\n    J.string \"x\"\n";
    let err = canon(src).expect_err("two imports aliased to `J` must be rejected");
    assert!(
        matches!(
            err,
            Diagnostic::Name {
                msg: NameError::DuplicateQualifier { .. },
                ..
            }
        ),
        "expected DuplicateQualifier, got: {err:?}"
    );
}

/// An explicit alias equal to a DIFFERENT pre-installed canonical qualifier must
/// be rejected rather than extend-merge over that qualifier's members.
#[test]
fn alias_colliding_with_a_different_canonical_qualifier_is_rejected() {
    // `Crypto` is a pre-installed (gated) canonical qualifier; aliasing an
    // unrelated stdlib module to it must not merge into its member table.
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Encode as Crypto\n\n\
               main =\n    Crypto.string \"x\"\n";
    let err = canon(src).expect_err("alias colliding with `Crypto` must be rejected");
    assert!(
        matches!(
            err,
            Diagnostic::Name {
                msg: NameError::DuplicateQualifier { .. },
                ..
            }
        ),
        "expected DuplicateQualifier, got: {err:?}"
    );
}

/// Two BARE imports whose last path segment merely coincides (`Ipe.Json.Decode`
/// and `Ipe.Db.Decode`, both segment `Decode`) must NOT be treated as an alias
/// collision: each speaks under its own canonical qualifier (`JsonDec`,
/// `Db.Decode`), so both are accepted. Regression guard for the over-broad gate
/// that rejected the generic-optional-decoder golden.
#[test]
fn two_bare_imports_sharing_a_last_segment_are_accepted() {
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Decode\n\
               import Ipe.Db.Decode\n\n\
               main =\n    JsonDec.string\n";
    let r = canon(src);
    assert!(
        r.is_ok(),
        "bare imports sharing only a last segment must not collide, got: {r:?}"
    );
}

/// Aliasing an unrelated import to a non-canonical name must NOT unlock the
/// `Crypto` gate: a `Crypto.sha256` use with no `import Ipe.Crypto` still raises
/// the must-import diagnostic. `J` aliases `Ipe.Json.Encode` (canonical
/// `JsonEnc`), so the alias plainly never keyed the `Crypto` gate.
#[test]
fn unrelated_alias_does_not_unlock_the_crypto_gate() {
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Encode as J\n\n\
               main =\n    Crypto.sha256 \"m\"\n";
    let err = canon(src).expect_err("un-imported `Crypto` use must be gated");
    assert!(
        matches!(
            err,
            Diagnostic::Name {
                msg: NameError::StdlibImportRequired { .. },
                ..
            }
        ),
        "expected StdlibImportRequired for un-imported Crypto, got: {err:?}"
    );
}

/// Positive control: a legitimate explicit stdlib alias still resolves its
/// members.
#[test]
fn legitimate_stdlib_alias_still_resolves() {
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Encode as J\n\n\
               main =\n    J.encode 0 (J.string \"x\")\n";
    let r = canon(src);
    assert!(
        r.is_ok(),
        "a legitimate `import Ipe.Json.Encode as J` must still resolve J.string, got: {r:?}"
    );
}
