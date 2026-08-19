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

use ipe_intern::Interner;
use ipe_kernels::{BuiltinTag, StdlibKernel, TyShape};
use ipe_syntax::TypeAnnotation;

/// The veneer source, read at compile time so the test is hermetic.
const MAYBE_IPE: &str = include_str!("../../../stdlib/Ipe/Maybe.ipe");

// ── Owned scheme representation ─────────────────────────────────────────────

/// An interning-free, owned type-scheme shape for SSOT comparison.
///
/// Type variables are named by their first-occurrence index in the annotation
/// (0 = first variable seen, 1 = second, …). This is the same positional
/// numbering `TyShape::Var(u8)` uses in the kernel scheme table, so the two
/// can be compared structurally without a shared interner.
#[derive(Clone, PartialEq, Eq, Debug)]
enum OwnedScheme {
    Fun(Box<Self>, Box<Self>),
    Con(BuiltinTag, Vec<Self>),
    Var(u8),
}

impl OwnedScheme {
    fn fun(a: Self, b: Self) -> Self {
        Self::Fun(Box::new(a), Box::new(b))
    }
}

// ── TyShape → OwnedScheme ────────────────────────────────────────────────────

fn shape_to_owned(shape: &TyShape) -> Result<OwnedScheme, String> {
    match shape {
        TyShape::Fun(a, b) => Ok(OwnedScheme::fun(shape_to_owned(a)?, shape_to_owned(b)?)),
        TyShape::Con(tag, args) => {
            let owned_args = args
                .iter()
                .map(shape_to_owned)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(OwnedScheme::Con(*tag, owned_args))
        }
        TyShape::Var(i) => Ok(OwnedScheme::Var(*i)),
        // The Maybe combinators use only Fun / Con / Var; the other variants
        // (Tuple, Record, Unit) are not reached for this module's exports.
        TyShape::Tuple(_) | TyShape::Record { .. } | TyShape::Unit => Err(format!(
            "unexpected TyShape variant {shape:?} in a Maybe combinator scheme"
        )),
    }
}

// ── TypeAnnotation → OwnedScheme ─────────────────────────────────────────────

/// Context for converting a parsed `TypeAnnotation` to `OwnedScheme`.
///
/// Maps type-variable names (resolved strings) to their first-occurrence
/// positional index so the result is byte-comparable with the kernel's
/// `scheme_shape()`, which uses the same positional convention.
struct AnnConverter<'i> {
    interner: &'i Interner,
    /// Variable name → positional index, in first-occurrence order.
    vars: Vec<String>,
}

impl<'i> AnnConverter<'i> {
    const fn new(interner: &'i Interner) -> Self {
        Self {
            interner,
            vars: Vec::new(),
        }
    }

    /// Map a type-variable name to its positional index, inserting it at
    /// the end if this is its first occurrence.
    fn var_index(&mut self, name: &str) -> Result<u8, String> {
        if let Some(i) = self.vars.iter().position(|v| v == name) {
            u8::try_from(i).map_err(|_| format!("too many type variables: index {i}"))
        } else {
            let idx = self.vars.len();
            self.vars.push(name.to_owned());
            u8::try_from(idx).map_err(|_| format!("too many type variables: index {idx}"))
        }
    }

    fn convert(&mut self, ann: &TypeAnnotation) -> Result<OwnedScheme, String> {
        match ann {
            TypeAnnotation::TLambda(a, b) => {
                Ok(OwnedScheme::fun(self.convert(a)?, self.convert(b)?))
            }
            TypeAnnotation::TVar(sym) => {
                let name = self
                    .interner
                    .resolve(*sym)
                    .ok_or_else(|| format!("type variable symbol {sym:?} not found in interner"))?;
                Ok(OwnedScheme::Var(self.var_index(name)?))
            }
            TypeAnnotation::TType(_qualifier, segments, args) => {
                // The type-constructor name is the last (and for simple names
                // the only) segment.
                let name = segments
                    .last()
                    .and_then(|&s| self.interner.resolve(s))
                    .ok_or("type constructor has no resolvable name segment")?;
                let tag = builtin_tag_from_name(name).ok_or_else(|| {
                    format!("unknown builtin type constructor `{name}` in Maybe.ipe annotation")
                })?;
                let converted_args = args
                    .iter()
                    .map(|a| self.convert(a))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OwnedScheme::Con(tag, converted_args))
            }
            TypeAnnotation::TUnit
            | TypeAnnotation::TTuple(_)
            | TypeAnnotation::TRecord(_)
            | TypeAnnotation::TRecordOpen(_, _) => Err(format!(
                "unexpected TypeAnnotation variant {ann:?} in Maybe.ipe"
            )),
        }
    }
}

/// Map a type-constructor name as it appears in `.ipe` source to the
/// corresponding [`BuiltinTag`]. Only the tags that appear in the `Maybe`
/// module's annotations are needed here.
fn builtin_tag_from_name(name: &str) -> Option<BuiltinTag> {
    match name {
        "Bool" => Some(BuiltinTag::Bool),
        "List" => Some(BuiltinTag::List),
        "Maybe" => Some(BuiltinTag::Maybe),
        _ => None,
    }
}

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
        let mut conv = AnnConverter::new(&interner);
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
