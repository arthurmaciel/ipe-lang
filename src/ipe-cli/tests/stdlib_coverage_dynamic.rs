#![forbid(unsafe_code)]
//! The stdlib coverage-matrix dynamic gate: the reconciled surface enumerates
//! every exported symbol, and the dynamic aspect columns drive each through the
//! compile stages a registry read cannot reach — lowering, a nested-composition
//! lowering, a real emit → build → run, the runtime symbol's existence, and wasm
//! availability.
//!
//! These columns generate and compile a program per symbol, so the whole file is
//! gated behind `IPE_E2E=1`: without it every test returns early. The `composes`
//! column is the composed-combinator bug-catcher — a higher-order symbol that
//! type-checks under nesting but does not lower is a real lowering gap, surfaced
//! at its coordinate and never weakened into a pass.

use ipe::coverage::contract::{AspectCheck, Cell, StdlibSymbol, Surface};
use ipe::coverage::matrix::{self, run};
use ipe::coverage::surface::StdlibSurface;

/// Whether the heavy end-to-end path is enabled.
fn e2e_enabled() -> bool {
    std::env::var("IPE_E2E").is_ok()
}

/// Run one dynamic column over the whole surface, returning every hole as a
/// `(dotted symbol, message)` pair.
fn holes_of(column: Box<dyn AspectCheck>) -> Vec<(String, String)> {
    let columns = vec![column];
    let report = run(&StdlibSurface, &columns);
    report
        .holes
        .into_iter()
        .map(|h| (h.symbol, h.message))
        .collect()
}

/// The pre-existing-hole allowlist for the dynamic columns. Each entry is
/// `(aspect, dotted-symbol, reason)`: a real gap that predates this gate, recorded
/// with a tracking reason so the column stays meaningful while the gate is not
/// blocked on debt it did not introduce.
const ALLOWLIST: &[(&str, &str, &str)] = &[];

/// Whether a hole coordinate is allowlisted.
fn allowlisted(aspect: &str, symbol: &str) -> bool {
    ALLOWLIST
        .iter()
        .any(|(a, s, _)| *a == aspect && *s == symbol)
}

// ── composes: the composed-combinator lowering bug-catcher ────────────────────

#[test]
fn composes_column_passes_over_every_higher_order_symbol() {
    use std::fmt::Write as _;
    if !e2e_enabled() {
        return;
    }

    let holes = holes_of(Box::new(
        ipe::coverage::columns_runtime::ComposesColumn::new(),
    ));

    let mut unexpected = String::new();
    for (symbol, message) in holes {
        if !allowlisted("composes", &symbol) {
            let _ = writeln!(unexpected, "  HOLE [composes] {symbol}: {message}");
        }
    }

    assert!(
        unexpected.is_empty(),
        "every higher-order stdlib symbol must lower under nesting (a composed \
         combinator that type-checks but does not lower is a real lowering gap):\n\
         {unexpected}",
    );
}

// ── lowers ────────────────────────────────────────────────────────────────────

#[test]
fn lowers_column_passes_over_the_surface() {
    use std::fmt::Write as _;
    if !e2e_enabled() {
        return;
    }

    let holes = holes_of(Box::new(
        ipe::coverage::columns_runtime::LowersColumn::new(),
    ));

    let mut unexpected = String::new();
    for (symbol, message) in holes {
        if !allowlisted("lowers", &symbol) {
            let _ = writeln!(unexpected, "  HOLE [lowers] {symbol}: {message}");
        }
    }

    assert!(
        unexpected.is_empty(),
        "every stdlib symbol whose point-free probe type-checks must lower:\n{unexpected}",
    );
}

// ── runtime-fn-exists + wasm (registry reads; cheap even under E2E) ────────────

#[test]
fn runtime_fn_and_wasm_columns_report_no_holes() {
    use std::fmt::Write as _;
    if !e2e_enabled() {
        return;
    }

    let mut unexpected = String::new();
    for column in [
        Box::new(ipe::coverage::columns_runtime::RuntimeFnExistsColumn::new())
            as Box<dyn AspectCheck>,
        Box::new(ipe::coverage::columns_runtime::WasmColumn),
    ] {
        let aspect = column.name();
        for (symbol, message) in holes_of(column) {
            if !allowlisted(aspect, &symbol) {
                let _ = writeln!(unexpected, "  HOLE [{aspect}] {symbol}: {message}");
            }
        }
    }

    assert!(
        unexpected.is_empty(),
        "the runtime-fn-exists and wasm columns report holes (both emit advisories, \
         not holes, by design — a hole here is a contract break):\n{unexpected}",
    );
}

// ── build+run: heavy (a full cargo build per symbol) ──────────────────────────

/// The build+run column emits, builds, and runs a program per symbol — a full
/// cargo build each — so the whole-surface sweep is the CI job's cost, not a unit
/// test's. This drives a representative slice (the higher-order combinators, the
/// class the composition bug lived in) so a gross regression is caught locally;
/// the full sweep runs in CI.
#[test]
fn build_run_column_over_a_representative_slice() {
    use std::fmt::Write as _;
    if !e2e_enabled() {
        return;
    }

    let column = ipe::coverage::columns_runtime::BuildRunColumn::new();
    let symbols: Vec<StdlibSymbol> = StdlibSurface
        .all()
        .into_iter()
        .filter(|s| s.is_higher_order)
        .collect();

    let mut unexpected = String::new();
    for sym in &symbols {
        let dotted = {
            let mut p = sym.module.join(".");
            if !p.is_empty() {
                p.push('.');
            }
            p.push_str(&sym.name);
            p
        };
        if let Cell::Hole(message) = column.check(sym) {
            if !allowlisted("build+run", &dotted) {
                let _ = writeln!(unexpected, "  HOLE [build+run] {dotted}: {message}");
            }
        }
    }

    assert!(
        unexpected.is_empty(),
        "every higher-order symbol's minimal program must emit, build, and run:\n{unexpected}",
    );
}

// ── the whole dynamic set, for a single-command CI sweep ──────────────────────

/// The whole dynamic column set over the whole surface, as one report. This is the
/// CI sweep entry: heavy (a build per symbol for build+run), so it is E2E-gated
/// like the rest.
#[test]
fn dynamic_columns_pass_over_the_whole_surface() {
    if !e2e_enabled() {
        return;
    }

    let report = matrix::run_dynamic();
    let unexpected: Vec<_> = report
        .holes
        .iter()
        .filter(|h| !allowlisted(h.aspect, &h.symbol))
        .collect();

    assert!(
        unexpected.is_empty(),
        "the dynamic coverage columns must pass over the whole stdlib surface (or \
         be recorded in the allowlist with a tracking reason):\n{}",
        report.render(),
    );
}
