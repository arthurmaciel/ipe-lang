//! Routed `Web.app` with an explicit `onNavigate : page -> msg` cfg field:
//! every URL-driven route change is turned into a `Msg` and dispatched through
//! `update`, so the app owns navigation instead of the runtime mutating the
//! model's `page` field.
//!
//! ## What this pins
//!
//! * ipe compiles the `onNavigate`-carrying routed app (the field is absorbed
//!   by the open Live cfg row).
//! * The emitted `set_page` closure passed to `web_app_routed` routes the
//!   matched page through the author's `update` (`(update)((onNavigate)(page),
//!   model)`) — the new page reaches the model only via `update`.
//! * The absent-field magic-page struct-update closure
//!   (`Model { page: __page, ..__model }`) is NOT emitted for this app — that
//!   form is reserved for apps that omit `onNavigate`.
//!
//! Pure ipe-pipeline check (parse → canon → types → lower → emit); no cargo
//! build. Skips if the embedded runtime cannot be resolved.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile the on-disk `live_on_navigate` golden and return the emitted
/// `main.rs`. `None` (skip) when the embedded runtime cannot be resolved.
// test scaffolding: an ipe-compile failure or a missing emitted file IS the
// failure signal we want to surface loudly.
#[allow(clippy::expect_used)]
fn emit_main_rs() -> Option<String> {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("live_on_navigate")
        .join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("live_on_navigate_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime().ok()?;
    ipe::build(&entry, &out, &runtime).expect("onNavigate routed app must ipe-compile");
    Some(
        std::fs::read_to_string(out.join("src").join("main.rs"))
            .expect("emitted main.rs must exist"),
    )
}

/// The `onNavigate` cfg field makes the runtime `set_page` closure route the
/// matched page through `update` — the URL navigation is a `Msg`, not a magic
/// `page`-field write.
#[test]
fn on_navigate_dispatches_matched_page_through_update() {
    let Some(main_rs) = emit_main_rs() else {
        return;
    };
    assert!(
        main_rs.contains("web_app_routed"),
        "a Model with a `page` field must emit `web_app_routed`",
    );
    // The set_page closure captures update + onNavigate and threads the matched
    // page through `update`, discarding its Cmd (URL reconcile is model-only).
    assert!(
        main_rs.contains("let __on_navigate ="),
        "onNavigate present ⇒ the set_page closure must bind the handler, \
         got:\n{main_rs}",
    );
    assert!(
        main_rs.contains("(__update)((__on_navigate)(__page), __model)"),
        "onNavigate present ⇒ the matched page must flow \
         `update(onNavigate(page), model)`, got:\n{main_rs}",
    );
}

/// The magic-page struct-update closure is the ABSENT-field desugaring only;
/// an app that supplies `onNavigate` must never emit it.
#[test]
fn on_navigate_present_suppresses_magic_page_struct_update() {
    let Some(main_rs) = emit_main_rs() else {
        return;
    };
    assert!(
        !main_rs.contains("{ page: __page, ..__model }"),
        "onNavigate present ⇒ the runtime must NOT struct-update the `page` \
         field directly (that is the absent-field desugaring), got:\n{main_rs}",
    );
}
