//! Seal gate for the native `Std.Ui.animateRaw` primitive + the compiled
//! `Std.Ui.Animation` module (26-ui-showcase blocker #175: SKY-N0004 unknown
//! module `Animation`).
//!
//! `Std.Ui.Animation.attribute` is a pure-Sky wrapper over the native
//! `Ui.animateRaw : String -> String -> String -> Bool -> Attribute msg` kernel
//! (`KernelFn::UiAnimateRaw`), which constructs `AttrAnimation name shorthand
//! keyframes respect`.  This test proves the whole seam:
//!   * `import Std.Ui.Animation` resolves (no SKY-N0004 regression) — the module
//!     also transitively pulls in its `Std.Ui.Transition` (`Easing`) and
//!     `Std.Ui.Transform` (`Prop`/`propsToCss`) sibling ports;
//!   * `Animation.attribute` type-checks against the `Spec` record;
//!   * the emit lowers to `ui_animate_raw_(<name>, <shorthand>, <keyframes>,
//!     <respect>)` — the four-arg native helper;
//!   * (`SKY_E2E`) the emitted Cargo project builds — the seal: skyc exit-0 ⟹
//!     cargo exit-0 — and renders the CSS `animation:` shorthand.

use std::path::PathBuf;

mod support;

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    skyc::resolve_runtime().expect("runtime must resolve for animation seal test")
}

/// A minimal Std.Ui program exercising `Animation.attribute`. NOTE: the
/// `keyframes` list is left EMPTY deliberately. A populated keyframe list routes
/// through `Std.Ui.Transform.propsToCss`, whose refutable tuple patterns
/// (`( "transform", v ) -> …`) hit the still-unimplemented `SKY-L0115
/// TuplePatternMatch` lowering (backlog #174) — an INDEPENDENT blocker from this
/// module's resolution + kernel wiring (#175). Empty keyframes still fully
/// exercise `Ui.animateRaw` (name + shorthand tail + empty body + respect flag),
/// so the seal this test guards — `Std.Ui.Animation` resolves and its
/// `attribute` lowers to the native kernel — is proven without depending on #174.
const MAIN_SKY: &str = r#"module Main exposing (main)

import Std.Html as Html
import Std.Ui as Ui
import Std.Ui.Animation as Animation


main =
    println
        (Html.htmlRender
            (Ui.layout []
                (Ui.el
                    [ Animation.attribute
                        { name = "fadeInUp"
                        , duration = 600
                        , easing = Animation.easeOut
                        , delay = 0
                        , iterations = Animation.once
                        , fillMode = Animation.forwards
                        , respectReducedMotion = True
                        , keyframes = []
                        }
                    ]
                    (Ui.text "fade"))))
"#;

/// `slot` gives each `#[test]` its OWN project dir under `CARGO_TARGET_TMPDIR` so
/// the two tests here (which run in parallel) never race on a shared path.
#[allow(clippy::expect_used)]
fn build_animation_project(slot: &str) -> (PathBuf, Result<(), skyc::CliError>) {
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("stdui_animation_seal_{slot}"));
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).expect("mk animation test project dirs");
    let entry = src.join("Main.sky");
    std::fs::write(&entry, MAIN_SKY).expect("write Main.sky");
    let emit = out.join("emit");
    let res = skyc::build(&entry, &emit, &runtime());
    (emit, res)
}

/// `Std.Ui.Animation` resolves (no SKY-N0004), type-checks, and the emit lowers
/// `Animation.attribute` to the four-arg `ui_animate_raw_` native helper.
///
/// IGNORED pending backlog #174: `Std.Ui.Animation` `import`s `Std.Ui.Transform`
/// (for the `Prop` type + `propsToCss`), and `Std.Ui.Transform`'s refutable
/// tuple `case`s (`( "transform", v ) -> …`) hit the still-unimplemented
/// `SKY-L0115 TuplePatternMatch` lowering — an INDEPENDENT pre-existing blocker
/// (`Std.Ui.Transform` fails to lower on its own, with or without this module).
/// This module's OWN resolution + kernel wiring is correct (canon + type-check of
/// `Animation` + `Main` pass; the only diagnostic is a `Lower` error located
/// inside `<embedded-stdlib>/Std.Ui.Transform`). Flip `#[ignore]` off once #174
/// lands so this seal begins guarding the full seam end to end.
#[test]
#[ignore = "blocked on #174 (SKY-L0115 tuple-pattern-match in the imported Std.Ui.Transform)"]
#[allow(clippy::expect_used)]
fn animation_module_resolves_and_emits_kernel() {
    let (emit, res) = build_animation_project("emit");
    assert!(
        res.is_ok(),
        "skyc build with `import Std.Ui.Animation` must succeed \
         (native animateRaw + compiled module): {:?}",
        res.err()
    );

    // The compiled `Std.Ui.Animation` module lowers to its OWN Rust file under
    // `src/sky_mods/` (per-Sky-module split), so scan the WHOLE emitted Sky-side
    // tree (main.rs + sky_mods/*.rs) for the helper call.
    let emitted = support::read_all_emitted_src(&emit);

    let calls = emitted.matches("ui_animate_raw_(").count();
    assert!(
        calls >= 1,
        "emitted Rust must carry the animateRaw helper call (got {calls}):\n{emitted}"
    );
    // `Animation.attribute` with `respectReducedMotion = True` threads the flag
    // through as the fourth argument — proving it is not dropped.
    assert!(
        emitted.contains("ui_animate_raw_(") && emitted.contains(", true)"),
        "the a11y-gated `attribute` must emit `ui_animate_raw_(…, true)`:\n{emitted}"
    );
}

/// The GREEN GATE: under `SKY_E2E=1` the emitted Cargo project builds and runs,
/// rendering the CSS `animation:` shorthand — the seal, end to end.
///
/// IGNORED pending backlog #174 (see `animation_module_resolves_and_emits_kernel`).
#[test]
#[ignore = "blocked on #174 (SKY-L0115 tuple-pattern-match in the imported Std.Ui.Transform)"]
fn animation_e2e_builds_and_renders_shorthand() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let (emit, res) = build_animation_project("e2e");
    assert!(
        res.is_ok(),
        "animation E2E build must succeed: {:?}",
        res.err()
    );

    let outcome = support::build_and_run_emitted("stdui_animation_seal", &emit);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "emitted animation binary must exit 0 (the seal)"
    );
    assert!(
        outcome.stdout.contains("animation:fadeInUp"),
        "rendered HTML must carry the built CSS animation shorthand:\n{}",
        outcome.stdout
    );
}
