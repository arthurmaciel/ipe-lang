//! `Stream.stream` handler captures are classified before the emit re-wrap clones them.
//!
//! The `StreamStream` emit arm rebuilds the handler per call inside a `move`
//! closure and shadows every free local with `.clone()`. The lowerer classifies
//! each capture as a `Copy` leaf, a `Clone` carrier, or a promotable fn binder
//! (carried as the `Arc` fn carrier); anything else is refused with IPE-L0126 at
//! ipe time, so the clone prologue never meets a `Box<dyn Fn>` (E0599).
//!
//! ```text
//! IPE_E2E=1 cargo nextest run -p ipe --test g_issues golden_stream_handler_capture_seal
//! ```

use std::path::{Path, PathBuf};

use ipe::CliError;

fn fixture_entry(fixture: &str) -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined)
        .unwrap_or(joined)
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.ipe")
}

/// A handler capturing a fn parameter passes ipe and the emitted crate cargo-builds.
#[test]
fn fn_param_capture_is_arc_carried_and_builds() {
    let entry = fixture_entry("stream_fn_param_capture");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("stream_fn_param_capture");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // runtime unavailable — skip silently rather than fail
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "fn-param capture of a Stream.stream handler must pass ipe: {:?}",
        built.err()
    );

    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    // Build-only: the fixture is a listening server, so it cannot run-to-exit.
    let built_bin = e2e_support::build_rust_binary("stream_fn_param_capture", &out);
    assert!(
        built_bin.is_ok(),
        "emitted crate must cargo-build (a bare `Box<dyn Fn>` capture has no `clone`): {}",
        built_bin.as_ref().err().map_or("", String::as_str)
    );
}

/// A handler capturing a destructure-bound fn is refused at ipe time with IPE-L0126.
#[test]
fn destructured_fn_capture_is_refused() {
    let entry = fixture_entry("stream_destructured_fn_capture");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("stream_destructured_fn_capture");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // runtime unavailable — skip silently rather than fail
    };
    let built = ipe::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(ipe_diagnostics::IPE_L0126),
        "a non-Clone Stream.stream capture must fail closed at ipe time, got {built:?}"
    );
}
