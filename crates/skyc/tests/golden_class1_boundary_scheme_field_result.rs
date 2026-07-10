//! Class-1 inference bug #2 ("Boundary Scheme Promotion") — regression for a
//! real SEAL violation found by independent adversarial review after the fix
//! first briefly landed as commit `29bab0d` (same-day reverted at `5e870b4`).
//!
//! Pre-fix: `skyc build` exits 0, but the emitted Rust fails
//! `cargo build` with:
//!
//! ```text
//! error[E0283]: type annotations needed
//!     cannot infer type of the type parameter `T1` declared on the function
//!     `lib1_get_name`
//!     = note: cannot satisfy `_: Clone`
//! ```
//!
//! Root cause (`promote_untyped_boundaries` in
//! `crates/sky_types/src/constrain.rs`): an unannotated top-level getter like
//! `getName r = r.name` registers a deferred `FieldAccess { record, result,
//! .. }` obligation. The obligation-gate `obligation_roots` set excluded
//! `fa.record` from quantification (so the record parameter correctly stays
//! monomorphic on a single-record-type cross-module use) but did NOT exclude
//! `fa.result` — the access's own result var, which is ALSO `getName`'s
//! return-type var. At the point `promote_untyped_boundaries` runs (BEFORE
//! `resolve_deferred`), `fa.result` is still an unresolved plain-`Flex` root,
//! so it got incorrectly quantified into `getName`'s scheme. By lowering
//! time `resolve_deferred` has already pinned it to the concrete field type
//! (`String`), so the quantified symbol never appears in the def's resolved
//! `params`/`ret` — yet `SolvedTypes::untyped_type_params` still listed it,
//! and the untyped lowering arm blindly turned every entry in that list into
//! a Rust generic. The result: a `Func::type_params` entry naming a generic
//! that the emitted signature never uses, so every call site fails E0283
//! ("cannot infer type of the type parameter").
//!
//! Fix (two parts, per BACKLOG.md's "Boundary Scheme Promotion" row):
//! 1. `crates/sky_types/src/constrain.rs`'s `promote_untyped_boundaries` now
//!    also inserts `fa.result` into `obligation_roots` alongside `fa.record`
//!    — the actual root-cause fix.
//! 2. `crates/sky_lower/src/lower.rs`'s untyped lowering arm now ports the
//!    Typed arm's `used_generics` structural-appearance filter (the
//!    Bug-28/Bug-29 invariant): `type_params` only ever contains a symbol
//!    that structurally appears in the RESOLVED `params`/`ret`, so a stale
//!    quantified var can never be declared as an unused Rust generic again
//!    — defense-in-depth for the same class of gap, independent of whether
//!    every `obligation_roots` case has been (or ever will be) fully
//!    enumerated.
//!
//! This test is deliberately an ACTUAL `cargo build` (gated `SKY_E2E=1`),
//! not just a `sky_types` unit test — the prior attempt's own unit-test
//! matrix (including
//! `obligation_gated_untyped_def_single_record_type_cross_module_use_accepted`)
//! all passed despite the bug, because the bug is a codegen-level defect
//! (an unused Rust generic) invisible to `sky_types`'s own HM-soundness
//! checks. Only a real `rustc` invocation on the emitted project catches it.
//!
//! The fixture (`tests/golden/class1_boundary_scheme_field_result/src/`) is a
//! genuine 3-module project:
//! - `Lib1.sky` — the unannotated cross-module getter (`getName r = r.name`).
//! - `Lib2.sky` — the record type (`Person`) and a sample value, used at
//!   exactly ONE concrete type from `Main` (the shape the obligation-gate
//!   fallback is supposed to keep monomorphic on the record parameter).
//! - `Main.sky` — calls `Lib1.getName Lib2.samplePerson` and prints the
//!   result.
//!
//! Run:
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_class1_boundary_scheme_field_result
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// skyc-0: the compiler must accept the 3-module program and emit a
/// concrete (non-generic) `lib1_get_name`. Checked unconditionally (cheap,
/// no `cargo`), independent of the `SKY_E2E` gate below.
#[test]
fn class1_field_result_skyc_accepts_and_emits_concrete_getter() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("class1_boundary_scheme_field_result")
        .join("src")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("class1_boundary_scheme_field_result_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP class1_boundary_scheme_field_result: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for class1_boundary_scheme_field_result: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    // The getter must lower to a CONCRETE signature. Pre-fix this line read
    // `pub fn lib1_get_name<T1: Clone>(r: RecAgeName) -> String {` — an
    // unused Rust generic that made every call site fail E0283.
    assert!(
        emitted.contains("pub fn lib1_get_name(r: RecAgeName) -> String {")
            || emitted.contains("pub fn lib1_get_name(r: RecAgeName) -> String{"),
        "getName must lower to a concrete (non-generic) signature; got main.rs:\n{emitted}"
    );
    assert!(
        !emitted.contains("lib1_get_name<"),
        "getName must NOT carry a spurious type parameter (the E0283 SEAL \
         violation this test guards); got main.rs:\n{emitted}"
    );
}

/// cargo-0 ∧ run-0: the emitted project actually compiles with `rustc` and
/// prints the field it read. Gated on `SKY_E2E=1` — a real `cargo build`,
/// the only check that would have caught the original SEAL violation (every
/// `sky_types` unit test in the prior attempt passed despite the bug).
#[test]
fn class1_field_result_cargo_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("class1_boundary_scheme_field_result")
        .join("src")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_class1_boundary_scheme_field_result_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for class1_boundary_scheme_field_result: {:?}",
        built.err()
    );

    // cargo-0 ∧ run-0: `build_and_run_emitted` fails the test loudly (with
    // cargo's own stderr) on any build failure — this is the exact gate the
    // prior attempt's own verify note ("not just the gate") was supposed to
    // be, and the one that actually caught the E0283 regression on review.
    let outcome = support::build_and_run_emitted("class1_boundary_scheme_field_result", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "class1_boundary_scheme_field_result binary must exit 0; got {:?}",
        outcome.exit_code
    );
    assert!(
        outcome.stdout.contains("Ada"),
        "must print the `name` field read via the cross-module getter; got: {:?}",
        outcome.stdout
    );
}
