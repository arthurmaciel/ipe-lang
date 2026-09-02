#![forbid(unsafe_code)]
//! The env-var coverage-matrix gate: the env-var surface enumerates every `IPE_*`
//! variable the runtime reads (plus any orphan read), and the aspect columns
//! judge each on registered, read-in-code, documented, truthy-parse-consistent,
//! and prod-safety-gated.
//!
//! This is the env-var sibling of the stdlib coverage matrix: one enumeration,
//! one `surface × aspect` grid, a hole named at its coordinate. The registered
//! and read-in-code columns turn the registry-drift gate into standing cells; the
//! truthy-parse and prod-safety columns surface advisory debt without failing.

use ipe::coverage::contract::{Cell, Surface};
use ipe::coverage::env_surface::{EnvItem, EnvVarSurface};
use ipe::coverage::matrix;

/// Allowlisted holes: a `(aspect, variable, reason)` coordinate that is a known,
/// tracked gap rather than fresh drift. Empty means every column is green over
/// the whole env-var surface.
const ALLOWLIST: &[(&str, &str, &str)] = &[];

#[test]
fn env_surface_is_non_empty() {
    let items = EnvVarSurface.all();
    assert!(
        !items.is_empty(),
        "the env-var surface must enumerate at least one variable",
    );
}

#[test]
fn env_surface_is_deterministic_and_sorted() {
    let first = EnvVarSurface.all();
    let second = EnvVarSurface.all();
    let names_first: Vec<String> = first.iter().map(|i| i.name().to_owned()).collect();
    let names_second: Vec<String> = second.iter().map(|i| i.name().to_owned()).collect();
    assert_eq!(
        names_first, names_second,
        "two enumerations must be byte-identical"
    );
    let mut sorted = names_first.clone();
    sorted.sort();
    assert_eq!(names_first, sorted, "the surface must be name-sorted");
}

#[test]
fn a_known_variable_is_registered_on_the_surface() {
    let items = EnvVarSurface.all();
    let known = items
        .iter()
        .find(|i| i.name() == "IPE_WEB_PORT")
        .expect("IPE_WEB_PORT must appear on the env-var surface");
    assert!(
        matches!(known, EnvItem::Registered(_)),
        "IPE_WEB_PORT must be a registered entry, not an orphan read",
    );
}

#[test]
fn env_columns_pass_over_the_whole_surface() {
    use std::fmt::Write as _;

    let report = matrix::run_env();

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
        "the env-var coverage columns must pass over the whole surface (or be \
         recorded in the allowlist with a tracking reason):\n{unexpected}\n\
         (allowlist has {} entr(y/ies))",
        ALLOWLIST.len(),
    );
}

#[test]
fn allowlisted_holes_are_still_real() {
    let report = matrix::run_env();
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

#[test]
fn advisories_are_reported_not_failing() {
    // The truthy-parse and prod-safety columns emit advisories (Warn), which the
    // runner collects without failing the gate. This pins that a Warn never
    // becomes a hole and that at least the dev-only vars surface as advisories.
    let report = matrix::run_env();
    assert!(
        report.passed(),
        "advisories must not fail the gate: {}",
        report.render()
    );
    assert!(
        report
            .advisories
            .iter()
            .any(|a| a.aspect == "prod-safety-gated"),
        "at least one dev-only variable must surface a prod-safety advisory",
    );
}

#[test]
fn registered_column_flags_an_orphan_read() {
    // A synthetic orphan is a hole on the registered column and not-applicable on
    // the registry-only columns.
    use ipe::coverage::columns_env::RegisteredColumn;
    use ipe::coverage::contract::AspectCheck;
    let orphan = EnvItem::OrphanRead("IPE_DEFINITELY_NOT_REGISTERED_PROBE".to_owned());
    let col = RegisteredColumn;
    assert!(
        matches!(col.check(&orphan), Cell::Hole(_)),
        "an orphan read must be a hole on the registered column",
    );
}
