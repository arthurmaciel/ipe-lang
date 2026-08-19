//! Shared helpers for the veneer SSOT tests.
//!
//! Each module test (`maybe_veneer_ssot`, `list_veneer_ssot`, …) needs the
//! same two conversions:
//!
//! - [`shape_to_owned`]: `TyShape` → `OwnedScheme` (from the kernel table).
//! - [`AnnConverter`]: `TypeAnnotation` → `OwnedScheme` (from a parsed `.ipe`
//!   file), mapping type-variable names to positional indices.
//!
//! Both sides land in [`OwnedScheme`], an interning-free representation that
//! can be compared with `==`. The `.ipe` file is the ground truth; the kernel
//! table is what must agree with it.

use ipe_intern::Interner;
use ipe_kernels::{BuiltinTag, TyShape};
use ipe_syntax::TypeAnnotation;

// ── Owned scheme representation ─────────────────────────────────────────────

/// An interning-free, owned type-scheme shape for SSOT comparison.
///
/// Type variables are named by their first-occurrence index in the annotation
/// (0 = first variable seen, 1 = second, …). This is the same positional
/// numbering `TyShape::Var(u8)` uses in the kernel scheme table, so the two
/// can be compared structurally without a shared interner.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum OwnedScheme {
    Fun(Box<Self>, Box<Self>),
    Con(BuiltinTag, Vec<Self>),
    Var(u8),
    Tuple(Vec<Self>),
    Unit,
}

impl OwnedScheme {
    #[must_use]
    pub fn fun(a: Self, b: Self) -> Self {
        Self::Fun(Box::new(a), Box::new(b))
    }
}

// ── TyShape → OwnedScheme ────────────────────────────────────────────────────

/// Converts a `TyShape` from the kernel table to [`OwnedScheme`], normalizing
/// type-variable indices by first-occurrence order.
///
/// The kernel assigns absolute indices (`A=0, B=1, C=2, …`) to type variables,
/// but the order in which they appear in a concrete scheme need not start at 0.
/// For example, `Result.andMap` uses `Var(2)` as the first variable encountered
/// in left-to-right traversal. The annotation converter also assigns indices by
/// first-occurrence, so both sides must use the same normalization before they
/// can be compared with `==`.
///
/// # Errors
///
/// Returns `Err` if the shape contains a `TyShape::Record` variant, which is
/// not expected in any stdlib combinator scheme.
pub fn shape_to_owned(shape: &TyShape) -> Result<OwnedScheme, String> {
    let mut converter = ShapeConverter::default();
    converter.convert(shape)
}

/// Stateful converter that normalizes kernel `TyShape::Var(u8)` indices to
/// first-occurrence positional order, matching what [`AnnConverter`] does for
/// annotation variables.
#[derive(Default)]
struct ShapeConverter {
    /// Maps each kernel var index (from `TyShape::Var(i)`) to the
    /// first-occurrence positional index assigned by this converter.
    seen: Vec<u8>,
}

impl ShapeConverter {
    fn var_index(&mut self, kernel_idx: u8) -> Result<u8, String> {
        if let Some(pos) = self.seen.iter().position(|&k| k == kernel_idx) {
            u8::try_from(pos).map_err(|_| format!("var index overflow at position {pos}"))
        } else {
            let pos = self.seen.len();
            self.seen.push(kernel_idx);
            u8::try_from(pos).map_err(|_| format!("var index overflow at position {pos}"))
        }
    }

    fn convert(&mut self, shape: &TyShape) -> Result<OwnedScheme, String> {
        match shape {
            TyShape::Fun(a, b) => Ok(OwnedScheme::fun(self.convert(a)?, self.convert(b)?)),
            TyShape::Con(tag, args) => {
                let owned_args = args
                    .iter()
                    .map(|a| self.convert(a))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OwnedScheme::Con(*tag, owned_args))
            }
            TyShape::Var(i) => Ok(OwnedScheme::Var(self.var_index(*i)?)),
            TyShape::Tuple(elems) => {
                let owned_elems = elems
                    .iter()
                    .map(|e| self.convert(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OwnedScheme::Tuple(owned_elems))
            }
            TyShape::Unit => Ok(OwnedScheme::Unit),
            TyShape::Record { .. } => Err(format!(
                "unexpected TyShape::Record in kernel scheme: {shape:?}"
            )),
        }
    }
}

// ── TypeAnnotation → OwnedScheme ─────────────────────────────────────────────

/// Context for converting a parsed `TypeAnnotation` to [`OwnedScheme`].
///
/// Maps type-variable names (resolved strings) to their first-occurrence
/// positional index so the result is byte-comparable with the kernel's
/// `scheme_shape()`, which uses the same positional convention.
pub struct AnnConverter<'i> {
    interner: &'i Interner,
    /// Variable name → positional index, in first-occurrence order.
    vars: Vec<String>,
    /// The module name, used only in error messages.
    module: &'static str,
}

impl<'i> AnnConverter<'i> {
    #[must_use]
    pub const fn new(interner: &'i Interner, module: &'static str) -> Self {
        Self {
            interner,
            vars: Vec::new(),
            module,
        }
    }

    /// Map a type-variable name to its positional index, inserting it at
    /// the end if this is its first occurrence.
    ///
    /// # Errors
    ///
    /// Returns `Err` if more than 255 distinct type variables appear.
    pub fn var_index(&mut self, name: &str) -> Result<u8, String> {
        if let Some(i) = self.vars.iter().position(|v| v == name) {
            u8::try_from(i).map_err(|_| format!("too many type variables: index {i}"))
        } else {
            let idx = self.vars.len();
            self.vars.push(name.to_owned());
            u8::try_from(idx).map_err(|_| format!("too many type variables: index {idx}"))
        }
    }

    /// Convert a `TypeAnnotation` to [`OwnedScheme`].
    ///
    /// # Errors
    ///
    /// Returns `Err` if the annotation contains a type constructor not in the
    /// known `BuiltinTag` set, or a `TRecord`/`TRecordOpen` variant.
    pub fn convert(&mut self, ann: &TypeAnnotation) -> Result<OwnedScheme, String> {
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
                let name = segments
                    .last()
                    .and_then(|&s| self.interner.resolve(s))
                    .ok_or("type constructor has no resolvable name segment")?;
                let tag = builtin_tag_from_name(name).ok_or_else(|| {
                    format!(
                        "unknown builtin type constructor `{name}` in {m}.ipe annotation",
                        m = self.module
                    )
                })?;
                let converted_args = args
                    .iter()
                    .map(|a| self.convert(a))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OwnedScheme::Con(tag, converted_args))
            }
            TypeAnnotation::TUnit => Ok(OwnedScheme::Unit),
            TypeAnnotation::TTuple(elems) => {
                let owned_elems = elems
                    .iter()
                    .map(|e| self.convert(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OwnedScheme::Tuple(owned_elems))
            }
            TypeAnnotation::TRecord(_) | TypeAnnotation::TRecordOpen(_, _) => Err(format!(
                "unexpected TypeAnnotation::TRecord in {m}.ipe annotation: {ann:?}",
                m = self.module
            )),
        }
    }
}

// ── Name → BuiltinTag ────────────────────────────────────────────────────────

/// Map a type-constructor name as written in `.ipe` source to the
/// corresponding [`BuiltinTag`].
///
/// Only the tags that appear in the modules under test are needed; add more
/// as new modules are covered.
#[must_use]
pub fn builtin_tag_from_name(name: &str) -> Option<BuiltinTag> {
    match name {
        "Bool" => Some(BuiltinTag::Bool),
        "Int" => Some(BuiltinTag::Int),
        "Float" => Some(BuiltinTag::Float),
        "String" => Some(BuiltinTag::String),
        "Char" => Some(BuiltinTag::Char),
        "List" => Some(BuiltinTag::List),
        "Maybe" => Some(BuiltinTag::Maybe),
        "Result" => Some(BuiltinTag::Result),
        "Order" => Some(BuiltinTag::Order),
        _ => None,
    }
}
