//! Regression guard for the SEAL run-output binary-isolation invariant.
//!
//! A SEAL test that RUNS an emitted binary and asserts its stdout/exit MUST
//! execute a binary at a path no concurrent test can write. Every emitted crate
//! builds a fixed-name `ipe-app`, so the shared warm target
//! (`IPE_ORACLE_SHARED_TARGET`) holds a single `debug/ipe-app` that parallel
//! nextest clobbers — a run-output test resolving to the shared target can read a
//! sibling's binary (a false red, or the worse false green that masks an emit
//! bug). `seal_e2e::emitted_run_target_dir` therefore ALWAYS returns the isolated
//! per-slot `out/target`, never the shared path, and consults no environment at
//! all. This test pins both facts structurally — no env mutation, so it is
//! deterministic under parallel nextest — so a future edit rerouting the
//! run-output helper onto the shared warm target breaks the build here, not just
//! a clobbered assertion two steps downstream.

use std::path::Path;

mod seal_e2e;

/// The run-output target is the isolated per-slot `out/target`, and never any
/// path an ambient shared warm target could name.
#[test]
fn run_target_is_isolated_never_shared() {
    // The helper is a pure function of `out_dir`: it must return the isolated
    // per-slot `out/target` regardless of any ambient `IPE_ORACLE_SHARED_TARGET`.
    // Asserted for several distinct slots so the guarantee is the per-slot join,
    // not a coincidental constant.
    for slot in [
        "/tmp/ipe-seal-guard-a",
        "relative/slot-b",
        "/var/tmp/slot-c",
    ] {
        let out = Path::new(slot);
        assert_eq!(
            seal_e2e::emitted_run_target_dir(out),
            out.join("target"),
            "run-output target must be the isolated per-slot out/target"
        );
    }

    // The build-only helper is the one allowed to follow the shared warm target;
    // the run-output helper must NOT agree with it whenever a shared path is set.
    // Compare against a representative absolute shared target: the run-output
    // path must differ, i.e. it never hands a run-output test the clobberable
    // shared `debug/ipe-app`.
    let out = Path::new("/tmp/ipe-seal-guard-shared-slot");
    let shared = Path::new("/tmp/ipe-oracle-shared-target");
    assert_ne!(
        seal_e2e::emitted_run_target_dir(out),
        shared.to_path_buf(),
        "run-output target must never resolve to the shared warm target"
    );
}
