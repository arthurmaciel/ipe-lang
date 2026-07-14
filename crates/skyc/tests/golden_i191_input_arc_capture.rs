//! BACKLOG #191 regression — a non-Copy binding captured into an Input `Arc`
//! callback AND reused by a sibling.
//!
//! Pre-fix: `skyc build` exits 0, but the emitted Rust fails `cargo build` with
//! E0382 ("use of moved value: `habit`"). The lowerer pre-clones the checkbox
//! `onChange` callback's captured `habit`
//! (`let habit = habit.clone(); Lambda { … }`), but `arc_callback_wrap` then
//! re-wrapped the WHOLE block in
//! `Arc::new(move |_x| ({ let habit = habit.clone(); … })(_x))` — the outer
//! `move` still move-captured the FREE outer `habit`, so a later sibling use
//! (`RemoveHabit habit.id`, the button `onPress`) hit use-after-move.
//!
//! Fix (`emit_arc_callback_field` in `crates/sky_backend_rust/src/emit_expr.rs`):
//! hoist the leading capture-clone `let`s OUTSIDE the `Arc`'s `move` closure —
//! `{ let habit = habit.clone(); ::std::sync::Arc::new(move |_x| (INNER)(_x)) }`
//! — so the `Arc` owns the pre-made clone and the original binding survives for
//! the sibling.
//!
//! Run:
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_i191_input_arc_capture
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn entry_path(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("i191_input_arc_capture")
        .join("Main.sky")
}

/// skyc-0: the compiler must accept the program AND emit the capture-clone `let`
/// OUTSIDE the `Arc`'s `move` closure — checked unconditionally (cheap, no
/// `cargo`), independent of the `SKY_E2E` gate. This is the exact assertion that
/// the E0382 SEAL break cannot recur: the pre-clone must sit before `Arc::new`,
/// never inside its `move |_x|` body.
#[test]
fn i191_skyc_accepts_and_hoists_capture_clone() {
    let root = repo_root();
    let entry = entry_path(&root);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i191_input_arc_capture_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP i191_input_arc_capture: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i191_input_arc_capture: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    // The pre-clone must be HOISTED outside the `Arc`'s `move` closure — the
    // `let habit = habit.clone();` immediately precedes `::std::sync::Arc::new`
    // (the #191 fix), so the Arc owns the clone and the original survives.
    assert!(
        emitted.contains("let habit = habit.clone(); ::std::sync::Arc::new"),
        "the capture-clone must be hoisted OUTSIDE the Arc `move` closure (#191); \
         got main.rs:\n{emitted}"
    );
    // Guard against the pre-fix shape: the clone must NOT sit inside the Arc's
    // `move |_x|` body (which would re-move the free outer `habit`).
    assert!(
        !emitted.contains("Arc::new(move |_x| (({ let habit = habit.clone();"),
        "the capture-clone must NOT sit inside the Arc `move` closure (the #191 \
         use-after-move shape); got main.rs:\n{emitted}"
    );
}

/// cargo-0 ∧ run-0: the emitted project actually compiles with `rustc` (no
/// E0382) and renders the row. Gated on `SKY_E2E=1` — the only check that would
/// have caught the original SEAL violation (E0382, `skyc build` clean).
#[test]
fn i191_cargo_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = entry_path(&root);
    let out = std::env::temp_dir().join("skyc_i191_input_arc_capture_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i191_input_arc_capture: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i191_input_arc_capture", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "i191_input_arc_capture binary must exit 0 (no E0382); got {:?} (stdout: {:?})",
        outcome.exit_code,
        outcome.stdout
    );
    // The reused `habit` renders the checkbox label AND the Remove button — proof
    // the binding survived every consuming site.
    assert!(
        outcome.stdout.contains("water") && outcome.stdout.contains("Remove"),
        "must render the row (checkbox label `water` + `Remove` button) through \
         the reused `habit` binding; got: {:?}",
        outcome.stdout
    );
}
