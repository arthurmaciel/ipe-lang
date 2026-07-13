//! Seal gate for the native `Std.Ui.transitionRaw` primitive + the compiled
//! `Std.Ui.Transition` module (26-ui-showcase blocker: SKY-N0004 unknown
//! module `Transition`).
//!
//! `Std.Ui.Transition.attribute` / `attributeUnsafe` are pure-Sky wrappers over
//! the native `Ui.transitionRaw : String -> Bool -> Attribute msg` kernel
//! (`KernelFn::UiTransitionRaw`), which constructs `AttrTransition shorthand
//! respect`.  This test proves the whole seam:
//!   * `import Std.Ui.Transition` resolves (no SKY-N0004 regression);
//!   * `transitionRaw` type-checks as `String -> Bool -> Attribute msg`;
//!   * the emit lowers both entry points to `ui_transition_raw_(<shorthand>,
//!     <respect>)` with the CORRECT respect flag (`true` for `attribute`,
//!     `false` for `attributeUnsafe`);
//!   * (`SKY_E2E`) the emitted Cargo project builds — the seal: skyc exit-0 ⟹
//!     cargo exit-0 — and renders the CSS `transition:` shorthand.

use std::path::PathBuf;

mod support;

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    skyc::resolve_runtime().expect("runtime must resolve for transition seal test")
}

/// A minimal Std.Ui program exercising BOTH transition entry points: the
/// a11y-gated `attribute` (respect = True) and the opt-out `attributeUnsafe`
/// (respect = False).
const MAIN_SKY: &str = r#"module Main exposing (main)

import Std.Html as Html
import Std.Ui as Ui
import Std.Ui.Transition as Transition


main =
    println
        (Html.htmlRender
            (Ui.layout []
                (Ui.column []
                    [ Ui.el
                        [ Transition.attribute
                            [ Transition.property "background-color"
                            , Transition.duration 200
                            , Transition.easing Transition.easeOut
                            ]
                        ]
                        (Ui.text "safe")
                    , Ui.el
                        [ Transition.attributeUnsafe
                            [ Transition.property "opacity"
                            , Transition.duration 100
                            ]
                        ]
                        (Ui.text "unsafe")
                    ])))
"#;

// `slot` gives each `#[test]` its OWN project dir under CARGO_TARGET_TMPDIR:
// the two tests here run in parallel by default and previously shared one
// `stdui_transition_seal` path, so one test's `remove_dir_all` could race the
// other's `create_dir_all` (a NotFound flake). A per-test slot removes the
// shared-path contention entirely.
#[allow(clippy::expect_used)]
fn build_transition_project(slot: &str) -> (PathBuf, Result<(), skyc::CliError>) {
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("stdui_transition_seal_{slot}"));
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).expect("mk transition test project dirs");
    let entry = src.join("Main.sky");
    std::fs::write(&entry, MAIN_SKY).expect("write Main.sky");
    let emit = out.join("emit");
    let res = skyc::build(&entry, &emit, &runtime());
    (emit, res)
}

/// `Std.Ui.Transition` resolves (no SKY-N0004), type-checks, and the emit
/// lowers both wrappers to `ui_transition_raw_` with the correct respect flags.
#[test]
#[allow(clippy::expect_used)]
fn transition_module_resolves_and_emits_kernel() {
    let (emit, res) = build_transition_project("emit");
    assert!(
        res.is_ok(),
        "skyc build with `import Std.Ui.Transition` must succeed \
         (native transitionRaw + compiled module): {:?}",
        res.err()
    );

    // The compiled `Std.Ui.Transition` module lowers to its OWN Rust file
    // under `src/sky_mods/` once the per-Sky-module split (Phase 5 Milestone C)
    // fires — this program has two distinct homes (`Main` + `Std.Ui.Transition`).
    // Scan the WHOLE emitted Sky-side tree (main.rs + sky_mods/*.rs) so the
    // assertion holds wherever the split correctly placed the helper calls.
    let emitted = support::read_all_emitted_src(&emit);

    // Both entry points lower through the native kernel helper — once each.
    let calls = emitted.matches("ui_transition_raw_(").count();
    assert!(
        calls >= 2,
        "emitted Rust must carry BOTH transitionRaw helper calls (got {calls}):\n{emitted}"
    );
    // `attribute` gates on reduced-motion (respect = true); `attributeUnsafe`
    // opts out (respect = false). Both flags must reach the emit as the
    // second argument of the helper — proving the flag is threaded, not dropped.
    assert!(
        emitted.contains("ui_transition_raw_(") && emitted.contains(", true)"),
        "the a11y-gated `attribute` must emit `ui_transition_raw_(…, true)`:\n{emitted}"
    );
    assert!(
        emitted.contains(", false)"),
        "`attributeUnsafe` must emit `ui_transition_raw_(…, false)`:\n{emitted}"
    );
}

/// The GREEN GATE: under `SKY_E2E=1` the emitted Cargo project builds and runs,
/// rendering the CSS `transition:` shorthand — the seal, end to end.
#[test]
fn transition_e2e_builds_and_renders_shorthand() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let (emit, res) = build_transition_project("e2e");
    assert!(
        res.is_ok(),
        "transition E2E build must succeed: {:?}",
        res.err()
    );

    let outcome = support::build_and_run_emitted("stdui_transition_seal", &emit);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "emitted transition binary must exit 0 (the seal)"
    );
    assert!(
        outcome
            .stdout
            .contains("transition:background-color 200ms ease-out"),
        "rendered HTML must carry the built CSS transition shorthand:\n{}",
        outcome.stdout
    );
}
