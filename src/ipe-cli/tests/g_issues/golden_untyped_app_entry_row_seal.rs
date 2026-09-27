//! THE SEAL for an app entry built inside an UNANNOTATED definition whose
//! parameter the entry's model is read through.
//!
//! `mk m = Web.embed { init = \_ -> ( m, Cmd.none ), … }` with a `view` that
//! reads `model.count` infers `m : { r | count : Int }` — an open record whose
//! row tail is generalized with no annotation to name it. The generic-entry
//! walk sees no annotation row generics for an unannotated definition, so this
//! shape must land on exactly one of the two sound outcomes:
//!
//! - refused by `ipe` with IPE-N0051 (the entry's model still generic), or
//! - accepted, and then — under `IPE_E2E` — the emitted crate `cargo build`s.
//!
//! Any other diagnostic, or an accept whose crate fails cargo, fails the test.
//! The entry is a mounted server app, so the emitted binary listens forever;
//! the positive leg proves the build, not a run.
//!
//! ```text
//! # emit / refusal check only (fast):
//! cargo test -p ipe --test g_issues golden_untyped_app_entry_row
//! # full (cargo build of an accepted program):
//! IPE_E2E=1 cargo test -p ipe --test g_issues golden_untyped_app_entry_row
//! ```

use std::path::PathBuf;

use ipe::CliError;

/// A runtime `false` the optimiser cannot fold, so `assert!(false_marker(), …)`
/// reads as a deliberate unconditional failure rather than a suspicious constant
/// condition.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

/// Write `source` as a single-file `Main.ipe` under a fresh scratch dir keyed by
/// `name`, returning the entry path (or `None` if scratch setup fails).
fn write_single(name: &str, source: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("untyped-app-entry-row")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).ok()?;
    let entry = src.join("Main.ipe");
    std::fs::write(&entry, source).ok()?;
    Some(entry)
}

/// The scratch output dir for `name`, cleared.
fn out_dir(name: &str) -> PathBuf {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("untyped-app-entry-row-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// An unannotated `mk` whose `Web.embed` model is the parameter `m`, read
/// through `model.count` in `view`.
const UNTYPED_EMBED_ROW_MODEL: &str = r#"module Main exposing (main)

import Ipe.Server.Http as Server
import Ipe.String as String
import Ipe.Task as Task exposing (Task)
import Ipe.Tea.Web as Web
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
import Ipe.Ui as Ui


type alias Model =
    { count : Int }


type Msg
    = Noop


initialModel : Model
initialModel =
    { count = 0 }


mk m =
    Web.embed
        { init = \_ -> ( m, Cmd.none )
        , update = \_ model -> ( model, Cmd.none )
        , view = \model -> Ui.text (String.fromInt model.count)
        , subscriptions = \_ -> Sub.none
        , routes = []
        , notFound = Noop
        }


main : Task Error ()
main =
    Server.listen 8000 [ Server.mountApp "/" (mk initialModel) ]
"#;

#[test]
fn untyped_embed_row_model_refused_or_builds() {
    let name = "untyped_embed_row_model";
    let Some(entry) = write_single(name, UNTYPED_EMBED_ROW_MODEL) else {
        assert!(
            false_marker(),
            "{name}: could not write the fixture into the scratch dir"
        );
        return;
    };
    let out = out_dir(name);
    let runtime = match ipe::resolve_runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            assert!(
                false_marker(),
                "{name}: the embedded runtime could not be resolved: {err:?}"
            );
            return;
        }
    };
    match ipe::build(&entry, &out, &runtime) {
        Ok(()) => crate::support::assert_seal_builds(name, &out),
        Err(CliError::Pipeline { diag, .. }) => assert_eq!(
            diag.code(),
            ipe_diagnostics::IPE_N0051,
            "{name}: a refusal must be the generic-app-entry IPE-N0051, not another reason"
        ),
        Err(other) => assert!(
            false_marker(),
            "{name}: non-pipeline build error: {other:?}"
        ),
    }
}
