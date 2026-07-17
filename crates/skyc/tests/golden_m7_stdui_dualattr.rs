//! Dual-attribute bridge gate —
//! mixing `Std.Ui` typed attributes (`Font.bold`, `Background.color`) with a
//! `Std.Html` node (`Html.div` / `Html.text`) embedded via `Ui.html`.
//!
//! The golden compiles `tests/golden/m7_stdui_dualattr/Main.sky` through
//! `skyc`, builds the emitted Rust project with the shared cargo target, runs
//! the binary, and checks its stdout against the cached oracle
//! (`tests/golden/m7_stdui_dualattr/oracle.meta` + `expected_go.txt`).
//! The test is gated on `SKY_E2E=1`; without it it returns early.
//!
//! ## Oracle provenance
//!
//! This is a DIVERGENCE golden (`oracle_divergence = true`).  The Go reference
//! compiler emits a different HTML skeleton.  `expected_go.txt` therefore holds
//! skyc's OWN output — the Rust-backend correct rendering — rather than the Go
//! oracle.  The divergence is documented in
//! `tests/golden/m7_stdui_dualattr/sanctioned.divergence`.
//!
//! ## What is tested
//!
//! * `Ui.el [ Font.bold, Background.color (Ui.rgb 0 128 0) ]` emits
//!   `font-weight:700;background-color:rgba(0,128,0,1)` on one element.
//! * `Ui.html (Html.div [] [ Html.text "html-node" ])` bridges a raw Html node
//!   into the Std.Ui element tree — the `UiCtor::Html` / `UiHtml` bridge path.
//! * `Ui.column [ Ui.spacing 8 ]` emits `gap:8px`.
//! * No internal direction markers (`__col`, `__row`) leak into the output.
//! * The full path `Std.Ui` + `Std.Html` kernel mix builds and runs.
//!
//! ## Scope note
//!
//! The full dual-Attribute+Event golden (mixing typed Std.Ui attributes and
//! Html.Events event handlers) is blocked by the `UiCtor::HtmlEvent`
//! emit-arm gap (SKY-I0###).  This reduced golden avoids event kernels and
//! exercises only the attribute + Html-node bridge.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m7_stdui_dualattr
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile / build / run `tests/golden/m7_stdui_dualattr/Main.sky` and return
/// the golden directory together with the run outcome. Gated on `SKY_E2E=1`.
fn build_run_dualattr() -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = root.join("tests").join("golden").join("m7_stdui_dualattr");
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join("skyc_m7_stdui_dualattr_e2e");
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
        "skyc build must succeed for m7_stdui_dualattr: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("m7_stdui_dualattr", &out);
    (dir, outcome)
}

/// Full E2E smoke test: dual-attribute Std.Ui + Std.Html bridge must compile,
/// build, run, and produce the cached expected HTML.
/// Divergence golden — the expected value is skyc's own correct output.
#[test]
fn dualattr_stdui_attributes_and_html_node_bridge_render_correctly() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let (dir, outcome) = build_run_dualattr();
    let html = &outcome.stdout;

    // Font.bold + Background.color must both be applied to the same element.
    assert!(
        html.contains("font-weight:700"),
        "m7_stdui_dualattr: Font.bold must render font-weight:700\n--- actual ---\n{html}"
    );
    assert!(
        html.contains("background-color:rgba(0,128,0,1)"),
        "m7_stdui_dualattr: Background.color must render correctly\n--- actual ---\n{html}"
    );
    // The Html.div / Html.text bridge must be present.
    assert!(
        html.contains("html-node"),
        "m7_stdui_dualattr: Ui.html bridge node must be present\n--- actual ---\n{html}"
    );
    // Spacing on the column must be present.
    assert!(
        html.contains("gap:8px"),
        "m7_stdui_dualattr: Ui.column spacing must render gap:8px\n--- actual ---\n{html}"
    );
    // Internal direction markers must NOT leak into output.
    assert!(
        !html.contains("__col") && !html.contains("__row"),
        "m7_stdui_dualattr: direction markers must not leak into HTML output\n--- actual ---\n{html}"
    );

    support::assert_go_parity("m7_stdui_dualattr", &dir, html);
    assert_eq!(outcome.exit_code, Some(0), "m7_stdui_dualattr: must exit 0");
}
