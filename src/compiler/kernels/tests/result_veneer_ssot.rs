//! SSOT gate: the type annotations written in `Result.ipe` equal the kernel
//! scheme shapes registered in `StdlibKernel::scheme_shape`.
//!
//! For each exported Result combinator the test:
//!
//! 1. Parses `Result.ipe` with the real parser (`ipe_parse`).
//! 2. Reads the `Value.type_annotation` the parser attached.
//! 3. Converts the parsed `TypeAnnotation` to an owned, interning-free
//!    `OwnedScheme` where type variables are keyed by first-occurrence index.
//! 4. Converts `k.scheme_shape()` (the structural `TyShape`) to the same form.
//! 5. Asserts structural equality.
//!
//! No hardcoded expected type strings exist: `Result.ipe` is ground truth.

mod veneer_helpers;

use ipe_intern::Interner;
use ipe_kernels::StdlibKernel;
use veneer_helpers::{AnnConverter, shape_to_owned};

/// The veneer source, read at compile time so the test is hermetic.
const RESULT_IPE: &str = include_str!("../../../stdlib/Ipe/Result.ipe");

// ── Kernel manifest ───────────────────────────────────────────────────────────

/// Each exported Result kernel paired with its name as it appears in `Result.ipe`.
const RESULT_KERNELS: &[(StdlibKernel, &str)] = &[
    (StdlibKernel::ResultWithDefault, "withDefault"),
    (StdlibKernel::ResultMap, "map"),
    (StdlibKernel::ResultAndThen, "andThen"),
    (StdlibKernel::ResultMapError, "mapError"),
    (StdlibKernel::ResultMap2, "map2"),
    (StdlibKernel::ResultMap3, "map3"),
    (StdlibKernel::ResultMap4, "map4"),
    (StdlibKernel::ResultMap5, "map5"),
    (StdlibKernel::ResultAndMap, "andMap"),
    (StdlibKernel::ResultCombine, "combine"),
    (StdlibKernel::ResultToMaybe, "toMaybe"),
    (StdlibKernel::ResultFromMaybe, "fromMaybe"),
];

// ── SSOT test ─────────────────────────────────────────────────────────────────

/// Every exported Result combinator's written annotation equals the kernel's
/// registered `scheme_shape`.
///
/// The annotation side is parsed from `Result.ipe` at test run time — no
/// hardcoded expected string exists.
#[test]
fn result_veneer_annotation_matches_kernel_scheme_shape() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(RESULT_IPE, &mut interner)
        .expect("Result.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(kernel, fn_name) in RESULT_KERNELS {
        let value = module
            .values
            .iter()
            .map(|loc| &loc.value)
            .find(|v| interner.resolve(v.name.value) == Some(fn_name));

        let Some(value) = value else {
            failures.push(format!("{fn_name}: value not found in parsed Result.ipe"));
            continue;
        };

        let Some(ann_loc) = &value.type_annotation else {
            failures.push(format!(
                "{fn_name}: no type annotation in Result.ipe — add `{fn_name} : T`"
            ));
            continue;
        };

        let Some(shape) = kernel.scheme_shape() else {
            failures.push(format!(
                "{fn_name} ({kernel:?}): kernel has no scheme_shape — \
                 it must be registered in the TyShape table"
            ));
            continue;
        };

        let mut conv = AnnConverter::new(&interner, "Result");
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
        "Result veneer annotation / kernel scheme divergence:\n{}",
        failures.join("\n")
    );
}

/// Every exported Result combinator in the kernel manifest has both a doc-string
/// and a type annotation in `Result.ipe`.
#[test]
fn every_result_export_has_doc_and_annotation() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(RESULT_IPE, &mut interner)
        .expect("Result.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(_, fn_name) in RESULT_KERNELS {
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
        "Result exports missing doc or annotation:\n{}",
        failures.join("\n")
    );
}
