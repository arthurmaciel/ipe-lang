//! Milestone-5 pipe-operator gate: `|>` (forward, left-associative, prec 0)
//! and `<|` (backward, right-associative, prec 0) desugar to `Call` nodes in
//! the canonicaliser and produce byte-identical `main.rs` output to the
//! checked-in goldens.
//!
//! Five programs exercise the surface end to end:
//!
//! * `m5pipe_chain`          — multi-step forward pipe prints `3`.
//! * `m5pipe_backward`       — `<|` right-assoc + looser-than-`+` prints `3`.
//! * `m5pipe_multiline`      — leading-`|>` continuation lines in a `let` binding.
//! * `m5pipe_prec_vs_arith`  — `2 + 3 |> String.fromInt` groups `+` first → `"5"`.
//! * `m5pipe_mixed_append`   — `"a" ++ "b" |> String.toUpper` groups `++` first → `"AB"`.
//!
//! Behavioural-parity oracle: `oracle_divergence = false` for all five — the Go
//! backend produces identical stdout (`3`, `3`, `3`, `5`, `AB`).

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

fn assert_byte_identical(name: &str) {
    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.sky");
    let golden = dir.join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_emit"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"));
    let want = std::fs::read_to_string(&golden);
    assert!(emitted.is_ok() && want.is_ok(), "both files must read");
    assert_eq!(
        emitted.ok(),
        want.ok(),
        "emitted main.rs for {name} must equal the golden byte-for-byte"
    );
}

fn assert_runs_and_matches_oracle(name: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

// ── byte-identical emit tests (always run) ────────────────────────────────────

#[test]
fn pipe_chain_emits_byte_identical_main_rs() {
    assert_byte_identical("m5pipe_chain");
}

#[test]
fn pipe_backward_emits_byte_identical_main_rs() {
    assert_byte_identical("m5pipe_backward");
}

#[test]
fn pipe_multiline_emits_byte_identical_main_rs() {
    assert_byte_identical("m5pipe_multiline");
}

#[test]
fn pipe_prec_vs_arith_emits_byte_identical_main_rs() {
    assert_byte_identical("m5pipe_prec_vs_arith");
}

#[test]
fn pipe_mixed_append_emits_byte_identical_main_rs() {
    assert_byte_identical("m5pipe_mixed_append");
}

// ── E2E oracle tests (gated on SKY_E2E=1) ────────────────────────────────────

#[test]
fn pipe_chain_builds_and_prints_three() {
    assert_runs_and_matches_oracle("m5pipe_chain");
}

#[test]
fn pipe_backward_builds_and_prints_three() {
    assert_runs_and_matches_oracle("m5pipe_backward");
}

#[test]
fn pipe_multiline_builds_and_prints_three() {
    assert_runs_and_matches_oracle("m5pipe_multiline");
}

#[test]
fn pipe_prec_vs_arith_builds_and_prints_five() {
    assert_runs_and_matches_oracle("m5pipe_prec_vs_arith");
}

#[test]
fn pipe_mixed_append_builds_and_prints_ab() {
    assert_runs_and_matches_oracle("m5pipe_mixed_append");
}
