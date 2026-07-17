//! Negative gates for the SEAL fix
//! (`docs/adr/0017-error-payload-nominal-identity.md`): a bare record literal
//! must NOT construct the nominal error-payload types `PanicInfo` / `TypeInfo` /
//! `ErrorInfo`. Under structural registration each fixture would be an
//! exit-0-then-cargo-fail (well-typed in skyc, but the emitted Rust passes a
//! project-local synthesized record struct where the runtime's concrete
//! `SkyPanicInfo`/`SkyTypeInfo`/`SkyErrorInfo` is required — E0308). They
//! are ordinary IPE-T0001 type mismatches at `skyc` time.
//!
//! Companion positive golden: `crates/skyc/tests/
//! golden_error_nominal_payload.rs` (field access + nominal-annotated helpers
//! stay green).
//!
//! Compile-only: these fixtures never run (the program is ill-typed), so
//! there is no oracle / `IPE_E2E` gate here.

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
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    let got = match &built {
        Err(skyc::CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "fixture {fixture}: expected {expected:?}, got build result {built:?}"
    );
}

/// `FfiPanic { message = "boom", stack = [] }` — the RED repro from the filed
/// backlog row, verbatim.
#[test]
fn panic_info_record_literal_is_rejected() {
    assert_gate(
        "error_record_literal_panicinfo",
        "error_record_literal_panicinfo_emit",
        ipe_diagnostics::IPE_T0001,
    );
}

/// `TypeMismatch { expected = "Int", actual = "String" }`.
#[test]
fn type_info_record_literal_is_rejected() {
    assert_gate(
        "error_record_literal_typeinfo",
        "error_record_literal_typeinfo_emit",
        ipe_diagnostics::IPE_T0001,
    );
}

/// `Error Io { message = "disk full", details = Nothing }` — direct `Error`
/// construction with a record-literal `ErrorInfo`; the sanctioned path is the
/// `Error.io`/… smart constructors.
#[test]
fn error_info_record_literal_is_rejected() {
    assert_gate(
        "error_record_literal_errorinfo",
        "error_record_literal_errorinfo_emit",
        ipe_diagnostics::IPE_T0001,
    );
}
