//! M4b Sky.Core.String parity gate: String and Char kernel functions compile
//! and run with Go parity (rune-correct, byte-for-byte output match).
//!
//! M4b surfaces the full `Sky.Core.String` and `Sky.Core.Char` kernel sets,
//! mirroring the Go runtime's `String_*` / `Char_*` helpers. Key semantic
//! invariants verified here:
//!
//! * `String.toUpper "hi"` → `"HI"` — ASCII toUpper.
//! * `String.length "héllo"` → `5` — rune count (not byte count); 'é' is U+00E9,
//!   one Unicode scalar value, so the 2-byte UTF-8 encoding must NOT inflate the
//!   count.
//! * `String.slice 1 3 "hello"` → `"el"` — rune-indexed slice, exclusive end.
//! * `String.split "," "a,b,c" |> List.length` → `3` — split produces three
//!   segments.
//! * `String.join "-" ["a","b"]` → `"a-b"` — join with separator.
//! * `String.dropLeft 2 "héllo"` → `"llo"` — rune-based drop: drops 'h' and 'é'
//!   (each one rune) and returns the three remaining runes.
//!
//! Every test is gated on `SKY_E2E=1`; without it the test returns early (the
//! compile-output gates above it still run). Run with:
//!
//! ```text
//! SKY_E2E=1 SKY_RUNTIME_DIR=<path-to-runtime-rust/src/sky_runtime> \
//!     cargo test golden_m4b_string
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
/// run it, and assert its stdout matches the Go oracle cached in
/// `expected_go.txt` / `oracle.meta`. Gated on `SKY_E2E=1`.
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

// ── toUpper ──────────────────────────────────────────────────────────────────

/// `String.toUpper "hi"` → `"HI"`.
#[test]
fn string_to_upper_hi() {
    assert_runs_and_matches_oracle("m4b_string_to_upper");
}

// ── length — rune count ───────────────────────────────────────────────────────

/// `String.length "héllo"` → `5` (five Unicode scalar values, not 6 bytes).
#[test]
fn string_length_unicode_rune_count() {
    assert_runs_and_matches_oracle("m4b_string_length");
}

// ── slice — rune-indexed ──────────────────────────────────────────────────────

/// `String.slice 1 3 "hello"` → `"el"`.
#[test]
fn string_slice_hello() {
    assert_runs_and_matches_oracle("m4b_string_slice");
}

// ── split + List.length ───────────────────────────────────────────────────────

/// `String.split "," "a,b,c" |> List.length` → `3`.
#[test]
fn string_split_three_segments() {
    assert_runs_and_matches_oracle("m4b_string_split_len");
}

// ── join ──────────────────────────────────────────────────────────────────────

/// `String.join "-" ["a","b"]` → `"a-b"`.
#[test]
fn string_join_with_separator() {
    assert_runs_and_matches_oracle("m4b_string_join");
}

// ── dropLeft — rune-based ─────────────────────────────────────────────────────

/// `String.dropLeft 2 "héllo"` → `"llo"` (drops 'h' and 'é', one rune each).
#[test]
fn string_drop_left_unicode_rune_based() {
    assert_runs_and_matches_oracle("m4b_string_drop_left");
}

// ── Char.toUpper ──────────────────────────────────────────────────────────────

/// `Char.toUpper 'a'` → `"A"`. Returns a single-rune String (Go kernel shape).
#[test]
fn char_to_upper_ascii() {
    assert_runs_and_matches_oracle("m4b_char_to_upper");
}

// ── Char.isDigit ──────────────────────────────────────────────────────────────

/// `Char.isDigit '5'` → `True` (printed via `if` to avoid Bool→String conv).
#[test]
fn char_is_digit_ascii_five() {
    assert_runs_and_matches_oracle("m4b_char_is_digit");
}

// ── Char.isAlpha ──────────────────────────────────────────────────────────────

/// `Char.isAlpha 'x'` → `True`.
#[test]
fn char_is_alpha_ascii_x() {
    assert_runs_and_matches_oracle("m4b_char_is_alpha");
}
