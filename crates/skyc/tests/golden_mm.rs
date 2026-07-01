//! Multi-module golden tests (Defect-5 mandate).
//!
//! Two kinds of assertion live here:
//!
//! **Positive fixtures** (`mm_local_pkg`, `mm_diamond`) compile successfully
//! and the emitted `main.rs` is byte-identical to the checked-in golden.
//! They also verify that `skyc build_project` doesn't regress on the three
//! blockers that were fixed in this milestone:
//!   * Defect 1 — kernel imports (`Sky.Core.Prelude`) accepted without
//!     SKY-N0020.
//!   * Defect 2 — same-named functions in different modules emit distinct
//!     Rust names (no E0428 from `cargo build`).
//!   * Defect 3 — same-named constructors from two modules trigger clean
//!     SKY-N0024 rather than silently resolving to the wrong type.
//!
//! **Negative fixtures** each contain a deliberate error and assert the
//! exact `Code` produced: `missing` → N0020, `cycle`/`selfimport` → N0021,
//! `notexposed` → N0022, `pathmismatch` → N0023, `ambigval`/`ambigctor`/
//! `samedef` → N0024, `reserved` → N0025, `sametype` → N0012.
//!
//! The golden `main.rs` files were captured by running `skyc` against the
//! fixture and committing the output. Single-file goldens remain
//! byte-identical after this milestone.

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(name: &str) -> PathBuf {
    repo_root().join("tests").join("golden").join(name)
}

// `fn runtime()` is a non-`#[test]` helper, so `allow-expect-in-tests = true`
// in clippy.toml does not exempt it automatically.  The explicit allow is
// correct: this is integration-test scaffolding that should panic loudly on a
// broken environment, and `expect` is the idiomatic way to express that.
#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    skyc::resolve_runtime().expect("runtime must resolve for golden_mm tests")
}

// ---------------------------------------------------------------------------
// Positive: mm_local_pkg
// Local module (Lib) exposes a value + ADT + ctors; Main imports both Lib
// and Sky.Core.Prelude. Exercises Defect-1 fix (kernel skip) and ensures the
// emitted Rust compiles with distinct names.
// ---------------------------------------------------------------------------

#[test]
fn mm_local_pkg_emits_byte_identical_main_rs() {
    let fixture = golden_dir("mm_local_pkg");
    let golden = fixture.join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mm_local_pkg");
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&fixture.join("sky.toml"), &out, &runtime());
    assert!(res.is_ok(), "build_project failed: {:?}", res.err());

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    let want = std::fs::read_to_string(&golden)
        .expect("golden main.rs must exist — regenerate with skyc if missing");
    assert_eq!(
        emitted, want,
        "emitted main.rs must equal the golden byte-for-byte"
    );
}

// ---------------------------------------------------------------------------
// Positive: mm_diamond — A→B, A→C, B→D, C→D. D is compiled exactly once.
// ---------------------------------------------------------------------------

#[test]
fn mm_diamond_emits_byte_identical_main_rs() {
    let fixture = golden_dir("mm_diamond");
    let golden = fixture.join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mm_diamond");
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&fixture.join("sky.toml"), &out, &runtime());
    assert!(res.is_ok(), "build_project failed: {:?}", res.err());

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    let want = std::fs::read_to_string(&golden).expect("golden main.rs must exist");
    assert_eq!(
        emitted, want,
        "emitted main.rs must equal the golden byte-for-byte"
    );

    // D's `base` function must appear exactly once: once compiled, shared by B and C.
    let count = emitted.matches("fn d_base").count();
    assert_eq!(
        count, 1,
        "d_base must appear exactly once in the emitted output (D compiled once)"
    );
}

// ---------------------------------------------------------------------------
// Negative helpers
// ---------------------------------------------------------------------------

fn expect_error_code(fixture_name: &str, expected: sky_diagnostics::Code) {
    let fixture = golden_dir(fixture_name);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(fixture_name);
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&fixture.join("sky.toml"), &out, &runtime());
    assert!(
        res.is_err(),
        "fixture `{fixture_name}` must fail but succeeded"
    );
    let Err(err) = res else { return };
    // Extract the Code from a Pipeline error; other variants have no code.
    let code = match &err {
        skyc::CliError::Pipeline { diag, .. } => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(expected),
        "fixture `{fixture_name}`: expected error code {expected:?}, got {code:?}\nerr = {err}"
    );
}

// ---------------------------------------------------------------------------
// Negative: missing module → SKY-N0020
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_missing_is_sky_n0020() {
    expect_error_code("mm_neg_missing", sky_diagnostics::SKY_N0020);
}

// ---------------------------------------------------------------------------
// Negative: import cycle → SKY-N0021
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_cycle_is_sky_n0021() {
    expect_error_code("mm_neg_cycle", sky_diagnostics::SKY_N0021);
}

// ---------------------------------------------------------------------------
// Negative: self-import → SKY-N0021 (self is a degenerate cycle)
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_selfimport_is_sky_n0021() {
    expect_error_code("mm_neg_selfimport", sky_diagnostics::SKY_N0021);
}

// ---------------------------------------------------------------------------
// Negative: name not exposed → SKY-N0022
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_notexposed_is_sky_n0022() {
    expect_error_code("mm_neg_notexposed", sky_diagnostics::SKY_N0022);
}

// ---------------------------------------------------------------------------
// Negative: module path mismatch → SKY-N0023
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_pathmismatch_is_sky_n0023() {
    expect_error_code("mm_neg_pathmismatch", sky_diagnostics::SKY_N0023);
}

// ---------------------------------------------------------------------------
// Negative: ambiguous value import → SKY-N0024
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_ambigval_is_sky_n0024() {
    expect_error_code("mm_neg_ambigval", sky_diagnostics::SKY_N0024);
}

// ---------------------------------------------------------------------------
// Negative: ambiguous constructor import (Defect-3 gate) → SKY-N0024
// Without the Defect-3 fix this would silently resolve to the wrong type and
// later trigger a Rust E0308 from `cargo build`.
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_ambigctor_is_sky_n0024() {
    expect_error_code("mm_neg_ambigctor", sky_diagnostics::SKY_N0024);
}

// ---------------------------------------------------------------------------
// Negative: reserved namespace (`Sky.*` / `Std.*`) → SKY-N0025
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_reserved_is_sky_n0025() {
    expect_error_code("mm_neg_reserved", sky_diagnostics::SKY_N0025);
}

// ---------------------------------------------------------------------------
// Negative: same-named value exported by two modules, both imported
// unqualified (Defect-2 regression guard — the name-level ambiguity must be
// caught by the canon layer, not survive to produce a Rust E0428) → SKY-N0024
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_samedef_is_sky_n0024() {
    expect_error_code("mm_neg_samedef", sky_diagnostics::SKY_N0024);
}

// ---------------------------------------------------------------------------
// Negative: same type name from two modules both imported → SKY-N0012
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_sametype_is_sky_n0012() {
    expect_error_code("mm_neg_sametype", sky_diagnostics::SKY_N0012);
}

// ---------------------------------------------------------------------------
// Negative: qualified cross-module reference with wrong argument type → SKY-T0001
//
// Lib.helper : Int -> Int; Main calls `Lib.helper "str"` — qualified, so
// SKY-N0024 does not fire.  Before the constrain re-key fix, `top_level` was
// keyed by bare Symbol so a same-named `Main.helper` (if present) would
// overwrite `Lib.helper`'s entry, making skyc exit 0 and emit wrong-type Rust
// that fails only at `cargo build` (E0308).  After the fix, `Lib.helper` is
// looked up under its own `(["Lib"], "helper")` key and the mismatch is
// diagnosed as SKY-T0001 right here, never reaching codegen.
// ---------------------------------------------------------------------------

#[test]
fn mm_neg_qualref_sig_is_sky_t0001() {
    expect_error_code("mm_neg_qualref_sig", sky_diagnostics::SKY_T0001);
}
