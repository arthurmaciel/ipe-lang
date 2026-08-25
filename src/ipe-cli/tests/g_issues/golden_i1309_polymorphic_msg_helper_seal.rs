//! SEAL regression: unconstrained-message defaulting must NOT over-fire.
//!
//! `sharedRow : Html msg` is a message-free zero-argument helper — the same
//! shape as the defect's `page` — but it is used at TWO different concrete
//! message types (`viewA : Html MsgA`, `viewB : Html MsgB`). Each use pins `msg`
//! to a concrete `Msg`, so the variable is genuinely polymorphic and must emit a
//! real `fn shared_row<T1>() -> Html<T1>`. `wrap : Html msg -> Html msg` threads
//! its `msg` through a parameter — another shape that stays generic.
//!
//! If defaulting fired here it would emit `Html<()>` where `viewA` needs
//! `Html<MsgA>` — an E0308 SEAL break in the opposite direction. This fixture
//! locks in that a pinned message variable keeps its generic and still builds.

use std::path::{Path, PathBuf};

use crate::support::repo_root;

const GOLDEN: &str = "polymorphic_msg_helper_seal";

fn fixture_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(GOLDEN)
        .join("Main.ipe")
}

/// Emit gate + byte-identity golden: the genuinely-polymorphic helpers must be
/// accepted and emit the checked-in `main.rs` byte-for-byte (the `<T1>` generics
/// are locked in — defaulting did not collapse them to `()`).
#[test]
fn polymorphic_msg_helper_emits_byte_identical() {
    let root = repo_root();
    let entry = fixture_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join(GOLDEN)
        .join("main.rs");
    let out = std::env::temp_dir().join("ipec_i1309_polymorphic_msg_emit");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // resolver unavailable — skip
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "a message-polymorphic helper used at two concrete msg types must keep \
         its generic and emit, got: {built:?}"
    );

    crate::support::assert_emitted_project_matches_golden_dir(
        &out,
        crate::support::golden_dir_of(&golden),
    );
}

/// THE SEAL: under `IPE_E2E=1`, actually `cargo build` the emitted crate — a
/// genuine `Html<T1>` helper used at two concrete msg types compiles.
#[test]
fn polymorphic_msg_helper_seal_builds() {
    let root = repo_root();
    let entry = fixture_entry(&root);
    let out = std::env::temp_dir().join("ipec_i1309_polymorphic_msg_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "{GOLDEN} must be accepted, got: {built:?}");

    crate::support::assert_seal_builds(GOLDEN, &out);
}
