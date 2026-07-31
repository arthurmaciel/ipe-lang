//! Gate — row-poly subset/superset record resolution (A7 watch).
//!
//! `docs/adr/0018-row-poly-records-pinned-before-lowering.md` records the
//! verdict: **no defect found**. Every
//! row-polymorphic subset/superset record shape reachable through Ipê's
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
//! `ipe build` / `cargo build` observation, not by assumption.
//!
//! | Fixture | Proof-matrix row(s) | Gate | Asserts |
//! |---|---|---|---|
//! | `row_poly_subset_access` | P2 | accept | unannotated getter, subset field access over a superset arg; emits a concrete (non-generic) getter against the superset struct; `IPE_E2E=1` prints `Ada` |
//! | `row_poly_subset_pattern` | P5, P7 | accept | subset `case` pattern AND subset lambda pattern (through `List.map`) over a superset scrutinee; emitted pattern completes to the superset struct (`RecAgeName { age: _, name, .. }`); `IPE_E2E=1` prints `Iri: Ada, Bo` |
//! | `row_poly_closed_superset_neg` | P4 | reject | CLOSED record annotation called with a superset arg → IPE-T0001 |
//! | `row_poly_two_supersets_neg` | P6 (class-1 tripwire) | reject | unannotated let-bound getter called with two DIFFERENT superset shapes → IPE-T0001 |
//! | `row_poly_annotation_gap` | P1 (gap canary) | reject | row-var record annotation `{ r \| f : T }` parses + type-checks, rejected at lowering → IPE-L0131 (backend monomorphisation deferred) |
//! | `row_poly_accessor` | P8 | accept | first-class accessor `.name` (desugars to `\r -> r.name`) through `List.map` over a superset list; emits a concrete getter closure over the superset struct; `IPE_E2E=1` prints `Ada, Bo` |
//!
//! Run the E2E accept-path bodies (real `cargo build` + run) with:
//! ```text
//! IPE_E2E=1 cargo test -p ipe --test golden_row_poly_records
//! ```
//! The three reject fixtures run unconditionally (compile-time only, no
//! `cargo`, so no gate needed).

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(name: &str) -> PathBuf {
    repo_root().join("tests").join("golden").join(name)
}

fn diag_code(err: &ipe::CliError) -> Option<ipe_diagnostics::Code> {
    match err {
        ipe::CliError::Pipeline { diag, .. } => Some(diag.code()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// row_poly_subset_access — P2: unannotated getter, subset access over a
// superset argument. Accept end-to-end.
// ---------------------------------------------------------------------------

/// ipe-0: the compiler must accept the unannotated `getName rec = rec.name`
/// called with a `{ name, age }` record and emit a CONCRETE getter whose
/// parameter is the resolved superset struct (`RecAgeName`) — proving the
/// A7 exact-sorted-field-set lookup resolves this subset-access shape
/// without a miss. Checked unconditionally (cheap, no `cargo`).
#[test]
fn subset_access_ipec_accepts_and_resolves_superset_struct() {
    let entry = golden_dir("row_poly_subset_access").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("row_poly_subset_access_ipec_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        eprintln!("SKIP row_poly_subset_access: runtime not available");
        return;
    };

    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipe build must accept row_poly_subset_access (P2, subset field \
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
/// field it read. Gated on `IPE_E2E=1` — matches the reference compiler's
/// own output for the identical shape (proof-matrix row P2: "accept; prints
/// `Ada`"), hand-verified against `ipe v0.16.29`.
#[test]
fn subset_access_cargo_builds_and_prints_ada() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let entry = golden_dir("row_poly_subset_access").join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_row_poly_subset_access_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipe build must succeed for row_poly_subset_access: {:?}",
        built.err()
    );

    let outcome = crate::support::build_and_run_emitted("row_poly_subset_access", &out);
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

/// ipe-0: the compiler must accept both a subset `case` pattern (P5) and a
/// subset lambda pattern through a HOF (P7) over a `{ name, age }`
/// scrutinee, and the lowerer must complete each pattern to the superset
/// struct with a `..` rest before the emitter resolves it — the exact
/// `RecAgeName { age: _, name, .. }` shape the design doc predicts.
/// Checked unconditionally (cheap, no `cargo`).
#[test]
fn subset_pattern_ipec_accepts_and_completes_superset_pattern() {
    let entry = golden_dir("row_poly_subset_pattern").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("row_poly_subset_pattern_ipec_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        eprintln!("SKIP row_poly_subset_pattern: runtime not available");
        return;
    };

    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipe build must accept row_poly_subset_pattern (P5 case pattern + \
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
/// values read through both subset patterns. Gated on `IPE_E2E=1` — matches
/// the reference compiler's own output for the identical shapes
/// (proof-matrix rows P5 "prints (same)" and P7 "prints `Ada, Bo`"),
/// hand-verified against `ipe v0.16.29` for this combined fixture
/// (`Iri: Ada, Bo`).
#[test]
fn subset_pattern_cargo_builds_and_prints_iri_ada_bo() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let entry = golden_dir("row_poly_subset_pattern").join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_row_poly_subset_pattern_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipe build must succeed for row_poly_subset_pattern: {:?}",
        built.err()
    );

    let outcome = crate::support::build_and_run_emitted("row_poly_subset_pattern", &out);
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
// Reject, IPE-T0001. Compile-time only, no gate.
// ---------------------------------------------------------------------------

/// A CLOSED record annotation (`{ name : String }`, no row var) cannot
/// absorb a superset argument's extra `age` field — mechanism 1
/// (`unifyRecords`) rejects it as IPE-T0001, in parity with the reference's
/// E2001 for the identical shape.
#[test]
fn closed_superset_is_ipe_t0001() {
    let name = "row_poly_closed_superset_neg";
    let entry = golden_dir(name).join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return;
    };
    let res = ipe::build(&entry, &out, &runtime);
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(ipe_diagnostics::IPE_T0001),
        "closed record annotation vs superset arg must be IPE-T0001; \
         err = {err}"
    );
    assert!(
        !out.join("src").join("main.rs").exists(),
        "{name}: no Rust must be emitted on a rejection"
    );
}

// ---------------------------------------------------------------------------
// row_poly_two_supersets_neg — P6. Reject, IPE-T0001. Compile-time only, no
// gate.
// ---------------------------------------------------------------------------

/// An unannotated LET-BOUND getter called with two DIFFERENT superset
/// shapes (`{ name, age }` then `{ name, id }`) is rejected as IPE-T0001 —
/// neither compiler let-generalizes over record rows. This rejection comes
/// from `unify.rs`'s ordinary closed-record-mismatch rule, via the
/// `Expr_::Let` no-let-polymorphism path (`constrain.rs`) — it does NOT
/// exercise `promote_untyped_boundaries`/Boundary Scheme Promotion, which
/// only generalizes MODULE-level bindings, not local `let`s. Do not read
/// this fixture as a regression gate for the class-1 cross-module
/// generalization mechanism (no cross-module two-superset fixture exists in
/// this repo; a real class-1 coupling tripwire would need one). It DOES still
/// pin the correct
/// no-let-poly-over-rows invariant described above: flipping this fixture
/// to accept without adding per-record-shape callee monomorphisation to the
/// backend would reintroduce the A7 exact-key miss as an ICE (best case) or
/// a seal-violating emitted-code type error. See
/// docs/adr/0018-row-poly-records-pinned-before-lowering.md "Coupling
/// tripwire" for the ORIGINAL (broader) framing of that risk — the fixture
/// here covers only the local-let-binding instance of it.
#[test]
fn two_different_supersets_is_ipe_t0001() {
    let name = "row_poly_two_supersets_neg";
    let entry = golden_dir(name).join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return;
    };
    let res = ipe::build(&entry, &out, &runtime);
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(ipe_diagnostics::IPE_T0001),
        "an unannotated getter used at two different superset shapes must \
         be IPE-T0001 (no let-generalization over rows); err = {err}"
    );
    assert!(
        !out.join("src").join("main.rs").exists(),
        "{name}: no Rust must be emitted on a rejection"
    );
}

// ---------------------------------------------------------------------------
// row_poly_annotation_gap — P1, the completeness-gap canary. Now parses and
// TYPE-CHECKS (the type layer models the open row); rejected at LOWERING,
// IPE-L0131, because the backend cannot yet monomorphise the callee per
// record shape. Compile-time only, no gate.
// ---------------------------------------------------------------------------

/// The row-var record annotation syntax `{ r | name : String }` parses and
/// type-checks — the type layer models the open row and accepts the program.
/// It is rejected at LOWERING with IPE-L0131, because the Rust backend emits
/// one struct per exact field set and cannot yet emit a callee once per
/// record shape at its call sites (per-record-shape callee monomorphisation).
/// The reference parses this, types the row var, and monomorphises the callee
/// per record-shape instantiation in its backend; Ipê fails closed at the
/// lowering boundary — exactly the layer that cannot yet emit — rather than
/// at parse. This test is the canary named in
/// docs/adr/0018-row-poly-records-pinned-before-lowering.md "Gap filed": the
/// front half of the chain (parser + AST + canon + `from_canon` + type layer)
/// is now in place; the deferred backend monomorphisation is what IPE-L0131
/// still gates. It MUST start failing (and force a re-read of that section)
/// the moment the backend gains per-record-shape monomorphisation and the
/// program begins to BUILD.
#[test]
fn row_var_annotation_is_ipe_l0131() {
    let name = "row_poly_annotation_gap";
    let entry = golden_dir(name).join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return;
    };
    let res = ipe::build(&entry, &out, &runtime);
    assert!(res.is_err(), "{name} must fail to compile");
    let Err(err) = res else { return };
    assert_eq!(
        diag_code(&err),
        Some(ipe_diagnostics::IPE_L0131),
        "row-var record annotation `{{ r | f : T }}` must fail closed at \
         lowering with IPE-L0131 (parses + type-checks; backend cannot yet \
         monomorphise the callee per record shape); err = {err}"
    );
    assert!(
        !out.join("src").join("main.rs").exists(),
        "{name}: no Rust must be emitted on a rejection"
    );
}

// ---------------------------------------------------------------------------
// row_poly_accessor — P8: the first-class accessor `.name` as a value.
// Accept end-to-end.
// ---------------------------------------------------------------------------

/// ipe-0: the compiler must accept `List.map .name people` — the first-class
/// accessor `.name` (which desugars to the getter `\r -> r.name`) — and emit a
/// CONCRETE getter closure whose parameter is the resolved superset struct
/// `RecAgeName`, never a generic. This is the accessor form of the mechanism-2
/// deferred field access plus mechanism-4 monomorphic pinning; the record
/// reaches the backend fully pinned so the A7 exact-sorted-field-set lookup
/// resolves it without a miss. Checked unconditionally (cheap, no `cargo`).
#[test]
fn accessor_ipec_accepts_and_resolves_concrete_getter() {
    let entry = golden_dir("row_poly_accessor").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("row_poly_accessor_ipec_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        eprintln!("SKIP row_poly_accessor: runtime not available");
        return;
    };

    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipe build must accept row_poly_accessor (P8, first-class accessor \
         `.name` over a superset record list): {:?}",
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
        emitted.contains("move |ipe_accessor_arg: RecAgeName| -> String"),
        "the accessor `.name` must lower to a CONCRETE getter closure over the \
         resolved superset struct (the A7 exact-key path), not a generic; got \
         main.rs:\n{emitted}"
    );
}

/// cargo-0 ∧ run-0: the emitted project actually compiles (SEAL) and prints the
/// `name` field read off each record through the accessor. Gated on
/// `IPE_E2E=1`.
#[test]
fn accessor_cargo_builds_and_prints_names() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let entry = golden_dir("row_poly_accessor").join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_row_poly_accessor_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipe build must succeed for row_poly_accessor: {:?}",
        built.err()
    );

    let outcome = crate::support::build_and_run_emitted("row_poly_accessor", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "row_poly_accessor binary must exit 0; got {:?}",
        outcome.exit_code
    );
    assert_eq!(
        outcome.stdout, "Ada, Bo\n",
        "must print the `name` fields read through the first-class accessor"
    );
}
