//! The shared coverage-matrix contract: one exported stdlib symbol, the surface
//! and aspect traits, and a cell verdict.
//!
//! The stdlib surface is otherwise an implicit concept, re-derived by three
//! partial enumerations that drift apart; a member present in one but forgotten
//! in another falls through the seam between their gates. This contract fixes one
//! reconciled symbol type and one `surface × aspect` grid so a new aspect is a
//! single column applied to every symbol, and a hole is named at its coordinate
//! rather than lost between checks.
//!
//! A [`Surface`] is any enumerable registry; [`StdlibSymbol`] is the item of the
//! stdlib surface. An [`AspectCheck`] renders one column, returning a [`Cell`]
//! per symbol. A [`Cell::Hole`] fails the gate; a [`Cell::Warn`] is advisory
//! debt; [`Cell::Ok`] and [`Cell::NotApplicable`] pass.

/// Which namespace of the exported surface a symbol belongs to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum SymbolKind {
    /// A value binding (a function or a constant).
    Value,
    /// A type constructor (a union or an opaque type name).
    Type,
    /// A data constructor of a union.
    Ctor,
}

/// One exported stdlib symbol, reconciled from every enumeration of the surface.
///
/// Each facet answers "does this enumeration know the symbol?" so a symbol
/// present in one registry but absent from another is visible at a glance, and
/// the aspect columns read the facets rather than re-deriving them.
// The four facet bools are independent registry answers (kernel / compiled-source
// / exported / higher-order), each read by a distinct column; collapsing them
// into an enum would fabricate mutual exclusions that do not hold (a symbol is
// commonly both kernel-backed and compiled-source-aliased).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct StdlibSymbol {
    /// The dot-free module segments the symbol lives in (`["Ipe", "List"]`).
    pub module: Vec<String>,
    /// The symbol's own name (`"map"`).
    pub name: String,
    /// Whether the symbol names a value, a type, or a constructor.
    pub kind: SymbolKind,
    /// Registered in the kernel registry.
    pub has_kernel: bool,
    /// Defined in a compiled-source stdlib module.
    pub has_compiled_source: bool,
    /// Listed in its module's `exposing (...)` — reachable by an importer.
    pub exported: bool,
    /// The α-canonical typed scheme string, when the symbol is typeable.
    pub scheme: Option<String>,
    /// The scheme takes or returns a function — drives the composition column.
    pub is_higher_order: bool,
}

/// An enumerable registry of items.
///
/// The stdlib surface is the first implementor; env-var, CLI, diagnostic,
/// foreign, and package registries are siblings that drift the same way, so the
/// runner iterates any `Surface × AspectCheck` and each surface declares which
/// aspects apply (an inapplicable aspect returns [`Cell::NotApplicable`]).
pub trait Surface: Sync {
    /// The kind of item this surface enumerates.
    type Item;
    /// A human name for the surface, used in the rendered matrix header.
    fn name(&self) -> &'static str;
    /// Every item of the surface, in a deterministic, sorted order.
    fn all(&self) -> Vec<Self::Item>;
}

/// One aspect column of the matrix.
///
/// Applied to every symbol of a surface; the returned [`Cell`] is that
/// `(symbol, aspect)` coordinate's verdict.
pub trait AspectCheck: Sync {
    /// A human name for the column, used in the rendered matrix header and in a
    /// hole's failure message.
    fn name(&self) -> &'static str;
    /// The verdict for one symbol on this aspect.
    fn check(&self, sym: &StdlibSymbol) -> Cell;
}

/// One `(symbol, aspect)` verdict.
///
/// The [`Cell::Hole`]-versus-[`Cell::Warn`] split is the severity axis: a
/// forgotten binding is a correctness gap that fails the gate, an advisory (an
/// untested helper, a review-staleness note) is debt that is reported without
/// failing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Cell {
    /// The aspect holds for this symbol.
    Ok,
    /// A real gap, carrying a human message — fails the gate at this coordinate.
    Hole(String),
    /// An advisory the aspect flags without failing the gate.
    Warn(String),
    /// The aspect does not apply to this symbol (e.g. composition for a
    /// first-order value, `wasm` for a native-only kernel).
    NotApplicable,
}
