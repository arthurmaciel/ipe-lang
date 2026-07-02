//! Phase-1a round-2 gate: `Event.onInput` with a Bool handler must be
//! REJECTED BY skyc with SKY-T0001 — never exit 0 and defer to cargo.
//!
//! ## Root cause (pre-fix)
//!
//! `crates/sky_types/src/constrain.rs` only covered the String-payload event
//! arm under `Some("Ui")`.  `Event.onInput` (canonical qualifier `"Event"` from
//! `import Std.Html.Events as Event`) fell to the flexible `Ty::Var(u32::MAX)`
//! fallback, skyc exited 0, and cargo emitted E0308 (Arc-wraps `Fn(String)`
//! around a Bool closure).
//!
//! ## Fix (round 2)
//!
//! All event arms in `constrain.rs` now cover `Some("Ui" | "Event")`, mirroring
//! `lower.rs`'s `("Ui" | "Event", ...)` qualifier coverage exactly.
//!
//! ## What is tested
//!
//! * **Negative** (`m7_stdui_event_illtyped`): `Event.onInput (\b -> SetChecked b)`
//!   (Bool handler on a String event) → skyc reports SKY-T0001 (type mismatch),
//!   never exits 0.
//!
//! * **Positive** (`m7_stdui_event_oninput`): `Event.onInput (\s -> SetText s)`
//!   (String handler, well-typed) → skyc succeeds (build returns `Ok`).
//!
//! Both tests are pure skyc-pipeline checks (no cargo build / runtime binary
//! required), so they run without `SKY_E2E=1`.  They return early if
//! [`skyc::resolve_runtime`] cannot locate the embedded runtime.

use std::path::{Path, PathBuf};

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Run the skyc pipeline on the named fixture and return the build result.
/// Returns `None` (skip) when the embedded runtime cannot be resolved.
fn run_skyc(fixture: &str, out_suffix: &str) -> Option<Result<(), CliError>> {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        // Runtime not available in this environment — skip.
        return None;
    };
    Some(skyc::build(&entry, &out, &runtime))
}

/// Phase-1a round-2 NEGATIVE gate: `Event.onInput` with a `Bool -> msg`
/// handler must be rejected by skyc with SKY-T0001.
///
/// Pre-fix: `constrain.rs` fell to `Ty::Var(u32::MAX)` for the `"Event"`
/// qualifier → skyc exited 0 → cargo emitted E0308.
/// Post-fix: `Some("Ui" | "Event")` arm unifies `Bool -> msg` with the
/// expected `String -> msg` → SKY-T0001 at the type-checking stage.
#[test]
fn event_oninput_illtyped_bool_handler_is_sky_t0001() {
    let Some(result) = run_skyc("m7_stdui_event_illtyped", "m7_stdui_event_illtyped_emit") else {
        return;
    };

    let got = match &result {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(sky_diagnostics::SKY_T0001),
        "m7_stdui_event_illtyped: expected SKY-T0001 (type mismatch), got: {result:?}",
    );
}

/// Phase-1a round-2 POSITIVE sentinel: `Event.onInput` with a correct
/// `String -> msg` handler must compile (skyc build returns `Ok`).
///
/// This confirms the widened qualifier arm in `constrain.rs` does not
/// break well-typed `Event.onInput` usage.
#[test]
fn event_oninput_correct_handler_compiles() {
    let Some(result) = run_skyc("m7_stdui_event_oninput", "m7_stdui_event_oninput_emit") else {
        return;
    };

    assert!(
        result.is_ok(),
        "m7_stdui_event_oninput: correct Event.onInput handler must compile, got: {:?}",
        result.err()
    );
}
