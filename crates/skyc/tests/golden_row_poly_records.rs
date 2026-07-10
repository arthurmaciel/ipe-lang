//! #56 gate — row-poly subset/superset record resolution (A7 watch).
//!
//! `docs/architecture/row-poly-subset-superset-design.md` records the
//! 2026-07-10 investigation's verdict: **no defect found**. Every
//! row-polymorphic subset/superset record shape reachable through ipê's
//! surface today either resolves end-to-end in parity with the reference
//! compiler, or is rejected with the same verdict the reference gives
//! (fail-loud parity). This file wires the spec's 5-fixture proof matrix
//! into the sweep so a future change — most plausibly the class-1
//! "Boundary Scheme Promotion" generalization work — cannot silently widen
//! the A7 exact-sorted-field-set struct registry's reachable-shape
//! invariant without a test noticing.
//!
//! No compiler code change accompanies this file: it is verification +
//! hardening, matching the spec's proof-matrix rows P1 through P7 by direct
//! `skyc build` / `cargo build` observation, not by assumption.
//!
//! | Fixture | Proof-matrix row(s) | Gate | Asserts |
//! |---|---|---|---|
//! | `row_poly_subset_access` | P2 | accept | unannotated getter, subset field access over a superset arg; emits a concrete (non-generic) getter against the superset struct; `SKY_E2E=1` prints `Ada` |
//! | `row_poly_subset_pattern` | P5, P7 | accept | subset `case` pattern AND subset lambda pattern (through `List.map`) over a superset scrutinee; emitted pattern completes to the superset struct (`RecAgeName { age: _, name, .. }`); `SKY_E2E=1` prints `Iri: Ada, Bo` |
//! | `row_poly_closed_superset_neg` | P4 | reject | CLOSED record annotation called with a superset arg → SKY-T0001 |
//! | `row_poly_two_supersets_neg` | P6 (class-1 tripwire) | reject | unannotated let-bound getter called with two DIFFERENT superset shapes → SKY-T0001 |
//! | `row_poly_annotation_gap` | P1 (gap canary, #56b) | reject | row-var record annotation `{ r \| f : T }` does not parse → SKY-P0001 |
//!
//! Run the E2E accept-path bodies (real `cargo build` + run) with:
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_row_poly_records
//! ```
//! The three reject fixtures run unconditionally (compile-time only, no
//! `cargo`, so no gate needed).

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(name: &str) -> PathBuf {
    repo_root().join("tests").join("golden").join(name)
}

fn diag_code(err: &skyc::CliError) -> Option<sky_diagnostics::Code> {
    match err {
        skyc::CliError::Pipeline { diag, .. } => Some(diag.code()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// row_poly_subset_access — P2: unannotated getter, subset access over a
// superset argument. Accept end-to-end.
// ---------------------------------------------------------------------------

/// skyc-0: the compiler must accept the unannotated `getName rec = rec.name`
/// called with a `{ name, age }` record and emit a CONCRETE getter whose
/// parameter is the resolved superset struct (`RecAgeName`) — proving the
/// A7 exact-sorted-field-set lookup resolves this subset-access shape
/// without a miss. Checked unconditionally (cheap, no `cargo`).
#[test]
fn subset_access_skyc_accepts_and_resolves_superset_struct() {
    let entry = golden_dir("row_poly_subset_access").join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("row_poly_subset_access_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP row_poly_subset_access: runtime not available");
        return;
    };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must accept row_poly_subset_access (P2, subset field \
         access over a superset record): {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    assert!(
        emitted.contains("struct RecAgeName"),
        "the superset struct RecAgeName must be resolved and emitted; got \
         main.rs:\n{emitted}"
    );
    assert!(
        emitted.contains("pub fn main_get_name(rec: RecAgeName) -> String"),
        "getName must lower to a concrete getter over the superset struct \
         (the A7 exact-key resolution path), not a generic; got \
         main.rs:\n{emitted}"
    );
}

/// cargo-0 ∧ run-0: the emitted project actually compiles and prints the
/// field it read. Gated on `SKY_E2E=1` — matches the reference compiler's
/// own output for the identical shape (proof-matrix row P2: "accept; prints
/// `Ada`"), hand-verified against `sky v0.16.29` during the #56
/// investigation and re-confirmed for this exact fixture.
#[test]
fn subset_access_cargo_builds_and_prints_ada() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let entry = golden_dir("row_poly_subset_access").join("Main.sky");
    let out = std::env::temp_dir().join("skyc_row_poly_subset_access_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for row_poly_subset_access: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("row_poly_subset_access", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "row_poly_subset_access binary must exit 0; got {:?}",
        outcome.exit_code
    );
    assert_eq!(
        outcome.stdout, "Ada\n",
        "must print the `name` field read through the subset access, \
         matching the reference oracle"
    );
}

// ---------------------------------------------------------------------------
// row_poly_subset_pattern — P5 + P7: subset case/lambda record patterns
// over a superset scrutinee. Accept end-to-end.
// ---------------------------------------------------------------------------

/// skyc-0: the compiler must accept both a subset `case` pattern (P5) and a
/// subset lambda pattern through a HOF (P7) over a `{ name, age }`
/// scrutinee, and the lowerer must complete each pattern to the superset
/// struct with a `..` rest before the emitter resolves it — the exact
/// `RecAgeName { age: _, name, .. }` shape the design doc predicts.
/// Checked unconditionally (cheap, no `cargo`).
#[test]
fn subset_pattern_skyc_accepts_and_completes_superset_pattern() {
    let entry = golden_dir("row_poly_subset_pattern").join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("row_poly_subset_pattern_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP row_poly_subset_pattern: runtime not available");
        return;
    };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must accept row_poly_subset_pattern (P5 case pattern + \
         P7 lambda-through-HOF pattern, both subset over a superset \
         scrutinee): {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    let completed_pattern_count = emitted.matches("RecAgeName { age: _, name, .. }").count();
    assert!(
        completed_pattern_count >= 2,
        "both the case pattern (P5) and the lambda pattern (P7) must \
         complete to the superset struct pattern `RecAgeName {{ age: _, \
         name, .. }}` — the A7 exact-set resolution path for record \
         PATTERNS, not just record access; found {completed_pattern_count} \
         occurrence(s) in main.rs:\n{emitted}"
    );
}

/// cargo-0 ∧ run-0: the emitted project actually compiles and prints the
/// values read through both subset patterns. Gated on `SKY_E2E=1` — matches
/// the reference compiler's own output for the identical shapes
/// (proof-matrix rows P5 "prints (same)" and P7 "prints `Ada, Bo`"),
/// hand-verified against `sky v0.16.29` during the #56 investigation and
/// re-confirmed for this exact combined fixture (`Iri: Ada, Bo`).
#[test]
fn subset_pattern_cargo_builds_and_prints_iri_ada_bo() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let entry = golden_dir("row_poly_subset_pattern").join("Main.sky");
    let out = std::env::temp_dir().join("skyc_row_poly_subset_pattern_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for row_poly_subset_pattern: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("row_poly_subset_pattern", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "row_poly_subset_pattern binary must exit 0; got {:?}",
        outcome.exit_code
    );
    assert_eq!(
        outcome.stdout, "Iri: Ada, Bo\n",
        "must print the leader's name (case pattern) followed by the \
         mapped names (lambda pattern through List.map), matching the \
         reference oracle"
    );
}

// ---------------------------------------------------------------------------
// row_poly_closed_superset_neg — P4: closed annotation vs superset arg.
// Reject, SKY-T0001. Compile-time only, no gate.
// ---------------------------------------------------------------------------

/// A CLOSED record annotation (`{ name : String }`, no row var) cannot
/// absorb a superset argument's extra `age` field — mechanism 1
/// (`unifyRecords`) rejects it as SKY-T0001, in parity with the reference's
/// E2001 for the identical shape.
#[test]
fn closed_superset_is_sky_t0001() {
    let name = "row_poly_closed_superset_neg";
    let entry = golden_dir(name).join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return;
    };
    let res = skyc::build(&entry, &out, &runtime);
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(sky_diagnostics::SKY_T0001),
        "closed record annotation vs superset arg must be SKY-T0001; \
         err = {err}"
    );
    assert!(
        !out.join("src").join("main.rs").exists(),
        "{name}: no Rust must be emitted on a rejection"
    );
}

// ---------------------------------------------------------------------------
// row_poly_two_supersets_neg — P6, the class-1 coupling tripwire. Reject,
// SKY-T0001. Compile-time only, no gate.
// ---------------------------------------------------------------------------

/// An unannotated LET-BOUND getter called with two DIFFERENT superset
/// shapes (`{ name, age }` then `{ name, id }`) is rejected as SKY-T0001 —
/// neither compiler let-generalizes over record rows. THIS is the tripwire
/// the class-1 "Boundary Scheme Promotion" work must respect: see
/// docs/architecture/row-poly-subset-superset-design.md "Coupling
/// tripwire" — flipping this fixture to accept without adding
/// per-record-shape callee monomorphisation to the backend reintroduces the
/// A7 exact-key miss as an ICE (best case) or a seal-violating emitted-code
/// type error.
#[test]
fn two_different_supersets_is_sky_t0001() {
    let name = "row_poly_two_supersets_neg";
    let entry = golden_dir(name).join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return;
    };
    let res = skyc::build(&entry, &out, &runtime);
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(sky_diagnostics::SKY_T0001),
        "an unannotated getter used at two different superset shapes must \
         be SKY-T0001 (no let-generalization over rows); err = {err}"
    );
    assert!(
        !out.join("src").join("main.rs").exists(),
        "{name}: no Rust must be emitted on a rejection"
    );
}

// ---------------------------------------------------------------------------
// row_poly_annotation_gap — P1, the #56b completeness-gap canary. Reject,
// SKY-P0001. Compile-time only, no gate.
// ---------------------------------------------------------------------------

/// The row-var record annotation syntax `{ r | name : String }` does not
/// parse — SKY-P0001 ("found `|`, expected `:`"). Filed as backlog row
/// `#56b` (Post-completion, corpus-unused, non-sweep-blocking): the
/// reference parses this, types the row var, and monomorphises the callee
/// per record-shape instantiation in its backend; ipê's backend cannot yet
/// do the per-shape monomorphisation the syntax would require, so the
/// syntax stays fail-closed at parse rather than accepting a program the
/// backend cannot emit. This test is the canary named in
/// docs/architecture/row-poly-subset-superset-design.md "Gap filed" — it
/// MUST start failing (and force a re-read of that section) the moment the
/// syntax begins to parse.
#[test]
fn row_var_annotation_is_sky_p0001() {
    let name = "row_poly_annotation_gap";
    let entry = golden_dir(name).join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return;
    };
    let res = skyc::build(&entry, &out, &runtime);
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(sky_diagnostics::SKY_P0001),
        "row-var record annotation `{{ r | f : T }}` must be SKY-P0001 \
         (unsupported syntax, fail-closed) until the backend gains \
         per-record-shape callee monomorphisation; err = {err}"
    );
    assert!(
        !out.join("src").join("main.rs").exists(),
        "{name}: no Rust must be emitted on a rejection"
    );
}
