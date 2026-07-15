//! M4a stdlib-foundation parity gate: the Prelude-exposed built-in
//! constructors (`True` / `False`, `Just` / `Nothing`, `Ok` / `Err`) usable as
//! VALUE expressions and in `case` patterns (closing SKY-N0001 / SKY-N0003), and
//! the built-in `List` type with `[]` / `[a, b, c]` literals and the `::` cons
//! operator.
//!
//! Four programs exercise the surface end to end:
//!
//! * `m4a_maybe` — `case (Just 42) of Just n -> n ; Nothing -> 0`, prints `42`.
//!   `Maybe a` lowers to the runtime's `SkyMaybe<T>`; the construction and the
//!   pattern both route to it.
//! * `m4a_result` — `case (Ok 7) of Ok n -> n ; Err _ -> 0`, prints `7`.
//!   `Result e a` lowers to the runtime's `SkyResult<E, A>`.
//! * `m4a_bool` — `if True then "yes" else "no"`, prints `yes`. `True` / `False`
//!   lower to the Rust `true` / `false` keyword constants.
//! * `m4a_list` — `1 :: 2 :: [3, 4, 5]` built and passed to a function, prints
//!   `7`. The list literal lowers to `vec![…]` and `::` to the runtime's
//!   `sky_list_cons`, over the runtime's `Vec<T>` list representation.
//!
//! Each emitted `main.rs` must be byte-identical to the checked-in golden, and
//! (behind `SKY_E2E=1`) the emitted project must build and print the value the
//! Go reference compiler produces — captured in each golden's `expected_go.txt`
//! / `oracle.meta` via the cached-oracle infra (no live Go in this gate).

use std::path::{Path, PathBuf};

mod support;

use support::repo_root;

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Compile `tests/golden/<name>/Main.sky` and assert the emitted `src/main.rs`
/// equals the checked-in `tests/golden/<name>/main.rs` byte-for-byte.
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

    // Directory-diff the emitted project against the golden dir (byte-compares
    // the emitted `src/main.rs` against the golden `main.rs`). Replaces the
    // former hand-rolled `read_to_string` + `assert_eq!` pair with the shared
    // harness helper.
    support::assert_emitted_project_matches_golden_dir(
        &out,
        support::golden_dir_of(&golden),
    );
}

/// Full spine: compile, build the emitted Cargo project, run it, and assert its
/// stdout matches the golden's CACHED Go oracle via the staleness-gated
/// `support::assert_go_parity` — NO live Go run. Gated on `SKY_E2E=1`.
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

#[test]
fn maybe_value_and_pattern_emit_byte_identical_main_rs() {
    assert_byte_identical("m4a_maybe");
}

#[test]
fn result_value_and_pattern_emit_byte_identical_main_rs() {
    assert_byte_identical("m4a_result");
}

#[test]
fn bool_value_emits_byte_identical_main_rs() {
    assert_byte_identical("m4a_bool");
}

#[test]
fn list_literal_and_cons_emit_byte_identical_main_rs() {
    assert_byte_identical("m4a_list");
}

#[test]
fn maybe_builds_and_prints_forty_two() {
    assert_runs_and_matches_oracle("m4a_maybe");
}

#[test]
fn result_builds_and_prints_seven() {
    assert_runs_and_matches_oracle("m4a_result");
}

#[test]
fn bool_builds_and_prints_yes() {
    assert_runs_and_matches_oracle("m4a_bool");
}

#[test]
fn list_builds_and_prints_seven() {
    assert_runs_and_matches_oracle("m4a_list");
}
