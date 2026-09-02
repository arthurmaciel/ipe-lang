#![forbid(unsafe_code)]
//! The diagnostic coverage-matrix gate: the diagnostic surface enumerates every
//! code in [`ipe_diagnostics::ALL_CODES`] and the aspect columns judge each
//! on `has-conforming-explain-page`, `documented`, and `refusal-tested`.
//!
//! This surface mechanises the "prove the refusals" principle: every rejection
//! path the compiler can take must have a standing test that drives it. A code
//! with no such test is a Hole — a rejection one edit can delete unnoticed.
//!
//! The [`ALLOWLIST`] records every known, tracked gap with a reason. An
//! allowlisted entry that is no longer a Hole fails the gate (stale allowlist);
//! an unexpected Hole that is not listed also fails. Both directions must be
//! clean.

use ipe::coverage::contract::Surface;
use ipe::coverage::diagnostic_surface::DiagnosticSurface;
use ipe::coverage::matrix;

/// Known, tracked gaps in the `refusal-tested` column.
///
/// Each entry is `(aspect, code-wire-string, reason)`. A code here is a real,
/// acknowledged gap — the test does not yet exist; it is NOT a signal that the
/// rejection path is unsound. Add a negative-suite test and remove the entry
/// when the gap is closed.
///
/// Codes are grouped by family and reason for clarity.
const ALLOWLIST: &[(&str, &str, &str)] = &[
    // ── IPE-F#### (FFI) ─────────────────────────────────────────────────────
    // These three require network failure or an FFI cache I/O fault — conditions
    // that cannot be reliably induced in an offline unit test.
    (
        "refusal-tested",
        "IPE-F4411",
        "git-source rejection needs a real network/VCS fixture",
    ),
    (
        "refusal-tested",
        "IPE-F4412",
        "FFI cache I/O fault is not reproducible in a unit test",
    ),
    (
        "refusal-tested",
        "IPE-F4414",
        "asserted FFI call refusal covered by golden_ffi_asserted_call_seal (IPE-F4414 literal absent from that file)",
    ),
    // ── IPE-I#### (internal compiler errors / ICEs) ──────────────────────────
    // ICE and intern codes are invariant violations that require deliberate
    // compiler corruption to trigger from user source. They cannot be reached
    // through normal user input — a test that produces them would break the
    // compiler-internal invariant being guarded.
    (
        "refusal-tested",
        "IPE-I0010",
        "intern: unresolved symbol requires deliberate interner corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0011",
        "intern: symbol table exhausted requires deliberate interner corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0100",
        "ICE: match on unknown variant requires compiler-internal corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0101",
        "ICE: duplicate match arm requires compiler-internal corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0102",
        "ICE: non-exhaustive match requires compiler-internal corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0103",
        "ICE: match arm enum mismatch requires compiler-internal corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0200",
        "ICE: no Rust name for symbol requires compiler-internal corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0201",
        "ICE: dangling value/variant symbol requires compiler-internal corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0202",
        "ICE: cross-module type-name collision requires compiler-internal corruption",
    ),
    (
        "refusal-tested",
        "IPE-I0203",
        "ICE: golden anchor missing requires compiler-internal corruption",
    ),
    // ── IPE-L#### (lowering / not-yet-supported) ────────────────────────────
    // The L-family represents features not yet emittable. Many of these are
    // exercised by compiler-internal unit tests that assert the code constant
    // without using the quoted wire string format the surface scan requires.
    // Each should be migrated to a negative-suite or golden test.
    (
        "refusal-tested",
        "IPE-L0100",
        "pattern kind gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0101",
        "operator gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0102",
        "polymorphic value gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0103",
        "function-valued parameter gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0104",
        "Task () gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0105",
        "parameter destructuring gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0106",
        "top-level function signature gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0108",
        "kernel not-available gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0110",
        "partial application gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0111",
        "generic record update gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0112",
        "nested constructor pattern gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0113",
        "constructor-as-fn gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0114",
        "function-in-constructor gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0115",
        "tuple pattern gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0116",
        "refutable-discrimination gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0118",
        "Web.appRouted gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0121",
        "succeed arity gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0122",
        "Web.route param-count gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0123",
        "Web.route builder gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0124",
        "Web.app no-page field gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0127",
        "function-value reuse gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0129",
        "wasm routed gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0133",
        "CustomElement not-emittable gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0143",
        "row-generic field type mismatch gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0144",
        "row-generic non-record gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0145",
        "Store.eq field accessor gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0146",
        "Store accessor point-free gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0147",
        "Ui.widget browser-only gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0149",
        "Store.select projection gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0150",
        "committed string literal Secret gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0151",
        "Secret.fromString point-free gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0153",
        "Ui.cells Cli shape gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-L0200",
        "deep-nesting gate tested via compiler unit test; needs negative-suite migration",
    ),
    // ── IPE-N#### (name resolution) ─────────────────────────────────────────
    (
        "refusal-tested",
        "IPE-N0021",
        "import cycle detection tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0023",
        "module path mismatch tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0024",
        "ambiguous import tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0025",
        "reserved namespace tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0030",
        "wasm server-module reachability tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0033",
        "Program managed-loop shape gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0036",
        "removed surface gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0037",
        "reserved JS-interop type gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0041",
        "Ipe.Codec.auto derivation gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0043",
        "config binding not threaded gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-N0045",
        "runtime shape selection gate tested via compiler unit test; needs negative-suite migration",
    ),
    // ── IPE-P#### (parse) ────────────────────────────────────────────────────
    (
        "refusal-tested",
        "IPE-P0018",
        "accessor-space gate tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-P0066",
        "doc-string on non-exported binding is a warning (not a hard rejection); needs a warning-surface test",
    ),
    (
        "refusal-tested",
        "IPE-P0067",
        "exported binding has no doc-string is a warning (not a hard rejection); needs a warning-surface test",
    ),
    // ── IPE-T#### (type) ────────────────────────────────────────────────────
    (
        "refusal-tested",
        "IPE-T0003",
        "type inference step budget tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-T0011",
        "redundant case branch tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-T0016",
        "async carrier arity tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-T0019",
        "or-pattern variable binding tested via compiler unit test; needs negative-suite migration",
    ),
    (
        "refusal-tested",
        "IPE-T0020",
        "Html-vs-Element type gate tested via compiler unit test; needs negative-suite migration",
    ),
];

#[test]
fn diagnostic_surface_is_non_empty() {
    let items = DiagnosticSurface.all();
    assert!(
        !items.is_empty(),
        "the diagnostic surface must enumerate at least one code",
    );
}

#[test]
fn diagnostic_surface_matches_all_codes_length() {
    use ipe_diagnostics::ALL_CODES;
    let items = DiagnosticSurface.all();
    assert_eq!(
        items.len(),
        ALL_CODES.len(),
        "the diagnostic surface must enumerate exactly ALL_CODES.len() items",
    );
}

#[test]
fn diagnostic_surface_is_deterministic() {
    let first: Vec<String> = DiagnosticSurface
        .all()
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();
    let second: Vec<String> = DiagnosticSurface
        .all()
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();
    assert_eq!(
        first, second,
        "two enumerations of the diagnostic surface must be byte-identical",
    );
}

#[test]
fn a_known_code_appears_on_the_surface() {
    use ipe_diagnostics::IPE_T0001;
    let items = DiagnosticSurface.all();
    assert!(
        items.contains(&IPE_T0001),
        "IPE-T0001 must appear on the diagnostic surface",
    );
}

#[test]
fn diagnostic_columns_pass_with_allowlist() {
    use std::fmt::Write as _;

    let report = matrix::run_diagnostic();

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
        "the diagnostic coverage columns must pass over the whole surface (or be \
         recorded in the allowlist with a tracking reason):\n{unexpected}\n\
         (allowlist has {} entr(y/ies))",
        ALLOWLIST.len(),
    );
}

#[test]
fn allowlisted_holes_are_still_real() {
    let report = matrix::run_diagnostic();
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
fn explain_page_column_passes_for_all_codes() {
    // Every code must have a conforming explain page — this column is never
    // allowlisted, so a hole here is always an immediate failure.
    let report = matrix::run_diagnostic();
    let page_holes: Vec<_> = report
        .holes
        .iter()
        .filter(|h| h.aspect == "has-conforming-explain-page")
        .collect();
    assert!(
        page_holes.is_empty(),
        "every code must have a conforming explain page:\n{}",
        page_holes.iter().fold(String::new(), |mut s, h| {
            use std::fmt::Write as _;
            let _ = writeln!(s, "  HOLE [{}] {}: {}", h.aspect, h.symbol, h.message);
            s
        }),
    );
}
