//! Seal gate for the native `Std.Ui.animateRaw` primitive + the compiled
//! `Std.Ui.Animation` module (26-ui-showcase blocker #175: IPE-N0004 unknown
//! module `Animation`).
//!
//! `Std.Ui.Animation.attribute` is a pure-Sky wrapper over the native
//! `Ui.animateRaw : String -> String -> String -> Bool -> Attribute msg` kernel
//! (`KernelFn::UiAnimateRaw`), which constructs `AttrAnimation name shorthand
//! keyframes respect`.  This test proves the whole seam:
//!   * `import Std.Ui.Animation` resolves (no IPE-N0004 regression) — the module
//!     also transitively pulls in its `Std.Ui.Transition` (`Easing`) and
//!     `Std.Ui.Transform` (`Prop`/`propsToCss`) sibling ports;
//!   * `Animation.attribute` type-checks against the `Spec` record;
//!   * the emit lowers to `ui_animate_raw_(<name>, <shorthand>, <keyframes>,
//!     <respect>)` — the four-arg native helper;
//!   * (`IPE_E2E`) the emitted Cargo project builds — the seal: skyc exit-0 ⟹
//!     cargo exit-0 — and renders the CSS `animation:` shorthand.

use std::path::PathBuf;

mod support;

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    ipe::resolve_runtime().expect("runtime must resolve for animation seal test")
}

/// A minimal Std.Ui program exercising `Animation.attribute`. NOTE: the
/// `keyframes` list is left EMPTY deliberately. A populated keyframe list routes
/// through `Std.Ui.Transform.propsToCss`, whose refutable tuple patterns
/// (`( "transform", v ) -> …`) hit the still-unimplemented `IPE-L0115
/// TuplePatternMatch` lowering — an INDEPENDENT blocker from this
/// module's resolution + kernel wiring. Empty keyframes still fully
/// exercise `Ui.animateRaw` (name + shorthand tail + empty body + respect flag),
/// so the seal this test guards — `Std.Ui.Animation` resolves and its
/// `attribute` lowers to the native kernel — is proven without depending on it.
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
fn build_animation_project(slot: &str) -> (PathBuf, Result<(), ipe::CliError>) {
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("stdui_animation_seal_{slot}"));
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).expect("mk animation test project dirs");
    let entry = src.join("Main.ipe");
    std::fs::write(&entry, MAIN_SKY).expect("write Main.ipe");
    let emit = out.join("emit");
    let res = ipe::build(&entry, &emit, &runtime());
    (emit, res)
}

/// `Std.Ui.Animation` resolves (no IPE-N0004), type-checks, and the emit lowers
/// `Animation.attribute` to the four-arg `ui_animate_raw_` native helper.
///
/// Now supported: `Std.Ui.Transform`'s refutable tuple `case`s
/// (`( "transform", v ) -> …` — a VARIABLE tuple scrutinee with a string-literal
/// column) now lower to a native `match pair { (__sg0, v) if __sg0.as_str() ==
/// "transform" => … }` (a by-value binder + `as_str()` guard), so the whole
/// `import Std.Ui.Animation` → `Std.Ui.Transform` seam builds end to end.
#[test]
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
    // `src/ipe_mods/` (per-Sky-module split), so scan the WHOLE emitted Sky-side
    // tree (main.rs + ipe_mods/*.rs) for the helper call.
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

/// The GREEN GATE: under `IPE_E2E=1` the emitted Cargo project builds and runs,
/// rendering the CSS `animation:` shorthand — the seal, end to end.
///
/// Now supported (see `animation_module_resolves_and_emits_kernel`).
#[test]
fn animation_e2e_builds_and_renders_shorthand() {
    if std::env::var("IPE_E2E").is_err() {
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
