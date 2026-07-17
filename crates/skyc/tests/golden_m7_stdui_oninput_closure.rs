//! Regression lock: `Arc<dyn Fn(String) -> M + Send + Sync>` bound.
//!
//! A CLOSURE (lambda, not a top-level fn item) passed to `Ui.onInput` must
//! compile, build, and run.  This locks the emitter's T6 Arc-wrap path for
//! String-carrying events: the emitted code is
//!
//! ```rust
//! sky_runtime::ui::helpers::ui_on_input_(
//!     ::std::sync::Arc::new(move |_x| ({closure})(_x))
//! )
//! ```
//!
//! where `{closure}` is the emitted lambda.  Without `Arc::new(move …)` the
//! closure does not satisfy `Arc<dyn Fn(String) -> M + Send + Sync + 'static>`
//! and cargo rejects it with E0308.
//!
//! Without the fix, fn-item handlers worked (captured no state, auto-coerced to fn ptr);
//! closure handlers exposed the bare `Fn(String) -> M` → missing `Send + Sync`
//! bounds (or the Arc wrap was absent entirely).
//!
//! ## Oracle provenance
//!
//! `oracle_divergence = true`: the Go reference compiler (`sky dev`) does not
//! expose `Html.htmlRender` and exits 1 on this source.  `expected_go.txt` holds
//! skyc's own correct output — the Rust-backend HTML with the input event wired.
//!
//! ## What is tested
//!
//! * `KernelFn::UiOnInput` emits `ui_on_input_(Arc::new(move |_x| f(_x)))`
//!   where `f` is a closure expression (not a top-level fn name).
//! * The emitted Rust project builds without `E0308` or missing-trait-bound errors.
//! * The rendered HTML contains `data-sky-on="input"` (event wired) and the
//!   text node `input here`.
//! * The binary exits 0.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m7_stdui_oninput_closure
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile / build / run `tests/golden/m7_stdui_oninput_closure/Main.sky` and
/// return the golden directory together with the run outcome.  Gated on `SKY_E2E=1`.
fn build_run_oninput_closure() -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = root
        .join("tests")
        .join("golden")
        .join("m7_stdui_oninput_closure");
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join("skyc_m7_stdui_oninput_closure_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else {
        return (
            dir,
            support::RunOutcome {
                stdout: String::new(),
                exit_code: None,
            },
        );
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for m7_stdui_oninput_closure: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("m7_stdui_oninput_closure", &out);
    (dir, outcome)
}

/// Regression lock: `onInput` with a closure handler must
/// compile (cargo exit 0), emit HTML with the event attribute wired, and
/// the binary must exit 0.
#[test]
fn oninput_closure_arc_wrap_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let (dir, outcome) = build_run_oninput_closure();
    let html = &outcome.stdout;

    // The input event must be rendered — confirms Arc<dyn Fn(String)->M> was
    // correctly emitted and the type-checker accepted it.
    assert!(
        html.contains("data-sky-on=\"input\"") || html.contains("sky-input"),
        "m7_stdui_oninput_closure: input event must be rendered in HTML output\n--- actual ---\n{html}"
    );
    // The text content must be present.
    assert!(
        html.contains("input here"),
        "m7_stdui_oninput_closure: text 'input here' must be in HTML output\n--- actual ---\n{html}"
    );
    // The wrapping div from ui_layout must exist.
    assert!(
        html.contains("display:flex"),
        "m7_stdui_oninput_closure: ui_layout must emit flex container\n--- actual ---\n{html}"
    );

    support::assert_go_parity("m7_stdui_oninput_closure", &dir, html);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "m7_stdui_oninput_closure: must exit 0"
    );
}
