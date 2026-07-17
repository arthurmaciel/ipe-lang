//! Sentinel: genuine bottom-up M-propagation for Std.Ui event subtrees.
//!
//! This golden covers: a non-view function (`staticHtml : String`)
//! that renders an event-bearing subtree (`el [onClick Bump] (text "x")`) must
//! compile, build, and run without a cargo E0308.
//!
//! A `UiLayout` arm emitting `ui_layout::<()>(…)` (an
//! `enclosing_ui_msg().unwrap_or("()")` fallback) while the event kernel emits
//! `Attribute<MainMsg>` produces a type mismatch. Instead, M is inferred
//! bottom-up from the event's concrete `Attribute<MainMsg>` with no turbofish.
//!
//! ## Oracle provenance
//!
//! `oracle_divergence = true`: the Go reference compiler (`sky dev`) does not
//! expose `Html.htmlRender` and exits 1 on this source.  `expected_go.txt` holds
//! skyc's own correct output — the Rust-backend HTML with the click event wired.
//!
//! ## What is tested
//!
//! * `ir_type_from_ty` handles `Attribute Msg` in the region-type map
//!   (`SolvedTypes.regions`), producing `IrType::Ui { AttrEvent, msg: MainMsg }`.
//! * `KernelFn::UiOnClick` lowers and emits `ui_on_click_(MainMsg::Bump)` →
//!   `element::Attribute::AttrEvent(EventAttr(OnMsg("click", Bump)))`.
//! * `UiLayout` emits no turbofish; Rust infers `M = MainMsg`
//!   bottom-up from the concrete `Attribute<MainMsg>` element.
//! * The rendered HTML contains `data-sky-on="click"` and the text node `x`.
//! * The binary exits 0.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m7_stdui_onclick
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile / build / run `tests/golden/m7_stdui_onclick/Main.sky` and return
/// the golden directory together with the run outcome.  Gated on `SKY_E2E=1`.
fn build_run_onclick() -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = root.join("tests").join("golden").join("m7_stdui_onclick");
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join("skyc_m7_stdui_onclick_e2e");
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
        "skyc build must succeed for m7_stdui_onclick: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("m7_stdui_onclick", &out);
    (dir, outcome)
}

/// Sentinel: `onClick` event in a non-view function must compile, build,
/// run, and produce HTML with the event attribute wired.  M is inferred
/// bottom-up from the event payload — no turbofish fallback to `()`.
#[test]
fn onclick_in_non_view_fn_propagates_m_bottom_up() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let (dir, outcome) = build_run_onclick();
    let html = &outcome.stdout;

    // The click event must be rendered — confirms M=MainMsg not M=().
    assert!(
        html.contains("data-sky-on=\"click\"") || html.contains("sky-click"),
        "m7_stdui_onclick: click event must be rendered in HTML output\n--- actual ---\n{html}"
    );
    // The text content must be present.
    assert!(
        html.contains(">x<"),
        "m7_stdui_onclick: text 'x' must be in HTML output\n--- actual ---\n{html}"
    );
    // The wrapping div structure from ui_layout must exist.
    assert!(
        html.contains("display:flex"),
        "m7_stdui_onclick: ui_layout must emit flex container\n--- actual ---\n{html}"
    );

    support::assert_go_parity("m7_stdui_onclick", &dir, html);
    assert_eq!(outcome.exit_code, Some(0), "m7_stdui_onclick: must exit 0");
}
