//! AUD-08 regression — two DISTINCT functions in DIFFERENT modules whose
//! `(home, name)` identity folds to the SAME Rust identifier under
//! `naming::module_value`'s `snake_case` fold must be rejected with
//! `NameError::DuplicateValue`, never silently emit two functions sharing
//! one Rust name (a `rustc` E0428 "duplicate definition", or worse, whichever
//! definition the emitter's map keeps last silently winning).
//!
//! `module_value`'s fold is not injective over the (home, name) split:
//! `["UiBorder"]/"rounded"` and `["Ui"]/"borderRounded"` both fold to
//! `ui_border_rounded` (verified against `to_snake_case`'s exact algorithm —
//! an interior uppercase char always emits a `_` boundary, so `UiBorder_rounded`
//! and `Ui_borderRounded` produce byte-identical output). Because `ui` is a
//! kernel namespace, both names are further disambiguated with a `user_` prefix
//! (the user-module-vs-kernel guard), so the shared identifier the collision
//! guard reports is `user_ui_border_rounded`. Mirrors the sibling
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
                "UiBorder.ipe",
                "module UiBorder exposing (rounded)\n\
                 import Ipe.Prelude exposing (..)\n\n\
                 rounded : Int -> Int\n\
                 rounded x = x\n",
            ),
            (
                "Ui.ipe",
                "module Ui exposing (borderRounded)\n\
                 import Ipe.Prelude exposing (..)\n\n\
                 borderRounded : Int -> Int\n\
                 borderRounded x = x + 1\n",
            ),
            (
                "Main.ipe",
                "module Main exposing (main)\n\
                 import Ipe.Prelude exposing (..)\n\
                 import Ipe.Io as Io\n\
                 import UiBorder\n\
                 import Ui\n\n\
import Ipe.String
                 main = Io.println (String.fromInt (UiBorder.rounded 1 + Ui.borderRounded 1))\n",
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
            "expected a DuplicateValue rejection for UiBorder.rounded vs \
             Ui.borderRounded (both fold to `user_ui_border_rounded`), but ipec \
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
        msg: ipe_diagnostics::NameError::DuplicateValue { name, .. },
        ..
    } = diag
    else {
        assert!(
            false_marker(),
            "expected NameError::DuplicateValue, got: {err}"
        );
        return;
    };
    assert_eq!(&**name, "user_ui_border_rounded");
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
                 import Ipe.Prelude exposing (..)\n\n\
                 helper : Int -> Int\n\
                 helper x = x\n",
            ),
            (
                "Main.ipe",
                "module Main exposing (main)\n\
                 import Ipe.Prelude exposing (..)\n\
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
