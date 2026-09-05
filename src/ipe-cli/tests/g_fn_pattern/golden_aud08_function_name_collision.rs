//! AUD-08 regression — two DISTINCT functions in DIFFERENT modules whose
//! `(home, name)` identity folds to the SAME Rust identifier under
//! `naming::module_value`'s `snake_case` fold must be rejected with
//! `NameError::RustNameFold`, never silently emit two functions sharing
//! one Rust name (a `rustc` E0428 "duplicate definition", or worse, whichever
//! definition the emitter's map keeps last silently winning). Unlike a
//! same-name source redefinition (`DuplicateValue`), this is a mangling
//! collision between two differently-spelled Ipê definitions, so the
//! diagnostic names both Ipê paths and the shared Rust name.
//!
//! `module_value`'s fold is not injective over the (home, name) split:
//! `["ZuiBorder"]/"rounded"` and `["Zui"]/"borderRounded"` both fold to
//! `zui_border_rounded` (verified against `to_snake_case`'s exact algorithm —
//! an interior uppercase char always emits a `_` boundary, so `ZuiBorder_rounded`
//! and `Zui_borderRounded` produce byte-identical output). `zui` is NOT a kernel
//! namespace, so no `user_` disambiguation prefix applies, and the shared
//! identifier the collision guard reports is `zui_border_rounded`. The module
//! names are DELIBERATELY not `Ui`/`UiBorder`: `Ui` is a reserved Tier-C stdlib
//! qualifier (`Ipe.Ui`), so a bare `Ui.borderRounded` would raise IPE-N0034
//! (demanding `import Ipe.Ui`) and mask the fold-collision this test covers;
//! `Zui`/`ZuiBorder` are non-reserved names that fold identically. Mirrors the sibling
//! enum-name collision guard (`crates/ipe_backend_rust/src/lib.rs`, the
//! `enum_names.values().any(...)` check ~10 lines above the guard this
//! test covers).
//!
//! ```text
//! cargo test -p ipe --test golden_aud08_function_name_collision
//! ```

use std::fs;
use std::path::PathBuf;

/// A runtime `false` the optimiser cannot fold — mirrors
/// `crates/ipe/src/lib.rs`'s own test helper and the AUD-14 regression's.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

fn write_project(dir: &std::path::Path, files: &[(&str, &str)]) -> bool {
    let src = dir.join("src");
    let _ = fs::remove_dir_all(dir);
    if fs::create_dir_all(&src).is_err() {
        return false;
    }
    files
        .iter()
        .all(|(name, contents)| fs::write(src.join(name), contents).is_ok())
}

#[test]
fn distinct_functions_folding_to_the_same_rust_name_are_rejected() {
    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // runtime unavailable in this environment — skip silently
    };

    let tmp = std::env::temp_dir().join("ipec_aud08_function_name_collision");
    let wrote = write_project(
        &tmp,
        &[
            (
                "ZuiBorder.ipe",
                "module ZuiBorder exposing (rounded)\n\
                 rounded : Int -> Int\n\
                 rounded x = x\n",
            ),
            (
                "Zui.ipe",
                "module Zui exposing (borderRounded)\n\
                 borderRounded : Int -> Int\n\
                 borderRounded x = x + 1\n",
            ),
            (
                "Main.ipe",
                "module Main exposing (main)\n\
                 import Ipe.Io as Io\n\
                 import Ipe.String\n\
                 import ZuiBorder\n\
                 import Zui\n\n\
                 main = Io.println (String.fromInt (ZuiBorder.rounded 1 + Zui.borderRounded 1))\n",
            ),
        ],
    );
    assert!(wrote, "must write the fixture project to a temp dir");

    let entry = tmp.join("src").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("aud08_function_name_collision_out");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_with_sibling_discovery(&entry, &out, &runtime);
    let Err(err) = built else {
        assert!(
            false_marker(),
            "expected a RustNameFold rejection for ZuiBorder.rounded vs \
             Zui.borderRounded (both fold to `zui_border_rounded`), but ipec \
             build SUCCEEDED — the collision would silently emit two Rust \
             fns sharing one name"
        );
        return;
    };
    let ipe::CliError::Pipeline { diag, .. } = &err else {
        assert!(false_marker(), "expected a Pipeline diagnostic, got: {err}");
        return;
    };
    let ipe_diagnostics::Diagnostic::Name {
        msg:
            ipe_diagnostics::NameError::RustNameFold {
                first,
                second,
                rust_name,
                kind,
            },
        ..
    } = &**diag
    else {
        assert!(false_marker(), "expected NameError::RustNameFold, got: {err}");
        return;
    };
    assert_eq!(&**rust_name, "zui_border_rounded");
    assert_eq!(*kind, ipe_diagnostics::RustNameFoldKind::Value);
    // Both colliding Ipê definitions are named, so the fix is unambiguous.
    let both: [&str; 2] = [first, second];
    assert!(
        both.contains(&"ZuiBorder.rounded") && both.contains(&"Zui.borderRounded"),
        "diagnostic must name both folding Ipê definitions, got {both:?}"
    );
}

/// Positive control: two functions in different modules whose names do NOT
/// collide under the fold must build cleanly — the guard must not be
/// over-eager and reject legitimate distinct names.
#[test]
fn distinct_functions_with_distinct_rust_names_are_accepted() {
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };

    let tmp = std::env::temp_dir().join("ipec_aud08_function_name_collision_control");
    let wrote = write_project(
        &tmp,
        &[
            (
                "Lib.ipe",
                "module Lib exposing (helper)\n\
                 helper : Int -> Int\n\
                 helper x = x\n",
            ),
            (
                "Main.ipe",
                "module Main exposing (main)\n\
                 import Ipe.Io as Io\n\
                 import Lib\n\n\
import Ipe.String
                 main = Io.println (String.fromInt (Lib.helper 1))\n",
            ),
        ],
    );
    assert!(wrote, "must write the fixture project to a temp dir");

    let entry = tmp.join("src").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("aud08_function_name_collision_control_out");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "distinct, non-colliding function names must build cleanly: {:?}",
        built.err()
    );
}
