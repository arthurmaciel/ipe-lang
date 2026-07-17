//! `Ipe.Secret` negative gates: every plausible
//! accidental-stringification / accidental-leak path that is NOT redacted
//! must instead be REJECTED at `skyc` compile time — never accepted and left
//! to misbehave (or leak) at runtime, and never deferred to a `cargo`
//! failure (the SEAL). Companion to `crates/skyc/tests/golden_secret.rs`
//! (the positive goldens) and `crates/skyc/tests/model_admissibility.rs`
//! (`live_model_with_secret_field_is_rejected`, the Model-gate IPE-L0120
//! case).
//!
//! Compile-only: these fixtures never run (there is nothing to execute — the
//! program is ill-typed), so there is no oracle / `IPE_E2E` gate here.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Build the named golden fixture and assert it surfaces exactly `expected` as
/// a pipeline diagnostic — never a panic, never a silent accept.
fn assert_gate(fixture: &str, out_suffix: &str, expected: ipe_diagnostics::Code) {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    let got = match &built {
        Err(ipe::CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "fixture {fixture}: expected {expected:?}, got build result {built:?}"
    );
}

/// `"using key " ++ Secret.fromString "sk_live_abc123"` — `Secret` does not
/// satisfy the `++` append obligation (`String` / `List _` only; see
/// `BinopClass::Append`'s doc comment in `ipe_types::constrain`). Both
/// operands are `eq`'d to one shared appendable-bounded super-var; the
/// concrete `String` literal on the left pins that var to `String` FIRST, so
/// the `Secret` right operand surfaces as an ordinary `IPE-T0001` `Con`
/// mismatch (`String` vs `Secret`) rather than the "no operand pins the
/// obligation" `IPE-T0014` shape — either way, a compile-time rejection,
/// never silently accepted and never deferred to a runtime concern.
#[test]
fn secret_concat_is_rejected() {
    assert_gate(
        "secret_concat_rejected",
        "secret_concat_rejected_emit",
        ipe_diagnostics::IPE_T0001,
    );
}
