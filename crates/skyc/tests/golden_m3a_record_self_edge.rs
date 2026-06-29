//! Milestone-3a recursion-soundness gate: a self-edge routed through a RECORD
//! payload. `skyc` must emit `main.rs` byte-identical to the checked-in golden,
//! and (behind `SKY_E2E=1`) the emitted project must build and print `5`.
//!
//! ```text
//! type RChain = REnd | RNode { rest : RChain, val : Int }
//! ```
//!
//! `RNode`'s payload is the record `{ rest : RChain, val : Int }`, whose `rest`
//! field reaches `RChain` again — the type-size cycle `RChain -> RecRestVal ->
//! RChain` is closed *through a record*. The pre-fix backend boxed only a direct
//! self-edge, so it emitted `RNode(RecRestVal)` with `RecRestVal { rest:
//! MainRChain, .. }` — mutually infinite-sized Rust types (E0072): `skyc` exited
//! 0 and the crate then failed `cargo build`. The fix boxes the cyclic
//! enum-payload edge (`RNode(Box<RecRestVal>)`), which breaks the cycle without
//! touching the record struct, balanced by `Box::new` at construction and a
//! deref at pattern binding (`let rec = *rec;`).
//!
//! Note: the Go reference parser does NOT accept a record type as a constructor
//! payload, so there is no Go oracle for this exact source — `skyc` accepts a
//! superset here. The in-test hand-computed `5` (`3 + 2`) is the oracle, and the
//! gate's load-bearing assertion is that the emitted crate BUILDS (no E0072) and
//! runs, pinning the indirect-cycle soundness floor so it can never regress to
//! the silent exit-0-then-cargo-fail mode.
use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3a_record_self_edge")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m3a_record_self_edge")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3a_record_self_edge_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"));
    let want = std::fs::read_to_string(&golden);
    assert!(emitted.is_ok() && want.is_ok(), "both files must read");
    assert_eq!(
        emitted.ok(),
        want.ok(),
        "emitted main.rs must equal the golden byte-for-byte"
    );
}

/// Full spine: compile, build the emitted Cargo project, run it, and assert the
/// record-self-edge ADT program prints `5`. Gated on `SKY_E2E=1`. This is the
/// soundness-floor regression for a self-edge through a record: before the fix
/// the crate did not build at all (E0072).
#[test]
fn end_to_end_builds_and_prints_five() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m3a_record_self_edge_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m3a_record_self_edge", &out);
    assert_eq!(
        outcome.stdout, "5\n",
        "program prints 5 (hand-computed oracle: 3 + 2)"
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0");
}
