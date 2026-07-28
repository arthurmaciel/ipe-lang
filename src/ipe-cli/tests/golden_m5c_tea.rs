//! TEA construct-only gate — `Cmd` / `Sub` construction wiring.
//!
//! These tests compile Ipê programs that construct `Cmd` and `Sub` values and
//! immediately discard them, then print `"ok"`.  This confirms that:
//!
//! * `Cmd.none`, `Cmd.batch`, `Cmd.perform`, `Sub.none`, `Sub.batch`,
//!   `Sub.every`, `Time.every` are wired through the type-checker, lowerer,
//!   and emitter.
//! * The emitted Rust project links against `ipe_runtime::tea` correctly.
//! * Type inference works without explicit annotations when a `Cmd.perform`
//!   or `Sub.every` call anchors the `msg` type parameter.
//! * An **un-anchored** `Cmd.none` / `Sub.none` (free `msg` type variable)
//!   surfaces IPE-L0102 rather than emitting Rust that `cargo build` rejects
//!   with E0282 ("type annotations needed for `tea::IpeCmd<_>`").
//!
//! No TEA dispatch loop is exercised here — that lands in M6.  These are
//! pure construct-and-discard tests whose sole output is `"ok\n"`.
//!
//! Positive tests are gated on `IPE_E2E=1`; gate (error) tests run without it.
//!
//! ## Oracle provenance
//!
//! Marked `oracle_divergence = true` (sanctioned): the Go reference compiler
//! has no equivalent Rust TEA constructors; Ipê-Rust's own output is the
//! authoritative reference.
//!
//! ## Golden catalogue
//!
//! * `perform_ctor` — `Cmd.perform (Task.succeed 1) (\_ -> 0)` discarded;
//!   proves type+emit+link for `cmd_perform`.
//! * `cmd_ctors` — `Cmd.batch [Cmd.perform …, Cmd.none]` discarded; proves
//!   `cmd_none` infers `msg` from a sibling in the batch.
//! * `sub_ctors` — `Sub.every 1000 0` and `Sub.batch [Sub.none, Sub.every
//!   500 1]` discarded; proves `sub_none` infers `msg` from a sibling.
//! * `gate_undetermined_msg` — `let _ = Cmd.none in Io.println "ok"` must
//!   surface IPE-L0102 (free `msg` type variable), never emit cargo-failing Rust.
//!
//! Run:
//!
//! ```text
//! IPE_E2E=1 cargo test golden_m5c_tea
//! ```

use std::path::{Path, PathBuf};

use ipe::CliError;

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Compile `tests/golden/<name>/Main.ipe`, build the emitted Cargo project,
/// run it, and assert its stdout matches the cached oracle.  Gated on
/// `IPE_E2E=1`.
fn assert_runs_and_matches_oracle(name: &str) {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.ipe");
    let out = std::env::temp_dir().join(format!("ipec_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the oracle");
}

// ── Cmd.perform construct-only ────────────────────────────────────────────────

/// `let _ = Cmd.perform (Task.succeed 1) (\_ -> 0) in Io.println "ok"`.
/// Constructs a `IpeCmd<i64>` thunk and discards it; confirms `cmd_perform`
/// wiring through type-checker, lowerer, emitter, and runtime link.
/// Output: `"ok"`.
#[test]
fn perform_ctor() {
    assert_runs_and_matches_oracle("perform_ctor");
}

// ── Cmd.none / Cmd.batch construct-only ──────────────────────────────────────

/// `Cmd.batch [Cmd.perform (Task.succeed 1) (\_ -> 0), Cmd.none]` discarded.
/// `Cmd.none` infers `msg = i64` from the sibling `Cmd.perform` in the batch.
/// Output: `"ok"`.
#[test]
fn cmd_ctors() {
    assert_runs_and_matches_oracle("cmd_ctors");
}

// ── Sub.none / Sub.batch / Sub.every construct-only ──────────────────────────

/// `Sub.every 1000 0` and `Sub.batch [Sub.none, Sub.every 500 1]` discarded.
/// `Sub.none` infers `msg = i64` from the sibling `Sub.every` in the batch.
/// Output: `"ok"`.
#[test]
fn sub_ctors() {
    assert_runs_and_matches_oracle("sub_ctors");
}

// ── Under-determined-msg gate ─────────────────────────────────────────────────

/// Build the named golden fixture and assert it surfaces exactly `expected` as a
/// pipeline diagnostic.  Fails if the build succeeds, or if a different error
/// code is returned.  A skip occurs only when the runtime cannot be resolved.
fn assert_gate(fixture: &str, out_suffix: &str, expected: ipe_diagnostics::Code) {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "fixture {fixture}: expected {expected:?}, got build result {built:?}"
    );
}

/// `let _ = Cmd.none in Io.println "ok"` — `Cmd.none` appears in isolation with
/// no sibling to pin its `msg` type.  The HM solver leaves `msg` as a free
/// type variable; ipe must exit 1 with IPE-L0102, never emit Rust that
/// `cargo build` rejects with E0282 ("type annotations needed for
/// `tea::IpeCmd<_>`").
#[test]
fn undetermined_cmd_none_msg_is_ipe_l0102() {
    assert_gate(
        "gate_undetermined_msg",
        "m5c_gate_undetermined_msg_emit",
        ipe_diagnostics::IPE_L0102,
    );
}
