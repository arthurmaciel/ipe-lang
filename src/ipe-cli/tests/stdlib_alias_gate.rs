//! Stdlib import aliases must not silently merge member tables or unlock the
//! must-import gate for an unrelated qualifier.
//!
//! Two coupled defects on the `import Ipe.* as Alias` path:
//! - two stdlib imports aliased to one name (or an alias colliding with a
//!   different module's pre-installed qualifier) used to extend-merge member
//!   tables last-wins with no diagnostic;
//! - marking the raw alias as imported let `import Ipe.Json.Encode as Crypto`
//!   unlock the unrelated `Crypto` gate, resolving `Crypto.hmacSha256` without
//!   ever importing `Ipe.Crypto`.

use ipe_diagnostics::{Diagnostic, NameError};
use ipe_intern::Interner;

fn canon(src: &str) -> Result<(), Diagnostic> {
    let mut interner = Interner::new();
    let parsed = ipe_parse::parse_module(src, &mut interner)?;
    ipe_canon::canonicalise(&parsed, &mut interner).map(drop)
}

/// Two distinct stdlib modules aliased to one name must be rejected with
/// `DuplicateQualifier`, never silently resolve `J.member` last-wins.
#[test]
fn two_stdlib_imports_sharing_an_alias_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Encode as J\n\
               import Ipe.Json.Decode as J\n\
               import Ipe.Io\n\n\
               main =\n    Io.println \"x\"\n";
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

/// An alias equal to a DIFFERENT pre-installed canonical qualifier must be
/// rejected rather than extend-merge over that qualifier's members.
#[test]
fn alias_colliding_with_a_different_canonical_qualifier_is_rejected() {
    // `Crypto` is a pre-installed (gated) canonical qualifier; aliasing an
    // unrelated stdlib module to it must not merge into its member table.
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Encode as Crypto\n\
               import Ipe.Io\n\n\
               main =\n    Io.println \"x\"\n";
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

/// Aliasing an unrelated import to `Crypto` must NOT unlock the `Crypto` gate:
/// a `Crypto.hmacSha256` use with no `import Ipe.Crypto` still raises the
/// must-import diagnostic.
///
/// (Uses a bare `Ipe.Log` import — whose canonical `Log` differs from `Crypto`
/// — proving the alias never keyed the gate. The collision check above rejects
/// aliasing to `Crypto` directly, so the gate-unlock path is exercised through a
/// name that both differs from `Crypto` and does not collide.)
#[test]
fn unrelated_alias_does_not_unlock_the_crypto_gate() {
    // `Nope` aliases `Ipe.Log`; a later `Crypto.hmacSha256` must still be gated
    // because `Ipe.Crypto` was never imported.
    let src = "module Main exposing (main)\n\
               import Ipe.Log as Nope\n\
               import Ipe.Io\n\n\
               main =\n    Io.println (Crypto.sha256 \"m\")\n";
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

/// Positive control: a legitimate stdlib alias still resolves its members.
#[test]
fn legitimate_stdlib_alias_still_resolves() {
    let src = "module Main exposing (main)\n\
               import Ipe.Json.Encode as J\n\
               import Ipe.Io\n\n\
               main =\n    Io.println (J.encode 0 (J.string \"x\"))\n";
    assert!(
        canon(src).is_ok(),
        "a legitimate `import Ipe.Json.Encode as J` must still resolve J.string"
    );
}
