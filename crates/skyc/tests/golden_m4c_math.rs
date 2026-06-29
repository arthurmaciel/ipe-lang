//! M4c Sky.Core.Math parity gate: `Math.min` / `Math.max` are polymorphic
//! `comparable` (`a -> a -> a`, Elm `Basics.min`/`max` semantics) and compile +
//! run with the CORRECT result — comparing at the argument's actual type, never
//! routing through an `Int` coercion.
//!
//! Semantic invariants verified here:
//!
//! * `Math.min 3 5` → `3` — integers. This shape is TRUE byte parity with the
//!   Go reference: Go's `Math_min` compares via `AsInt`, which is correct for
//!   `Int`, so the cached oracle is the Go output (`oracle_divergence = false`).
//! * `Math.min 0.4 1.3` → `0.4` — floats. Current Go truncates BOTH arguments
//!   through `AsInt` before comparing (`AsInt 0.4 = 0`, `AsInt 1.3 = 1`), a
//!   documented Go bug (anzellai/sky PR #136, OPEN). Sky-Rust compares the
//!   `f64`s directly and returns the lesser value unchanged. Recorded as a
//!   `go-bug:` divergence; it re-converges to byte parity once #136 merges and
//!   the vendored Go syncs.
//! * `Math.max "apple" "banana"` → `"banana"` — strings. Go's `AsInt` compare is
//!   meaningless on `String`; the correct behaviour is the lexicographic
//!   ordering. Also a `go-bug:` divergence.
//!
//! The float / string goldens therefore assert against Sky-Rust's OWN recorded
//! (correct) output, the int golden against the Go oracle. Every test is gated
//! on `SKY_E2E=1`; without it the test returns early. Run with:
//!
//! ```text
//! SKY_E2E=1 SKY_RUNTIME_DIR=<path-to-runtime-rust/src/sky_runtime> \
//!     cargo test golden_m4c_math
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Compile `tests/golden/<name>/Main.sky`, build the emitted Cargo project,
/// run it, and assert its stdout matches the cached oracle (the Go reference
/// for the parity case, or Sky-Rust's own recorded-correct output for a
/// `go-bug:` divergence). Gated on `SKY_E2E=1`.
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
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the oracle");
}

// ── min — Int (TRUE Go parity) ────────────────────────────────────────────────

/// `Math.min 3 5` → `3`. Integer compare matches Go's `AsInt` path exactly.
#[test]
fn math_min_int() {
    assert_runs_and_matches_oracle("m4c_math_min_int");
}

// ── min — Float (go-bug #136 divergence: no truncation) ───────────────────────

/// `Math.min 0.4 1.3` → `0.4`. The Go reference truncates the compare to ints
/// (`0 < 1`) — we compare the `f64`s and return `0.4`.
#[test]
fn math_min_float_no_truncation() {
    assert_runs_and_matches_oracle("m4c_math_min_float");
}

// ── max — String (go-bug #136 divergence: lexicographic) ──────────────────────

/// `Math.max "apple" "banana"` → `"banana"`. Polymorphic compare on `String`
/// (lexicographic), where Go's `AsInt` compare is meaningless.
#[test]
fn math_max_string_lexicographic() {
    assert_runs_and_matches_oracle("m4c_math_max_string");
}
