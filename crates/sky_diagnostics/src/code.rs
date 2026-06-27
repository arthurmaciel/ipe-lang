//! Stable error codes (`SKY-XNNNN`) and their metadata.
//!
//! Every [`crate::Diagnostic`] maps to exactly one [`Code`] via
//! [`crate::Diagnostic::code`]. Codes are the machine-greppable, user-facing
//! handle a reader passes to `skyc explain <CODE>`; they never change once
//! shipped. The taxonomy is authoritative and lives in
//! `docs/superpowers/specs/2026-06-27-diagnostics-error-code-system-design.md`.
//!
//! Ranges: `SKY-P####` parse, `SKY-N####` name resolution, `SKY-T####` type,
//! `SKY-L####` lower / not-yet-supported, `SKY-I####` internal (compiler bug).

/// A stable compiler error code, e.g. `SKY-T0001`.
///
/// The wrapped string is always one of the taxonomy constants in this module;
/// the field is private so a `Code` cannot be forged with an unknown value from
/// outside the crate. Compare via the constants ([`SKY_T0001`] etc.).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Code(&'static str);

impl Code {
    /// The wire form of the code, e.g. `"SKY-T0001"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// How severe a diagnostic is. Governs the rendered header word and exit policy.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Severity {
    /// A genuine user error: compilation must stop.
    Error,
    /// A warning: compilation may continue but the code is suspect.
    Warning,
    /// An internal compiler bug (an `SKY-I####`): "please report".
    Bug,
}

// ---------------------------------------------------------------------------
// Parse (SKY-P####)
// ---------------------------------------------------------------------------

/// unexpected token
pub const SKY_P0001: Code = Code("SKY-P0001");
/// unexpected end of file
pub const SKY_P0002: Code = Code("SKY-P0002");
/// input nests too deeply
pub const SKY_P0003: Code = Code("SKY-P0003");
/// unknown character
pub const SKY_P0010: Code = Code("SKY-P0010");
/// stray `.`
pub const SKY_P0011: Code = Code("SKY-P0011");
/// number joined to a name
pub const SKY_P0012: Code = Code("SKY-P0012");
/// integer literal out of range
pub const SKY_P0013: Code = Code("SKY-P0013");
/// malformed module header
pub const SKY_P0020: Code = Code("SKY-P0020");
/// malformed exposing list
pub const SKY_P0021: Code = Code("SKY-P0021");
/// missing `=` in definition
pub const SKY_P0030: Code = Code("SKY-P0030");
/// malformed type declaration
pub const SKY_P0031: Code = Code("SKY-P0031");
/// only a type constructor can take arguments
pub const SKY_P0040: Code = Code("SKY-P0040");
/// expected a type
pub const SKY_P0041: Code = Code("SKY-P0041");
/// unclosed delimiter
pub const SKY_P0050: Code = Code("SKY-P0050");
/// malformed case expression
pub const SKY_P0060: Code = Code("SKY-P0060");

// ---------------------------------------------------------------------------
// Name resolution (SKY-N####)
// ---------------------------------------------------------------------------

/// cannot find this value in scope
pub const SKY_N0001: Code = Code("SKY-N0001");
/// cannot find this type in scope
pub const SKY_N0002: Code = Code("SKY-N0002");
/// cannot find this constructor
pub const SKY_N0003: Code = Code("SKY-N0003");
/// unknown module or import
pub const SKY_N0004: Code = Code("SKY-N0004");
/// module has no such member
pub const SKY_N0005: Code = Code("SKY-N0005");
/// value defined more than once
pub const SKY_N0010: Code = Code("SKY-N0010");
/// constructor defined more than once
pub const SKY_N0011: Code = Code("SKY-N0011");
/// type defined more than once
pub const SKY_N0012: Code = Code("SKY-N0012");

// ---------------------------------------------------------------------------
// Type (SKY-T####)
// ---------------------------------------------------------------------------

/// type mismatch
pub const SKY_T0001: Code = Code("SKY-T0001");
/// infinite type
pub const SKY_T0002: Code = Code("SKY-T0002");
/// type inference exceeded its step budget
pub const SKY_T0003: Code = Code("SKY-T0003");
/// more parameters than the type signature describes
pub const SKY_T0004: Code = Code("SKY-T0004");
/// this case does not handle every possibility
pub const SKY_T0010: Code = Code("SKY-T0010");
/// redundant case branch
pub const SKY_T0011: Code = Code("SKY-T0011");

// ---------------------------------------------------------------------------
// Lower / not-yet-supported (SKY-L####)
// ---------------------------------------------------------------------------

/// pattern kind not supported yet
pub const SKY_L0100: Code = Code("SKY-L0100");
/// operator not supported yet
pub const SKY_L0101: Code = Code("SKY-L0101");
/// polymorphic type variables not supported yet
pub const SKY_L0102: Code = Code("SKY-L0102");
/// function-valued parameters/returns not supported yet
pub const SKY_L0103: Code = Code("SKY-L0103");
/// only `Task ()` is supported yet
pub const SKY_L0104: Code = Code("SKY-L0104");
/// parameter destructuring not supported yet
pub const SKY_L0105: Code = Code("SKY-L0105");
/// top-level function needs a type signature
pub const SKY_L0106: Code = Code("SKY-L0106");
/// first-class functions not supported yet
pub const SKY_L0107: Code = Code("SKY-L0107");
/// kernel function not available yet
pub const SKY_L0108: Code = Code("SKY-L0108");
/// expression nests too deeply for the backend
pub const SKY_L0200: Code = Code("SKY-L0200");

// ---------------------------------------------------------------------------
// Internal (SKY-I####)
// ---------------------------------------------------------------------------

/// internal compiler error
pub const SKY_I0001: Code = Code("SKY-I0001");
/// intern: unresolved symbol
pub const SKY_I0010: Code = Code("SKY-I0010");
/// intern: symbol table exhausted
pub const SKY_I0011: Code = Code("SKY-I0011");
/// ICE: match on unknown variant
pub const SKY_I0100: Code = Code("SKY-I0100");
/// ICE: duplicate match arm
pub const SKY_I0101: Code = Code("SKY-I0101");
/// ICE: non-exhaustive match
pub const SKY_I0102: Code = Code("SKY-I0102");
/// ICE: match arm enum mismatch
pub const SKY_I0103: Code = Code("SKY-I0103");
/// ICE: no Rust name for symbol
pub const SKY_I0200: Code = Code("SKY-I0200");
/// ICE: dangling value/variant symbol
pub const SKY_I0201: Code = Code("SKY-I0201");
/// ICE: cross-module type-name collision
pub const SKY_I0202: Code = Code("SKY-I0202");
/// ICE: golden anchor missing
pub const SKY_I0203: Code = Code("SKY-I0203");

/// The one-line human title for a code.
///
/// Total over the taxonomy: every shipped constant has an explicit arm. A code
/// outside the taxonomy (impossible to construct outside this crate) falls back
/// to the generic internal-error title rather than panicking.
#[must_use]
pub fn title(c: Code) -> &'static str {
    match c {
        SKY_P0001 => "unexpected token",
        SKY_P0002 => "unexpected end of file",
        SKY_P0003 => "input nests too deeply",
        SKY_P0010 => "unknown character",
        SKY_P0011 => "stray '.'",
        SKY_P0012 => "number joined to a name",
        SKY_P0013 => "integer literal out of range",
        SKY_P0020 => "malformed module header",
        SKY_P0021 => "malformed exposing list",
        SKY_P0030 => "missing '=' in definition",
        SKY_P0031 => "malformed type declaration",
        SKY_P0040 => "only a type constructor can take arguments",
        SKY_P0041 => "expected a type",
        SKY_P0050 => "unclosed delimiter",
        SKY_P0060 => "malformed case expression",
        SKY_N0001 => "cannot find this value in scope",
        SKY_N0002 => "cannot find this type in scope",
        SKY_N0003 => "cannot find this constructor",
        SKY_N0004 => "unknown module or import",
        SKY_N0005 => "module has no such member",
        SKY_N0010 => "value defined more than once",
        SKY_N0011 => "constructor defined more than once",
        SKY_N0012 => "type defined more than once",
        SKY_T0001 => "type mismatch",
        SKY_T0002 => "infinite type",
        SKY_T0003 => "type inference exceeded its step budget",
        SKY_T0004 => "more parameters than the type signature describes",
        SKY_T0010 => "this case does not handle every possibility",
        SKY_T0011 => "redundant case branch",
        SKY_L0100 => "pattern kind not supported yet",
        SKY_L0101 => "operator not supported yet",
        SKY_L0102 => "polymorphic type variables not supported yet",
        SKY_L0103 => "function-valued parameters/returns not supported yet",
        SKY_L0104 => "only Task () is supported yet",
        SKY_L0105 => "parameter destructuring not supported yet",
        SKY_L0106 => "top-level function needs a type signature",
        SKY_L0107 => "first-class functions not supported yet",
        SKY_L0108 => "kernel function not available yet",
        SKY_L0200 => "expression nests too deeply for the backend",
        SKY_I0001 => "internal compiler error",
        SKY_I0010 => "intern: unresolved symbol",
        SKY_I0011 => "intern: symbol table exhausted",
        SKY_I0100 => "ICE: match on unknown variant",
        SKY_I0101 => "ICE: duplicate match arm",
        SKY_I0102 => "ICE: non-exhaustive match",
        SKY_I0103 => "ICE: match arm enum mismatch",
        SKY_I0200 => "ICE: no Rust name for symbol",
        SKY_I0201 => "ICE: dangling value/variant symbol",
        SKY_I0202 => "ICE: cross-module type-name collision",
        SKY_I0203 => "ICE: golden anchor missing",
        _ => "unknown error code",
    }
}

/// The embedded `skyc explain` page for a code.
///
/// Returns `None` for now: the explain pages land in a later phase. The
/// signature is fixed so callers (`skyc explain`, the CI page-coverage test)
/// can be written against it before the pages exist.
#[must_use]
pub const fn explain_page(_c: Code) -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every taxonomy code this module exposes, for round-trip / coverage tests.
    const ALL: &[Code] = &[
        SKY_P0001, SKY_P0002, SKY_P0003, SKY_P0010, SKY_P0011, SKY_P0012, SKY_P0013, SKY_P0020,
        SKY_P0021, SKY_P0030, SKY_P0031, SKY_P0040, SKY_P0041, SKY_P0050, SKY_P0060, SKY_N0001,
        SKY_N0002, SKY_N0003, SKY_N0004, SKY_N0005, SKY_N0010, SKY_N0011, SKY_N0012, SKY_T0001,
        SKY_T0002, SKY_T0003, SKY_T0004, SKY_T0010, SKY_T0011, SKY_L0100, SKY_L0101, SKY_L0102,
        SKY_L0103, SKY_L0104, SKY_L0105, SKY_L0106, SKY_L0107, SKY_L0108, SKY_L0200, SKY_I0001,
        SKY_I0010, SKY_I0011, SKY_I0100, SKY_I0101, SKY_I0102, SKY_I0103, SKY_I0200, SKY_I0201,
        SKY_I0202, SKY_I0203,
    ];

    #[test]
    fn taxonomy_has_fifty_codes() {
        assert_eq!(ALL.len(), 50);
    }

    #[test]
    fn every_code_has_a_nonempty_distinct_title() {
        for &c in ALL {
            assert!(!title(c).is_empty(), "{} has empty title", c.as_str());
        }
    }

    #[test]
    fn codes_are_distinct_and_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for &c in ALL {
            let s = c.as_str();
            assert!(s.starts_with("SKY-"), "{s} bad prefix");
            assert!(seen.insert(s), "{s} duplicated");
        }
        assert_eq!(seen.len(), 50);
    }

    #[test]
    fn explain_pages_absent_for_now() {
        for &c in ALL {
            assert!(explain_page(c).is_none());
        }
    }
}
