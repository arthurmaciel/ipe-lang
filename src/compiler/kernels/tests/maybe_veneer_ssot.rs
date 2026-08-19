//! SSOT gate: the type annotations written in `Maybe.ipe` equal the kernel
//! scheme shapes registered in `StdlibKernel::scheme_shape`.
//!
//! For each exported Maybe combinator the test:
//!
//! 1. Parses `Maybe.ipe` with the real parser (`ipe_parse`).
//! 2. Reads the `Value.type_annotation` the parser attached.
//! 3. Converts the parsed `TypeAnnotation` to an owned, interning-free
//!    representation (`OwnedScheme`) where type variables are keyed by their
//!    first-occurrence position in the annotation.
//! 4. Converts `k.scheme_shape()` (the structural `TyShape`) to the same
//!    `OwnedScheme` representation.
//! 5. Asserts the two are structurally equal.
//!
//! If the veneer annotation and the kernel shape ever diverge — say a kernel
//! type is updated but the annotation is not, or vice versa — this test
//! reddens. No hardcoded copy of the expected type exists: the ground truth is
//! always read from the file.

mod veneer_helpers;

use ipe_intern::Interner;
use ipe_kernels::StdlibKernel;
use veneer_helpers::{AnnConverter, shape_to_owned};

/// The veneer source, read at compile time so the test is hermetic.
const MAYBE_IPE: &str = include_str!("../../../stdlib/Ipe/Maybe.ipe");

// ── The SSOT test ─────────────────────────────────────────────────────────────

/// Each Maybe kernel's `decl().name` paired with its variant, so we can look
/// up the parsed `Value` by name.
const MAYBE_KERNELS: &[(StdlibKernel, &str)] = &[
    (StdlibKernel::MaybeWithDefault, "withDefault"),
    (StdlibKernel::MaybeMap, "map"),
    (StdlibKernel::MaybeAndThen, "andThen"),
    (StdlibKernel::MaybeMap2, "map2"),
    (StdlibKernel::MaybeMap3, "map3"),
    (StdlibKernel::MaybeMap4, "map4"),
    (StdlibKernel::MaybeMap5, "map5"),
    (StdlibKernel::MaybeAndMap, "andMap"),
    (StdlibKernel::MaybeCombine, "combine"),
    (StdlibKernel::MaybeIsJust, "isJust"),
    (StdlibKernel::MaybeIsNothing, "isNothing"),
];

/// Every exported Maybe combinator's written type annotation equals the kernel's
/// registered `scheme_shape`.
///
/// The comparison operand on the annotation side is parsed from `Maybe.ipe` at
/// test run time — no hardcoded expected string exists. A mismatch between the
/// veneer file and the kernel shape is a test failure.
#[test]
fn maybe_veneer_annotation_matches_kernel_scheme_shape() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(MAYBE_IPE, &mut interner)
        .expect("Maybe.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(kernel, fn_name) in MAYBE_KERNELS {
        // Locate the parsed Value by name.
        let value = module
            .values
            .iter()
            .map(|loc| &loc.value)
            .find(|v| interner.resolve(v.name.value) == Some(fn_name));

        let Some(value) = value else {
            failures.push(format!("{fn_name}: value not found in parsed Maybe.ipe"));
            continue;
        };

        // The annotation must be present — every export must carry one.
        let Some(ann_loc) = &value.type_annotation else {
            failures.push(format!(
                "{fn_name}: no type annotation in Maybe.ipe — add `{fn_name} : T`"
            ));
            continue;
        };

        // The kernel must carry a structural scheme shape.
        let Some(shape) = kernel.scheme_shape() else {
            failures.push(format!(
                "{fn_name} ({kernel:?}): kernel has no scheme_shape — \
                 it must be registered in the TyShape table"
            ));
            continue;
        };

        // Convert both sides to the common `OwnedScheme` form and compare.
        let mut conv = AnnConverter::new(&interner, "Maybe");
        let from_annotation = match conv.convert(&ann_loc.value) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{fn_name}: annotation conversion failed: {e}"));
                continue;
            }
        };
        let from_shape = match shape_to_owned(shape) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{fn_name}: scheme_shape conversion failed: {e}"));
                continue;
            }
        };

        if from_annotation != from_shape {
            failures.push(format!(
                "{fn_name}: annotation {from_annotation:?} ≠ scheme_shape {from_shape:?}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "Maybe veneer annotation / kernel scheme divergence:\n{}",
        failures.join("\n")
    );
}

/// Every exported Maybe combinator has both a doc-string and a type annotation.
///
/// A missing doc-string or annotation reddens early rather than waiting for the
/// SSOT comparison above; the error message names the specific missing piece.
#[test]
fn every_maybe_export_has_doc_and_annotation() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(MAYBE_IPE, &mut interner)
        .expect("Maybe.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(_, fn_name) in MAYBE_KERNELS {
        let value = module
            .values
            .iter()
            .map(|loc| &loc.value)
            .find(|v| interner.resolve(v.name.value) == Some(fn_name));

        let Some(value) = value else {
            failures.push(format!("{fn_name}: not found in parsed module"));
            continue;
        };
        if value.doc.is_none() {
            failures.push(format!(
                "{fn_name}: missing {{-| … -}} doc-string — every export must be documented"
            ));
        }
        if value.type_annotation.is_none() {
            failures.push(format!(
                "{fn_name}: missing type annotation — add `{fn_name} : T` before the binding"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "Maybe exports missing doc or annotation:\n{}",
        failures.join("\n")
    );
}
