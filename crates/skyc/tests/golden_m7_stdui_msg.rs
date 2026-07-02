//! M7 `Ui.layout` turbofish Phase-0 gate —
//! `Ui.layout` inside a function annotated `Msg -> Html Msg` must emit
//! `ui_layout::<MainMsg>(...)` (the enclosing return type's message parameter),
//! NOT the hardcoded `ui_layout::<()>(...)` (BLOCKER-1 fix).
//!
//! The golden compiles `tests/golden/m7_stdui_msg/Main.sky` through `skyc`,
//! builds the emitted Rust project with the shared cargo target, runs the
//! binary, and checks its stdout against the cached oracle
//! (`tests/golden/m7_stdui_msg/oracle.meta` + `expected_go.txt`).
//! The test is gated on `SKY_E2E=1`; without it it returns early.
//!
//! ## Oracle provenance
//!
//! This is a DIVERGENCE golden (`oracle_divergence = true`).  The Go reference
//! compiler emits a different HTML skeleton.  `expected_go.txt` therefore holds
//! skyc's OWN output — the Rust-backend correct rendering — rather than the Go
//! oracle.  The divergence is documented in
//! `tests/golden/m7_stdui_msg/sanctioned.divergence`.
//!
//! ## What is tested
//!
//! * `ir_type_from_canon` handles `Html Msg` in type annotations, producing
//!   `IrType::Ui { ctor: UiCtor::Html, msg: IrType::Enum { Msg, [] } }`.
//! * `emit_func` extracts the enclosing return type's `msg` parameter and
//!   threads it into `GenericScope` via `with_ui_msg`.
//! * `KernelFn::UiLayout` arm uses `generics.enclosing_ui_msg()` rather than
//!   the hardcoded `"()"` — emitting `ui_layout::<MainMsg>(…)`.
//! * The rendered HTML is correct (BLOCKER-1 is a soundness fix, not just a
//!   type-level change: the wrong monomorphisation previously silently produced
//!   a `fn(()) -> Html<()>` that type-checked only because `msg` was
//!   unconstrained in the no-event smoke test).
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m7_stdui_msg
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile / build / run `tests/golden/m7_stdui_msg/Main.sky` and return the
/// golden directory together with the run outcome. Gated on `SKY_E2E=1`.
fn build_run_msg() -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = root.join("tests").join("golden").join("m7_stdui_msg");
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join("skyc_m7_stdui_msg_e2e");
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
        "skyc build must succeed for m7_stdui_msg: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("m7_stdui_msg", &out);
    (dir, outcome)
}

/// Full E2E smoke test: `Ui.layout` inside a `Msg -> Html Msg` function must
/// compile, build, run, and produce the cached expected HTML.
/// Divergence golden — the expected value is skyc's own correct output.
#[test]
fn ui_layout_turbofish_uses_enclosing_msg_type() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let (dir, outcome) = build_run_msg();
    let html = &outcome.stdout;

    // The output must contain the typed text node.
    assert!(
        html.contains("typed"),
        "m7_stdui_msg: text 'typed' must be present in output\n--- actual ---\n{html}"
    );
    // Font.bold must be applied.
    assert!(
        html.contains("font-weight:700"),
        "m7_stdui_msg: Font.bold must render font-weight:700\n--- actual ---\n{html}"
    );

    support::assert_go_parity("m7_stdui_msg", &dir, html);
    assert_eq!(outcome.exit_code, Some(0), "m7_stdui_msg: must exit 0");
}
