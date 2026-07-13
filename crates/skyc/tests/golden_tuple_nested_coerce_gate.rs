//! #174 fix-up — a coercing leaf (`PStr` / `PList` / `PCons`) nested inside a
//! nested-TUPLE column on a VARIABLE (non-literal) tuple scrutinee must fail
//! closed to SKY-L0115 at `skyc` time — NOT slip through to the coercion-free
//! by-value whole `match` and produce a skyc-0-then-cargo-fail.
//!
//! Adversarial review of `3cc891e` found the non-literal-scrutinee gate inspected
//! only the TOP-LEVEL columns for coercing sub-patterns. A `PStr` / `PCons`
//! nested inside a nested-`PTuple` column — the very shape the by-value path newly
//! admits — slipped through:
//!
//! * PROBE E: `case v of ( ( "x", n ), A ) -> …` on `v : ((String,Int), Tag)` →
//!   emitted `match v { (("x", n), A) => … }` → cargo E0308 (`&str` vs `String`).
//! * PROBE G: `case v of ( ( x :: _, n ), A ) -> …` on `v : ((List Int,Int),Tag)`
//!   → emitted `(([x, ..], n), …)` → cargo E0529 (slice vs `Vec`).
//!
//! The gate now recurses through every structural column (nested tuple / ctor
//! args / alias inner / list & cons elements); a coercing leaf found anywhere
//! fails closed to SKY-L0115 — an honest skyc-fail, exactly as a top-level
//! coercing column already does. This test locks both probes so the hole can
//! never reopen.

use std::path::{Path, PathBuf};

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Build the named golden fixture and assert it surfaces exactly SKY-L0115 as a
/// pipeline diagnostic. A build that SUCCEEDS (the pre-fix-up exit-0 hole), or
/// fails with any other error, makes `got` differ from `Some(SKY_L0115)` and
/// fails with a descriptive message — never a panic. A skip occurs only when the
/// runtime cannot be resolved.
fn assert_l0115_gate(fixture: &str, out_suffix: &str) {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(sky_diagnostics::SKY_L0115),
        "fixture {fixture}: a coercing sub-pattern nested in a nested-tuple column \
         must fail closed to SKY-L0115 (never skyc-0-then-cargo-fail); got build \
         result {built:?}"
    );
}

/// PROBE E: a string literal (`PStr`) nested inside a nested-tuple column.
#[test]
fn nested_tuple_str_column_is_sky_l0115() {
    assert_l0115_gate("i_tuple_nested_coerce_str", "tuple_nested_coerce_str_emit");
}

/// PROBE G: a cons sub-pattern (`PCons`) nested inside a nested-tuple column.
#[test]
fn nested_tuple_cons_column_is_sky_l0115() {
    assert_l0115_gate(
        "i_tuple_nested_coerce_cons",
        "tuple_nested_coerce_cons_emit",
    );
}
