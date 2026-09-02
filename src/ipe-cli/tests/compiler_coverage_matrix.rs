#![forbid(unsafe_code)]
//! The compiler-crate coverage-matrix gate: the compiler surface enumerates
//! every crate under `src/compiler/`, and the aspect columns judge each on
//! tested, no-panic, and documented.
//!
//! This is the compiler sibling of the env-var and stdlib coverage matrices:
//! one enumeration, one `surface × aspect` grid, a hole named at its
//! coordinate. The ALLOWLIST records every known gap with a tracking reason so
//! the gate is green while the debt is visible.

use ipe::coverage::compiler_surface::CompilerSurface;
use ipe::coverage::contract::Surface;
use ipe::coverage::matrix;

/// Allowlisted holes: `(aspect, crate_name, reason)` triples for known,
/// tracked gaps. Remove an entry once the gap is fixed; the
/// `allowlisted_holes_are_still_real` test will catch stale entries.
const ALLOWLIST: &[(&str, &str, &str)] = &[
    // ── no-panic gaps ────────────────────────────────────────────────────────
    // Each crate below has unwrap/expect/panic!/index in its production source.
    // These are soundness debt (PRINCIPLES §3) tracked here until fixed.
    (
        "no-panic",
        "ipe_annotate",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_backend",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_canon",
        "production source contains unwrap/expect/panic — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_diagnostics",
        "production source contains expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_ffi",
        "production source contains unwrap/expect/panic — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_intern",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_ir",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_lint",
        "production source contains expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_lower",
        "production source contains unwrap/expect/panic — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_parse",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_sandbox",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_types",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    (
        "no-panic",
        "ipe_watch",
        "production source contains unwrap/expect — tracked soundness debt",
    ),
    // ── tested gaps ──────────────────────────────────────────────────────────
    (
        "tested",
        "ipe_path_core",
        "path-core has no #[test] in src/ or tests/ — add tests for path helpers",
    ),
    // ── documented gaps ──────────────────────────────────────────────────────
    (
        "documented",
        "ipe_intern",
        "intern src/lib.rs has no //! crate-level doc comment — add one",
    ),
];

#[test]
fn compiler_surface_is_non_empty() {
    let items = CompilerSurface.all();
    assert!(
        !items.is_empty(),
        "the compiler surface must enumerate at least one crate",
    );
}

#[test]
fn compiler_surface_is_deterministic_and_sorted() {
    let first = CompilerSurface.all();
    let second = CompilerSurface.all();
    let names_first: Vec<&str> = first.iter().map(|c| c.name).collect();
    let names_second: Vec<&str> = second.iter().map(|c| c.name).collect();
    assert_eq!(
        names_first, names_second,
        "two enumerations must be identical",
    );
    let mut sorted = names_first.clone();
    sorted.sort_unstable();
    assert_eq!(
        names_first, sorted,
        "the surface must be sorted by crate name"
    );
}

#[test]
fn a_known_crate_appears_on_the_surface() {
    let items = CompilerSurface.all();
    assert!(
        items.iter().any(|c| c.name == "ipe_parse"),
        "ipe_parse must appear on the compiler surface",
    );
}

#[test]
fn compiler_columns_pass_over_the_whole_surface() {
    use std::fmt::Write as _;

    let report = matrix::run_compiler();

    let mut unexpected = String::new();
    for h in report.holes.iter().filter(|h| {
        !ALLOWLIST
            .iter()
            .any(|(aspect, symbol, _)| *aspect == h.aspect && *symbol == h.symbol)
    }) {
        let _ = writeln!(
            unexpected,
            "  HOLE [{}] {}: {}",
            h.aspect, h.symbol, h.message
        );
    }

    assert!(
        unexpected.is_empty(),
        "compiler coverage columns must pass over the whole surface (or be \
         recorded in the allowlist with a tracking reason):\n{unexpected}\n\
         (allowlist has {} entr(y/ies))",
        ALLOWLIST.len(),
    );
}

#[test]
fn allowlisted_holes_are_still_real() {
    let report = matrix::run_compiler();
    for (aspect, symbol, reason) in ALLOWLIST {
        let present = report
            .holes
            .iter()
            .any(|h| h.aspect == *aspect && h.symbol == *symbol);
        assert!(
            present,
            "allowlisted hole [{aspect}] {symbol} ({reason}) is no longer \
             reported — remove the stale allowlist entry",
        );
    }
}
