//! `Std.Ui` / `Std.Html` gate —
//! `Ui.layout` / `Ui.column` / `Ui.el` / `Ui.text` / `Ui.spacing` /
//! `Background.color` / `Font.bold` / `Html.htmlRender` end-to-end smoke test.
//!
//! The golden compiles `tests/golden/m7_stdui/Main.sky` through `skyc`, builds
//! the emitted Rust project with the shared cargo target, runs the binary, and
//! checks its stdout against the cached oracle
//! (`tests/golden/m7_stdui/oracle.meta` + `expected_go.txt`).
//! The test is gated on `SKY_E2E=1`; without it it returns early.
//!
//! ## Oracle provenance
//!
//! This is a DIVERGENCE golden (`oracle_divergence = true`).  The Go reference
//! compiler emits a different HTML skeleton (separate `<style>` reset block,
//! `min-height`, trailing spaces).  `expected_go.txt` therefore holds skyc's
//! OWN output — the Rust-backend correct rendering — rather than the Go oracle.
//! The divergence is documented in `tests/golden/m7_stdui/sanctioned.divergence`.
//!
//! ## What is tested
//!
//! * `Std.Html` qualifier (`Std.Html.htmlRender`) resolves correctly via the
//!   qualifier-alias table (env.rs `QUALIFIER_ALIASES`).
//! * `Std.Ui` qualifier (`Std.Ui.layout`, `Std.Ui.column`, etc.) resolves
//!   correctly.
//! * `Ui.column [Ui.spacing N]` emits `display:flex;flex-direction:column;gap:N`.
//! * `Ui.el [Font.bold]` emits `font-weight:700`.
//! * `Ui.el [Background.color (Ui.rgb r g b)]` emits `background-color:rgba(r,g,b,1)`.
//! * Internal direction markers (`__col`, `__row`) do NOT leak into HTML output.
//! * The emitted Rust project builds and produces a correctly-shaped HTML string.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m7_stdui
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile / build / run `tests/golden/m7_stdui/Main.sky` and return the golden
/// directory together with the run outcome. Gated on `SKY_E2E=1`.
fn build_run_m7() -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = root.join("tests").join("golden").join("m7_stdui");
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join("skyc_m7_stdui_e2e");
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
        "skyc build must succeed for m7_stdui: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("m7_stdui", &out);
    (dir, outcome)
}

/// Full E2E smoke test: `Std.Ui` + `Std.Html.htmlRender` must compile, build,
/// run, and produce the cached expected HTML.  Divergence golden — the expected
/// value is skyc's own correct output, not the Go oracle.
#[test]
fn stdui_layout_column_el_text_renders_html() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let (dir, outcome) = build_run_m7();
    support::assert_go_parity("m7_stdui", &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "m7_stdui: must exit 0");
}
