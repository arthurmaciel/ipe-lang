#![forbid(unsafe_code)]
//! The stdlib coverage-matrix gate: the reconciled surface enumerates every
//! exported symbol, and the static aspect columns judge each on home, resolves,
//! closed-scheme, and layer-agreement.
//!
//! This is the spine that turns "no stdlib symbol is forgotten in any aspect"
//! into a checked property: one enumeration, one `surface × aspect` grid, a hole
//! named at its coordinate. A1 pins the surface's facets on a known symbol; A2
//! runs the four static columns over the WHOLE current surface.

use ipe::coverage::contract::{StdlibSymbol, Surface, SymbolKind};
use ipe::coverage::matrix;
use ipe::coverage::surface::StdlibSurface;

/// Find the reconciled symbol for a dotted path (`Ipe.List` + `map`).
fn find<'a>(
    symbols: &'a [StdlibSymbol],
    module: &[&str],
    name: &str,
    kind: SymbolKind,
) -> Option<&'a StdlibSymbol> {
    let module: Vec<String> = module.iter().map(|s| (*s).to_owned()).collect();
    symbols
        .iter()
        .find(|s| s.module == module && s.name == name && s.kind == kind)
}

// ── A1: the surface enumerates, deterministically, with the right facets ──────

#[test]
fn surface_is_non_empty() {
    let symbols = StdlibSurface.all();
    assert!(
        !symbols.is_empty(),
        "the reconciled stdlib surface must enumerate at least one symbol",
    );
}

#[test]
fn surface_is_deterministic_and_sorted() {
    let first = StdlibSurface.all();
    let second = StdlibSurface.all();
    assert_eq!(
        first, second,
        "the surface enumeration must be deterministic across calls",
    );

    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(
        first, sorted,
        "the surface must be sorted by (module, name, kind)",
    );
}

#[test]
fn list_map_has_the_right_facets() {
    let symbols = StdlibSurface.all();
    let map = find(&symbols, &["Ipe", "List"], "map", SymbolKind::Value)
        .expect("Ipe.List.map must be on the surface");

    assert_eq!(map.kind, SymbolKind::Value, "List.map is a value");
    assert!(map.exported, "List.map is in Ipe.List's `exposing (...)`");
    assert!(
        map.has_compiled_source || map.has_kernel,
        "List.map has a home — a compiled-source alias and/or a kernel",
    );
    assert!(
        map.is_higher_order,
        "List.map takes a function `(a -> b)` — it is higher-order",
    );
    assert!(
        map.scheme.is_some(),
        "List.map's scheme projects through the typed interface",
    );
}

// ── A2: the four static columns pass over the WHOLE current surface ───────────

/// The pre-existing-hole allowlist. A real hole the static columns surface that
/// predates this gate is recorded here with a one-line reason and a tracking
/// note, so the column stays meaningful (never weakened) while the gate is not
/// blocked on debt it did not introduce.
///
/// Each entry is `(aspect, dotted-symbol, reason)`. Every allowlisted hole below
/// is a `home` hole for a kernel whose registry qualifier is a short internal tag
/// that maps to no canonical importable module: the user-facing member reconciles
/// through its compiled-source alias (`Ipe.Ui.Cells.cells`, `Ipe.Html.Attributes`,
/// `Ipe.Crypto`, `Ipe.Email`) or is shape-scoped (`Cmd` / `Sub` reached through
/// `Ipe.Tea.<Shape>.Cmd` / `.Sub`, deliberately absent as standalone modules),
/// but the kernel row itself carries only its bare qualifier. The fix is a
/// registry decision — add a qualifier→module mapping or align the qualifier to
/// the module — tracked as a coverage-SSOT follow-up, not weakened here.
const ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        "home",
        "Ipe.Attr.attribute",
        "kernel qualifier `Attr` → compiled-source Ipe.Html.Attributes; no registry mapping",
    ),
    (
        "home",
        "Ipe.Attr.boolAttribute",
        "kernel qualifier `Attr` → compiled-source Ipe.Html.Attributes; no registry mapping",
    ),
    (
        "home",
        "Ipe.Attr.noAttr",
        "kernel qualifier `Attr` → compiled-source Ipe.Html.Attributes; no registry mapping",
    ),
    (
        "home",
        "Ipe.Cmd.batch",
        "kernel qualifier `Cmd` is shape-scoped (Ipe.Tea.<Shape>.Cmd); no standalone module",
    ),
    (
        "home",
        "Ipe.Cmd.map",
        "kernel qualifier `Cmd` is shape-scoped (Ipe.Tea.<Shape>.Cmd); no standalone module",
    ),
    (
        "home",
        "Ipe.Cmd.none",
        "kernel qualifier `Cmd` is shape-scoped (Ipe.Tea.<Shape>.Cmd); no standalone module",
    ),
    (
        "home",
        "Ipe.Cmd.perform",
        "kernel qualifier `Cmd` is shape-scoped (Ipe.Tea.<Shape>.Cmd); no standalone module",
    ),
    (
        "home",
        "Ipe.Cmd.publish",
        "kernel qualifier `Cmd` is shape-scoped (Ipe.Tea.<Shape>.Cmd); no standalone module",
    ),
    (
        "home",
        "Ipe.Cmd.publishNoEcho",
        "kernel qualifier `Cmd` is shape-scoped (Ipe.Tea.<Shape>.Cmd); no standalone module",
    ),
    (
        "home",
        "Ipe.EmailAddress.parse",
        "kernel qualifier `EmailAddress` → compiled-source Ipe.Email; no registry mapping",
    ),
    (
        "home",
        "Ipe.EmailAddress.toString",
        "kernel qualifier `EmailAddress` → compiled-source Ipe.Email; no registry mapping",
    ),
    (
        "home",
        "Ipe.Key.fromBytes",
        "kernel qualifier `Key` → Ipe.Crypto key helpers; no registry mapping",
    ),
    (
        "home",
        "Ipe.Key.fromString",
        "kernel qualifier `Key` → Ipe.Crypto key helpers; no registry mapping",
    ),
    (
        "home",
        "Ipe.Mac.toHex",
        "kernel qualifier `Mac` → Ipe.Crypto MAC helpers; no registry mapping",
    ),
    (
        "home",
        "Ipe.Sub.batch",
        "kernel qualifier `Sub` is shape-scoped (Ipe.Tea.<Shape>.Sub); no standalone module",
    ),
    (
        "home",
        "Ipe.Sub.every",
        "kernel qualifier `Sub` is shape-scoped (Ipe.Tea.<Shape>.Sub); no standalone module",
    ),
    (
        "home",
        "Ipe.Sub.map",
        "kernel qualifier `Sub` is shape-scoped (Ipe.Tea.<Shape>.Sub); no standalone module",
    ),
    (
        "home",
        "Ipe.Sub.none",
        "kernel qualifier `Sub` is shape-scoped (Ipe.Tea.<Shape>.Sub); no standalone module",
    ),
    (
        "home",
        "Ipe.Sub.subscribeTopic",
        "kernel qualifier `Sub` is shape-scoped (Ipe.Tea.<Shape>.Sub); no standalone module",
    ),
    (
        "home",
        "Ipe.Sub.subscribeWebSocket",
        "kernel qualifier `Sub` is shape-scoped (Ipe.Tea.<Shape>.Sub); no standalone module",
    ),
    (
        "home",
        "Ipe.UiCells.cells",
        "kernel qualifier `UiCells` → compiled-source Ipe.Ui.Cells; no registry mapping",
    ),
    (
        "home",
        "Ipe.UiCells.column",
        "kernel qualifier `UiCells` → compiled-source Ipe.Ui.Cells; no registry mapping",
    ),
    (
        "home",
        "Ipe.UiCells.el",
        "kernel qualifier `UiCells` → compiled-source Ipe.Ui.Cells; no registry mapping",
    ),
    (
        "home",
        "Ipe.UiCells.none",
        "kernel qualifier `UiCells` → compiled-source Ipe.Ui.Cells; no registry mapping",
    ),
    (
        "home",
        "Ipe.UiCells.row",
        "kernel qualifier `UiCells` → compiled-source Ipe.Ui.Cells; no registry mapping",
    ),
    (
        "home",
        "Ipe.UiCells.text",
        "kernel qualifier `UiCells` → compiled-source Ipe.Ui.Cells; no registry mapping",
    ),
];

#[test]
fn static_columns_pass_over_the_whole_surface() {
    use std::fmt::Write as _;

    let report = matrix::run_static();

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
        "the four static coverage columns must pass over the whole stdlib \
         surface (or be recorded in the allowlist with a tracking reason):\n{unexpected}\n\
         (allowlist has {} entr(y/ies))",
        ALLOWLIST.len(),
    );
}

#[test]
fn allowlisted_holes_are_still_real() {
    // Guard against a stale allowlist: every allowlisted coordinate must still be
    // a hole the run reports. A removed hole must be removed from the allowlist,
    // not left to rot.
    let report = matrix::run_static();
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
