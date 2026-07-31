//! Integration + security golden for compiled-source `Ipe.Css`.
//!
//! `Ipe.Css` is compiled pure Ipê source (`crates/ipe/stdlib/Std/Css.ipe`); its
//! only Rust surface is the four `Ipe.CssSafety` leaf security kernels
//! (`safeValue` / `safePropName` / `safeSelector` / `stripStyleClose`). These
//! lock:
//!   * the module injects → canonicalises-as-stdlib → lowers → emits (a Std-homed
//!     `CssProp` / `CssRule` ADT defined AND matched — kernel-impossible);
//!   * (`IPE_E2E`) the emitted binary RUNS and its CSS output keeps the benign
//!     rule byte-for-byte while NEUTRALISING all three injection vectors
//!     (value breakout, `url(javascript:)`, selector breakout).

use std::path::{Path, PathBuf};

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    ipe::resolve_runtime().expect("runtime must resolve for css golden")
}

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn css_manifest() -> PathBuf {
    repo_root()
        .join("tests")
        .join("fixtures")
        .join("spike-css-source")
        .join("ipe.toml")
}

/// The compiled-source `Ipe.Css` resolves + lowers like a user module: the
/// project builds (no `IPE-N0001 stylesheet not found`), the emitted Rust carries
/// the Std-homed render fold, and it routes free strings through the leaf
/// security kernels (`safe_value` / `safe_selector`).
#[test]
fn css_source_builds_and_injects_leaf_kernels() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("css_source_emit");
    let _ = std::fs::remove_dir_all(&out);

    let res = ipe::build_project(&css_manifest(), &out, &runtime());
    assert!(
        res.is_ok(),
        "Ipe.Css project must build (inject → canon-as-stdlib → lower → emit): {:?}",
        res.err()
    );

    // The compiled `Ipe.Css` module lowers to its OWN Rust file under
    // `src/ipe_mods/` once the per-Ipê-module split
    // fires — this program has two distinct homes (`Main` + `Ipe.Css`). Scan
    // the WHOLE emitted Ipê-side tree (main.rs + ipe_mods/*.rs) so both the
    // presence assertions (render fold + leaf security kernels) and the
    // negative retired-enum assertion hold wherever the split placed the code.
    let emitted = crate::support::read_all_emitted_src(&out);
    // The compiled Ipe.Css render fold is homed + prefixed as compiled source.
    assert!(
        emitted.contains("ipe_css_stylesheet") && emitted.contains("ipe_css_render_rule"),
        "emitted Rust must carry the compiled Ipe.Css render fold"
    );
    // Free-string entries route through the leaf security kernels (the SOLE Rust
    // surface). Their presence proves the gate is wired, not bypassed.
    assert!(
        emitted.contains("safe_value") && emitted.contains("safe_selector"),
        "emitted Rust must call the css_safety leaf kernels (value + selector gates)"
    );
    // Design-2 retired: no reflection enum reaches the emitted Rust.
    assert!(
        !emitted.contains("css :: CssProp") && !emitted.contains("css::CssProp"),
        "the retired Design-2 runtime CssProp enum must not appear"
    );
}

/// SECURITY GOLDEN (`IPE_E2E`): the emitted binary runs and its CSS output keeps
/// the benign rule byte-for-byte while EVERY injection vector is neutralised —
/// no `</style>`, `<script>`, `javascript:`, `expression(`, or `alert` survives.
#[test]
fn css_e2e_neutralises_injection() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("css_source_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let res = ipe::build_project(&css_manifest(), &out, &runtime());
    assert!(res.is_ok(), "Ipe.Css build must succeed: {:?}", res.err());

    let outcome = crate::support::build_and_run_emitted("spike-css-source", &out);
    assert_eq!(outcome.exit_code, Some(0), "emitted binary must exit 0");

    let stdout = outcome.stdout;
    let low = stdout.to_ascii_lowercase();

    // Functional: the benign rule renders (byte-parity with the pure-Ipê fold).
    assert!(
        stdout.contains(".x {") && stdout.contains("#ff6600") && stdout.contains("8px"),
        "benign stylesheet must render:\n{stdout}"
    );

    // non-regression: a benign `keyframes` still renders its frames.
    assert!(
        stdout.contains("opacity: 0") && stdout.contains("opacity: 1"),
        "benign keyframes must render:\n{stdout}"
    );

    // Security: NONE of the injection payloads survive in any form —
    // including the `@import` (CSS-level SSRF) vector newly gated on
    // `raw` / `keyframes` bodies.
    for needle in [
        "</style",
        "<script",
        "javascript:",
        "expression(",
        "alert",
        "@import",
    ] {
        assert!(
            !low.contains(needle),
            "injection needle {needle:?} must be neutralised, but survived in:\n{stdout}"
        );
    }
}
