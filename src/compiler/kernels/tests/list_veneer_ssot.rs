//! SSOT gate: the type annotations written in `List.ipe` equal the kernel
//! scheme shapes registered in `StdlibKernel::scheme_shape`.
//!
//! For each exported List combinator the test:
//!
//! 1. Parses `List.ipe` with the real parser (`ipe_parse`).
//! 2. Reads the `Value.type_annotation` the parser attached.
//! 3. Converts the parsed `TypeAnnotation` to an owned, interning-free
//!    `OwnedScheme` where type variables are keyed by first-occurrence index.
//! 4. Converts `k.scheme_shape()` (the structural `TyShape`) to the same form.
//! 5. Asserts structural equality.
//!
//! No hardcoded expected type strings exist: `List.ipe` is ground truth.

mod veneer_helpers;

use ipe_intern::Interner;
use ipe_kernels::StdlibKernel;
use veneer_helpers::{AnnConverter, shape_to_owned};

/// The veneer source, read at compile time so the test is hermetic.
const LIST_IPE: &str = include_str!("../../../stdlib/Ipe/List.ipe");

// ── Kernel manifest ───────────────────────────────────────────────────────────

/// Each exported List kernel paired with its name as it appears in `List.ipe`.
///
/// Only kernels that (a) appear in the module's `exposing` clause and (b) have
/// a registered `scheme_shape` are listed here; the SSOT assertion requires
/// both.
const LIST_KERNELS: &[(StdlibKernel, &str)] = &[
    (StdlibKernel::ListMap, "map"),
    (StdlibKernel::ListFilter, "filter"),
    (StdlibKernel::ListAny, "any"),
    (StdlibKernel::ListAll, "all"),
    (StdlibKernel::ListFind, "find"),
    (StdlibKernel::ListFoldl, "foldl"),
    (StdlibKernel::ListFoldr, "foldr"),
    (StdlibKernel::ListConcatMap, "concatMap"),
    (StdlibKernel::ListIndexedMap, "indexedMap"),
    (StdlibKernel::ListFilterMap, "filterMap"),
    (StdlibKernel::ListIsEmpty, "isEmpty"),
    (StdlibKernel::ListLength, "length"),
    (StdlibKernel::ListHead, "head"),
    (StdlibKernel::ListTail, "tail"),
    (StdlibKernel::ListCons, "cons"),
    (StdlibKernel::ListReverse, "reverse"),
    (StdlibKernel::ListTake, "take"),
    (StdlibKernel::ListDrop, "drop"),
    (StdlibKernel::ListAppend, "append"),
    (StdlibKernel::ListConcat, "concat"),
    (StdlibKernel::ListMember, "member"),
    (StdlibKernel::ListRange, "range"),
    (StdlibKernel::ListZip, "zip"),
    (StdlibKernel::ListSortBy, "sortBy"),
    (StdlibKernel::ListSortWith, "sortWith"),
    (StdlibKernel::ListSum, "sum"),
    (StdlibKernel::ListMaximum, "maximum"),
    (StdlibKernel::ListMinimum, "minimum"),
    (StdlibKernel::ListUnique, "unique"),
];

// ── SSOT test ─────────────────────────────────────────────────────────────────

/// Every exported List combinator's written annotation equals the kernel's
/// registered `scheme_shape`.
///
/// The annotation side is parsed from `List.ipe` at test run time — no
/// hardcoded expected string exists.
#[test]
fn list_veneer_annotation_matches_kernel_scheme_shape() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(LIST_IPE, &mut interner)
        .expect("List.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(kernel, fn_name) in LIST_KERNELS {
        let value = module
            .values
            .iter()
            .map(|loc| &loc.value)
            .find(|v| interner.resolve(v.name.value) == Some(fn_name));

        let Some(value) = value else {
            failures.push(format!("{fn_name}: value not found in parsed List.ipe"));
            continue;
        };

        let Some(ann_loc) = &value.type_annotation else {
            failures.push(format!(
                "{fn_name}: no type annotation in List.ipe — add `{fn_name} : T`"
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

        let mut conv = AnnConverter::new(&interner, "List");
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
        "List veneer annotation / kernel scheme divergence:\n{}",
        failures.join("\n")
    );
}

/// Every exported List combinator in the kernel manifest has both a doc-string
/// and a type annotation in `List.ipe`.
#[test]
fn every_list_export_has_doc_and_annotation() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(LIST_IPE, &mut interner)
        .expect("List.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(_, fn_name) in LIST_KERNELS {
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
        "List exports missing doc or annotation:\n{}",
        failures.join("\n")
    );
}
