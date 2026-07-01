//! Parser-gaps golden gate — build+run fixtures for the parser features whose
//! coverage previously stopped at parse-level unit tests. Non-negotiable #9
//! ("every new feature becomes a regression test") requires a build+run golden
//! per feature, not just an AST-shape assertion; this file closes that gap for
//! five surfaces.
//!
//! ## Positives (build → run → byte-diff against the cached oracle)
//!
//! Each single-file positive compiles `tests/golden/<name>/Main.sky` through
//! `skyc`, builds the emitted Rust project in the shared cargo target, runs it,
//! and checks stdout against the cached Go oracle (`oracle.meta` +
//! `expected_go.txt`). All three carry `oracle_divergence = false` — the Go
//! reference compiler produces byte-identical stdout, captured via
//! `refresh-oracle`.
//!
//! * `m2lex_intdiv` — `//` integer division. `20 // 2` → `10` and `(-7) // 2` →
//!   `-3` (truncation toward zero, Go + Elm parity; a floor-division backend
//!   would give `-4`). Output: `"10 -3"`.
//! * `m1_let_fn` — a let-bound *function* (`inc n = n + 1`) applied inside the
//!   `in` body. Output: `"30"`.
//! * `m0_blockcomment` — a `{- … -}` block comment containing an em-dash plus a
//!   nested `{- {- -} -}` comment; both are skipped and the program prints.
//!   Output: `"ok"`.
//!
//! The multi-module positive uses `skyc::build_project` (a `sky.toml` workspace
//! with a `Lib` module) then runs the emitted binary:
//!
//! * `mm_qualtype` — `Lib` exposes `type Box a = Box a`; `Main` annotates a
//!   binding with the *qualified* type name `Lib.Box Int` and unwraps it.
//!   Output: `"42"` (Go-verified). Multi-module goldens keep their source under
//!   `src/`, so the single-file oracle-cache layout does not apply; the run
//!   asserts the literal Go-verified stdout instead.
//!
//! ## Negatives (assert the exact `SKY-*` diagnostic / runtime classification)
//!
//! * `m2lex_intdiv_divzero` — `10 // 0` at runtime. The runtime classifies this
//!   as `DivisionByZero` and aborts with exit code 101 (a Rust panic), rather
//!   than emitting a wrong answer. Asserted by building + running the emitted
//!   binary and checking the exit code (gated on `SKY_E2E`).
//! * `m0_blockcomment_unterminated` — a `{-` that is never closed → `SKY-P0017`
//!   (unterminated block comment). This is a sanctioned stricter behaviour: the
//!   Go oracle leniently swallows the unterminated comment to EOF and builds an
//!   empty program, so there is no oracle to diff — the diagnostic code is the
//!   assertion.
//! * `mm_neg_qualtype_unknownmod` — an annotation referencing `Bogus.Box`, a
//!   module that is neither imported nor present → `SKY-N0004` (unknown module).
//!
//! Run the E2E positives + the runtime negative with:
//!
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_parser_gaps
//! ```
//!
//! The compile-time negatives run unconditionally (no build/run, so no gate).

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(name: &str) -> PathBuf {
    repo_root().join("tests").join("golden").join(name)
}

// `runtime()` is a non-`#[test]` helper, so clippy's test-only `expect`
// exemption does not apply automatically. The explicit allow is correct: a
// broken toolchain environment should fail loudly here, not silently skip.
#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    skyc::resolve_runtime().expect("runtime must resolve for golden_parser_gaps tests")
}

// ---------------------------------------------------------------------------
// Single-file positive: build → run → cached-oracle byte-diff (SKY_E2E-gated).
// ---------------------------------------------------------------------------

fn assert_single_oracle(name: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let dir = golden_dir(name);
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let built = skyc::build(&entry, &out, &runtime());
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

#[test]
fn intdiv_builds_and_matches_oracle() {
    assert_single_oracle("m2lex_intdiv");
}

#[test]
fn let_fn_builds_and_matches_oracle() {
    assert_single_oracle("m1_let_fn");
}

#[test]
fn blockcomment_builds_and_matches_oracle() {
    assert_single_oracle("m0_blockcomment");
}

// ---------------------------------------------------------------------------
// Multi-module positive: build_project → run → literal Go-verified stdout.
// ---------------------------------------------------------------------------

#[test]
fn qualtype_project_builds_and_prints_42() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let name = "mm_qualtype";
    let dir = golden_dir(name);
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let built = skyc::build_project(&dir.join("sky.toml"), &out, &runtime());
    assert!(
        built.is_ok(),
        "build_project failed for {name}: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted(name, &out);
    // Go-verified reference: `unbox (Box 42)` prints `42`. Multi-module goldens
    // store source under `src/`, so the single-file oracle cache does not apply.
    assert_eq!(outcome.stdout, "42\n", "qualified `Lib.Box Int` must yield 42");
    assert_eq!(outcome.exit_code, Some(0), "clean exit");
}

// ---------------------------------------------------------------------------
// Runtime negative: `//` division by zero → DivisionByZero, exit 101.
// ---------------------------------------------------------------------------

#[test]
fn intdiv_by_zero_aborts_exit_101() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let name = "m2lex_intdiv_divzero";
    let dir = golden_dir(name);
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    // Codegen succeeds — the failure is a *runtime* classification, not a
    // compile error.
    let built = skyc::build(&entry, &out, &runtime());
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    assert_eq!(
        outcome.exit_code,
        Some(101),
        "division by zero must abort with the Rust panic exit code, not print a value"
    );
    assert_eq!(
        outcome.stdout, "",
        "no value is printed once DivisionByZero fires"
    );
}

// ---------------------------------------------------------------------------
// Compile-time negatives: exact SKY-* diagnostic codes (always run).
// ---------------------------------------------------------------------------

fn diag_code(err: &skyc::CliError) -> Option<sky_diagnostics::Code> {
    match err {
        skyc::CliError::Pipeline { diag, .. } => Some(diag.code()),
        _ => None,
    }
}

#[test]
fn unterminated_blockcomment_is_sky_p0017() {
    let name = "m0_blockcomment_unterminated";
    let entry = golden_dir(name).join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}"));
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build(&entry, &out, &runtime());
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(sky_diagnostics::SKY_P0017),
        "unterminated `{{-` must be SKY-P0017; err = {err}"
    );
}

#[test]
fn unknown_module_in_annotation_is_sky_n0004() {
    let name = "mm_neg_qualtype_unknownmod";
    let dir = golden_dir(name);
    let out = std::env::temp_dir().join(format!("skyc_{name}"));
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&dir.join("sky.toml"), &out, &runtime());
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(sky_diagnostics::SKY_N0004),
        "`Bogus.Box` must be SKY-N0004 (unknown module); err = {err}"
    );
}
