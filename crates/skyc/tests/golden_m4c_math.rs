//! M4c `Sky.Core.Math` parity gate — two classes of golden in one file:
//!
//! 1. **Go-bug divergence** (anzellai/sky PR #136, OPEN). `Math.min` / `Math.max`
//!    are polymorphic `comparable` (`a -> a -> a`, Elm `Basics.min`/`max`). The
//!    current Go reference routes BOTH arguments through `AsInt` before the
//!    compare, which truncates floats (`AsInt 0.4 = 0`, `AsInt 1.3 = 1`) and is
//!    meaningless on `String`. Sky-Rust compares at the argument's actual type
//!    and returns the lesser / greater value UNCHANGED. These goldens therefore
//!    assert against Sky-Rust's OWN recorded-correct output via a
//!    `sanctioned.divergence` marker tagged `go-bug:` — they re-converge to byte
//!    parity once #136 merges and the vendored Go syncs.
//!
//!      * `Math.min 0.4 1.3` → `0.4`   (Go would wrongly give `0`)
//!      * `Math.max 0.4 1.3` → `1.3`   (Go would wrongly give `1`)
//!      * `Math.min "b" "a"` → `"a"`   (lexicographic; Go's `AsInt` is meaningless)
//!      * `Math.max "b" "a"` → `"b"`
//!
//! 2. **Go parity** — the rest. Here the Go reference is correct, so the cached
//!    oracle is the Go output (`oracle_divergence = false`) and Sky-Rust must
//!    match it byte-for-byte: `Math.min` / `Math.max` on `Int` (Go's `AsInt`
//!    path is correct for integers), `abs`, `sqrt` (incl. the `sqrt (-1.0)` NaN
//!    domain edge), `pow`, `round` (half-away-from-zero, both signs), `floor` /
//!    `ceil` / `trunc` on a negative (the three round differently), `mod` vs
//!    `remainder`, and the `pi` / `nan` constants.
//!
//! Every test is gated on `SKY_E2E=1`; without it the test returns early. Run:
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
/// for a parity case, or Sky-Rust's own recorded-correct output for a
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

// ── min / max — Int (TRUE Go parity: AsInt path is correct on integers) ───────

/// `Math.min 3 7` → `3`.
#[test]
fn math_min_int() {
    assert_runs_and_matches_oracle("m4c_math_min_int");
}

/// `Math.max 3 7` → `7`.
#[test]
fn math_max_int() {
    assert_runs_and_matches_oracle("m4c_math_max_int");
}

// ── min / max — Float (go-bug #136 divergence: no truncation) ─────────────────

/// `Math.min 0.4 1.3` → `0.4`. Go truncates the compare to ints (`0 < 1`); we
/// compare the `f64`s and return `0.4` unchanged.
#[test]
fn math_min_float_no_truncation() {
    assert_runs_and_matches_oracle("m4c_math_min_float");
}

/// `Math.max 0.4 1.3` → `1.3`. Go would truncate to `1`; we return `1.3`.
#[test]
fn math_max_float_no_truncation() {
    assert_runs_and_matches_oracle("m4c_math_max_float");
}

// ── min / max — String (go-bug #136 divergence: lexicographic) ────────────────

/// `Math.min "b" "a"` → `"a"`. Polymorphic compare on `String` (lexicographic),
/// where Go's `AsInt` compare is meaningless.
#[test]
fn math_min_string_lexicographic() {
    assert_runs_and_matches_oracle("m4c_math_min_string");
}

/// `Math.max "b" "a"` → `"b"`.
#[test]
fn math_max_string_lexicographic() {
    assert_runs_and_matches_oracle("m4c_math_max_string");
}

// ── abs ──────────────────────────────────────────────────────────────────────

/// `Math.abs (-5)` → `5`. Integer absolute value (`AsInt` is correct here).
#[test]
fn math_abs() {
    assert_runs_and_matches_oracle("m4c_math_abs");
}

// ── sqrt + NaN domain edge ────────────────────────────────────────────────────

/// `Math.sqrt 2.0` → `1.4142135623730951`.
#[test]
fn math_sqrt() {
    assert_runs_and_matches_oracle("m4c_math_sqrt");
}

/// `Math.sqrt (-1.0)` → `NaN`. Domain edge: `string_from_float` special-cases NaN.
#[test]
fn math_sqrt_negative_is_nan() {
    assert_runs_and_matches_oracle("m4c_math_sqrt_neg");
}

// ── pow ──────────────────────────────────────────────────────────────────────

/// `Math.pow 2.0 10.0` → `1024`. Arity-2 exponentiation.
#[test]
fn math_pow() {
    assert_runs_and_matches_oracle("m4c_math_pow");
}

// ── round — half-away-from-zero, both signs ───────────────────────────────────

/// `Math.round 2.5` → `3`. Go `math.Round` rounds halves away from zero.
#[test]
fn math_round_half_away() {
    assert_runs_and_matches_oracle("m4c_math_round");
}

/// `Math.round (-2.5)` → `-3`. Half-away-from-zero on the negative side.
#[test]
fn math_round_negative_half_away() {
    assert_runs_and_matches_oracle("m4c_math_round_neg");
}

// ── floor / ceil / trunc — the three round a negative differently ─────────────

/// `Math.floor (-2.7)` → `-3`. Toward −∞.
#[test]
fn math_floor_negative() {
    assert_runs_and_matches_oracle("m4c_math_floor");
}

/// `Math.ceil (-2.7)` → `-2`. Toward +∞.
#[test]
fn math_ceil_negative() {
    assert_runs_and_matches_oracle("m4c_math_ceil");
}

/// `Math.trunc (-2.7)` → `-2`. Toward zero.
#[test]
fn math_trunc_negative() {
    assert_runs_and_matches_oracle("m4c_math_trunc");
}

// ── mod vs remainder ──────────────────────────────────────────────────────────

/// `Math.mod 7.0 3.0` → `1`. Modulo carrying the dividend's sign (Go `math.Mod`).
#[test]
fn math_mod() {
    assert_runs_and_matches_oracle("m4c_math_mod");
}

/// `Math.remainder 7.0 3.0` → `1`. IEEE 754 remainder (Go `math.Remainder`).
#[test]
fn math_remainder() {
    assert_runs_and_matches_oracle("m4c_math_remainder");
}

// ── constants ─────────────────────────────────────────────────────────────────

/// `Math.pi` → `3.141592653589793`. Zero-arity Float constant.
#[test]
fn math_pi() {
    assert_runs_and_matches_oracle("m4c_math_pi");
}

/// `Math.nan` → `NaN`. `string_from_float` special-cases NaN.
#[test]
fn math_nan() {
    assert_runs_and_matches_oracle("m4c_math_nan");
}
