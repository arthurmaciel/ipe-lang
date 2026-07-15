//! SKY-L0105 by-VALUE alias binding — the partial-move (E0382) SEAL regression.
//!
//! An `as`-alias over a destructure (`(a, b) as whole`) in a by-VALUE binding
//! position binds BOTH the whole and its sub-parts. Rust's `name @ (a, b)`
//! spelling moves them from the SAME value — a partial move (E0382) for any
//! non-`Copy` payload. Before the fix, `((a, b) as whole)` over `(String,
//! String)` emitted `let whole @ (a, b) = arg;` and was `skyc`-0 then
//! `cargo`-101. The fix binds the whole first and destructures the sub-parts
//! from a CLONE, so every binder is independently owned:
//!
//! ```ignore
//! let whole = arg;
//! let (a, b) = whole.clone();
//! ```
//!
//! The fixture exercises ALL FOUR by-value binding shapes with the non-`Copy`
//! `(String, String)` payload, every binder used in the body:
//!
//! * PARAM — `f ((a, b) as whole) = …` (a #96 function parameter pattern).
//! * CASE — a single-arm product `case`, an irrefutable destructure.
//! * LET — an irrefutable `let` destructure with an alias.
//! * NESTED — `(h, ((c, d) as inner))`, an alias nested inside a tuple, driving
//!   the fresh-temp (`__sky_bind_N`) path.
//!
//! Two locks:
//!
//! 1. `skyc` emits `main.rs` byte-identical to the checked-in golden — which
//!    records that NO by-value alias renders as `name @ inner`; every alias
//!    binds the whole then clones the sub-shape.
//! 2. Behind `SKY_E2E=1` the emitted project BUILDS (the seal: was `cargo`-101)
//!    and prints `pqpqrsrstutuhcdcd`, exit 0 — proving whole AND parts are live.

use std::path::{Path, PathBuf};

mod support;

use support::repo_root;

fn fixture_dir(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("l0105_alias_move_seal")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = fixture_dir(&root).join("Main.sky");
    let golden = fixture_dir(&root).join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("l0105_alias_move_seal_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    // Directory-diff the emitted project against the golden dir (byte-compares
    // the emitted `src/main.rs` against the golden `main.rs`). Replaces the
    // former hand-rolled `read_to_string` + `assert_eq!` pair with the shared
    // harness helper.
    support::assert_emitted_project_matches_golden_dir(
        &out,
        support::golden_dir_of(&golden),
    );
}

/// The seal invariant asserted independently of the byte lock: NO by-value
/// alias binding renders as Rust's moving `name @ (…)` subpattern, and every
/// alias binds the whole then clones the destructured sub-shape.
#[test]
fn no_by_value_alias_uses_at_subpattern() {
    let root = repo_root();
    let entry = fixture_dir(&root).join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("l0105_alias_move_seal_shapes");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());
    let src = std::fs::read_to_string(out.join("src").join("main.rs")).expect("main.rs");

    // The partial-move spelling must NOT appear in any by-value binding. `@ (`
    // (binding-with-tuple-subpattern) is exactly the E0382 shape the seal
    // removes; a legitimate slice rest binder is `@ ..`, never `@ (`.
    assert!(
        !src.contains("@ ("),
        "a by-value alias must never render as the moving `name @ (…)`:\n{src}"
    );
    // Top-level alias → bind whole, then clone the sub-shape.
    assert!(
        src.contains("let whole = arg_0; let (a, b) = whole.clone();"),
        "PARAM alias must bind the whole then clone-destructure:\n{src}"
    );
    // Nested alias inside a tuple → fresh temps, then clone the inner shape.
    assert!(
        src.contains("let (__sky_bind_0, __sky_bind_1) = arg_1;")
            && src.contains("let inner = __sky_bind_1; let (c, d) = inner.clone();"),
        "NESTED alias must bind fresh temps then clone the inner shape:\n{src}"
    );
}

/// Full spine: build the emitted Cargo project, run it, assert it prints
/// `pqpqrsrstutuhcdcd`, exit 0. Gated on `SKY_E2E=1`. This is the SEAL: before
/// the fix the by-value alias binding was `skyc`-0 then `cargo`-101 (E0382);
/// the clean build + correct output prove whole AND parts are independently
/// owned and live.
#[test]
fn end_to_end_builds_and_prints_the_concatenation() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let root = repo_root();
    let entry = fixture_dir(&root).join("Main.sky");
    let out = std::env::temp_dir().join("skyc_l0105_alias_move_seal_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("l0105_alias_move_seal", &out);
    assert_eq!(
        outcome.stdout.trim_end(),
        "pqpqrsrstutuhcdcd",
        "concatenated whole+parts across PARAM/CASE/LET/NESTED positions"
    );
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "clean exit — the E0382 seal holds"
    );
}
