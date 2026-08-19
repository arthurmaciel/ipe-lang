//! SSOT gate: the type annotations written in `String.ipe` equal the kernel
//! scheme shapes registered in `StdlibKernel::scheme_shape`.
//!
//! Only the polymorphic String combinators — those whose `scheme_shape` returns
//! `Some(…)` — are checked here. Monomorphic combinators (`fromInt`,
//! `toUpper`, etc.) have concrete arrow types whose correctness is covered by
//! the broader type-inference tests; the SSOT gate targets the schemes that
//! contain type variables and are therefore most likely to drift.
//!
//! No hardcoded expected type strings exist: `String.ipe` is ground truth.

mod veneer_helpers;

use ipe_intern::Interner;
use ipe_kernels::StdlibKernel;
use veneer_helpers::{AnnConverter, shape_to_owned};

/// The veneer source, read at compile time so the test is hermetic.
const STRING_IPE: &str = include_str!("../../../stdlib/Ipe/String.ipe");

// ── Kernel manifest ───────────────────────────────────────────────────────────

/// Exported String combinators that have a registered `scheme_shape`.
///
/// Monomorphic combinators (those whose scheme uses only concrete primitive
/// types and no type variables) return `None` from `scheme_shape` and are
/// omitted — they have no type variable to check positional agreement on.
const STRING_KERNELS: &[(StdlibKernel, &str)] = &[
    (StdlibKernel::StringToInt, "toInt"),
    (StdlibKernel::StringToFloat, "toFloat"),
    (StdlibKernel::StringFromList, "fromList"),
    (StdlibKernel::StringConcat, "concat"),
    (StdlibKernel::StringWords, "words"),
    (StdlibKernel::StringLines, "lines"),
    (StdlibKernel::StringToList, "toList"),
    (StdlibKernel::StringJoin, "join"),
    (StdlibKernel::StringSplit, "split"),
    (StdlibKernel::StringUncons, "uncons"),
    (StdlibKernel::StringIndexes, "indexes"),
    (StdlibKernel::StringFoldl, "foldl"),
    (StdlibKernel::StringFoldr, "foldr"),
];

// ── SSOT test ─────────────────────────────────────────────────────────────────

/// Every listed String combinator's written annotation equals the kernel's
/// registered `scheme_shape`.
#[test]
fn string_veneer_annotation_matches_kernel_scheme_shape() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(STRING_IPE, &mut interner)
        .expect("String.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(kernel, fn_name) in STRING_KERNELS {
        let value = module
            .values
            .iter()
            .map(|loc| &loc.value)
            .find(|v| interner.resolve(v.name.value) == Some(fn_name));

        let Some(value) = value else {
            failures.push(format!("{fn_name}: value not found in parsed String.ipe"));
            continue;
        };

        let Some(ann_loc) = &value.type_annotation else {
            failures.push(format!(
                "{fn_name}: no type annotation in String.ipe — add `{fn_name} : T`"
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

        let mut conv = AnnConverter::new(&interner, "String");
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
        "String veneer annotation / kernel scheme divergence:\n{}",
        failures.join("\n")
    );
}

/// Every String combinator in the kernel manifest has both a doc-string and a
/// type annotation in `String.ipe`.
#[test]
fn every_string_export_has_doc_and_annotation() {
    let mut interner = Interner::new();
    let module = ipe_parse::parse_module(STRING_IPE, &mut interner)
        .expect("String.ipe must parse without errors");

    let mut failures: Vec<String> = Vec::new();

    for &(_, fn_name) in STRING_KERNELS {
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
        "String exports missing doc or annotation:\n{}",
        failures.join("\n")
    );
}
