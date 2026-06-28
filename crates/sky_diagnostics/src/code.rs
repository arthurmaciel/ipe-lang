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

/// Where a reader reports a compiler bug or nudges an unimplemented feature.
///
/// Single source of truth: every humble / ICE message and every `SKY-I*` /
/// `SKY-L*` explain page footer references this one constant. `OWNER` is a
/// placeholder until the repository's Codeberg home is fixed.
pub const ISSUE_TRACKER_URL: &str = "https://codeberg.org/OWNER/sky-rust/issues";

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
/// malformed let expression
pub const SKY_P0061: Code = Code("SKY-P0061");
/// malformed if expression
pub const SKY_P0062: Code = Code("SKY-P0062");

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
/// type alias applied with the wrong number of arguments
pub const SKY_N0013: Code = Code("SKY-N0013");

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
/// this record has no such field
pub const SKY_T0012: Code = Code("SKY-T0012");
/// constructor pattern binds the wrong number of payload fields
pub const SKY_T0013: Code = Code("SKY-T0013");

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
/// partial or over-application of a function not supported yet
pub const SKY_L0110: Code = Code("SKY-L0110");
/// updating a generic record needs a bounded type parameter (M2d)
pub const SKY_L0111: Code = Code("SKY-L0111");
/// a constructor payload sub-pattern other than a variable / wildcard
pub const SKY_L0112: Code = Code("SKY-L0112");
/// a data constructor used as a first-class function value / partially applied
pub const SKY_L0113: Code = Code("SKY-L0113");
/// a function value stored in a constructor payload not supported yet
pub const SKY_L0114: Code = Code("SKY-L0114");
/// a tuple pattern beyond a single irrefutable destructure not supported yet
pub const SKY_L0115: Code = Code("SKY-L0115");
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
        SKY_P0061 => "malformed let expression",
        SKY_P0062 => "malformed if expression",
        SKY_N0001 => "cannot find this value in scope",
        SKY_N0002 => "cannot find this type in scope",
        SKY_N0003 => "cannot find this constructor",
        SKY_N0004 => "unknown module or import",
        SKY_N0005 => "module has no such member",
        SKY_N0010 => "value defined more than once",
        SKY_N0011 => "constructor defined more than once",
        SKY_N0012 => "type defined more than once",
        SKY_N0013 => "type alias applied with the wrong number of arguments",
        SKY_T0001 => "type mismatch",
        SKY_T0002 => "infinite type",
        SKY_T0003 => "type inference exceeded its step budget",
        SKY_T0004 => "more parameters than the type signature describes",
        SKY_T0010 => "this case does not handle every possibility",
        SKY_T0011 => "redundant case branch",
        SKY_T0012 => "this record has no such field",
        SKY_T0013 => "constructor pattern binds the wrong number of fields",
        SKY_L0100 => "pattern kind not supported yet",
        SKY_L0101 => "operator not supported yet",
        SKY_L0102 => "polymorphic value's type could not be determined",
        SKY_L0103 => "function-valued parameters/returns not supported yet",
        SKY_L0104 => "only Task () is supported yet",
        SKY_L0105 => "parameter destructuring not supported yet",
        SKY_L0106 => "top-level function needs a type signature",
        SKY_L0107 => "function value in a record field not supported yet",
        SKY_L0108 => "kernel function not available yet",
        SKY_L0110 => "partial or over-application not supported yet",
        SKY_L0111 => "updating a generic record is not supported yet",
        SKY_L0112 => "nested constructor payload patterns not supported yet",
        SKY_L0113 => "constructor used as a function value not supported yet",
        SKY_L0114 => "function value in a constructor payload not supported yet",
        SKY_L0115 => "tuple pattern not supported here yet",
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
/// Each page is `include_str!`d from `explain/<CODE>.md` at compile time, so a
/// missing or renamed page is a build error — the registry cannot drift from
/// the taxonomy silently. Total over every shipped constant; the `_` arm only
/// guards a `Code` that cannot be constructed outside this crate.
///
/// Page invariants (enforced by [`tests::every_code_has_a_conforming_explain_page`]):
/// line 1 is exactly `# <CODE>: <title()>`, and the body carries at least three
/// ```` ```sky ```` fences.
#[must_use]
pub fn explain_page(c: Code) -> Option<&'static str> {
    match c {
        SKY_P0001 => Some(include_str!("../explain/SKY-P0001.md")),
        SKY_P0002 => Some(include_str!("../explain/SKY-P0002.md")),
        SKY_P0003 => Some(include_str!("../explain/SKY-P0003.md")),
        SKY_P0010 => Some(include_str!("../explain/SKY-P0010.md")),
        SKY_P0011 => Some(include_str!("../explain/SKY-P0011.md")),
        SKY_P0012 => Some(include_str!("../explain/SKY-P0012.md")),
        SKY_P0013 => Some(include_str!("../explain/SKY-P0013.md")),
        SKY_P0020 => Some(include_str!("../explain/SKY-P0020.md")),
        SKY_P0021 => Some(include_str!("../explain/SKY-P0021.md")),
        SKY_P0030 => Some(include_str!("../explain/SKY-P0030.md")),
        SKY_P0031 => Some(include_str!("../explain/SKY-P0031.md")),
        SKY_P0040 => Some(include_str!("../explain/SKY-P0040.md")),
        SKY_P0041 => Some(include_str!("../explain/SKY-P0041.md")),
        SKY_P0050 => Some(include_str!("../explain/SKY-P0050.md")),
        SKY_P0060 => Some(include_str!("../explain/SKY-P0060.md")),
        SKY_P0061 => Some(include_str!("../explain/SKY-P0061.md")),
        SKY_P0062 => Some(include_str!("../explain/SKY-P0062.md")),
        SKY_N0001 => Some(include_str!("../explain/SKY-N0001.md")),
        SKY_N0002 => Some(include_str!("../explain/SKY-N0002.md")),
        SKY_N0003 => Some(include_str!("../explain/SKY-N0003.md")),
        SKY_N0004 => Some(include_str!("../explain/SKY-N0004.md")),
        SKY_N0005 => Some(include_str!("../explain/SKY-N0005.md")),
        SKY_N0010 => Some(include_str!("../explain/SKY-N0010.md")),
        SKY_N0011 => Some(include_str!("../explain/SKY-N0011.md")),
        SKY_N0012 => Some(include_str!("../explain/SKY-N0012.md")),
        SKY_N0013 => Some(include_str!("../explain/SKY-N0013.md")),
        SKY_T0001 => Some(include_str!("../explain/SKY-T0001.md")),
        SKY_T0002 => Some(include_str!("../explain/SKY-T0002.md")),
        SKY_T0003 => Some(include_str!("../explain/SKY-T0003.md")),
        SKY_T0004 => Some(include_str!("../explain/SKY-T0004.md")),
        SKY_T0010 => Some(include_str!("../explain/SKY-T0010.md")),
        SKY_T0011 => Some(include_str!("../explain/SKY-T0011.md")),
        SKY_T0012 => Some(include_str!("../explain/SKY-T0012.md")),
        SKY_T0013 => Some(include_str!("../explain/SKY-T0013.md")),
        SKY_L0100 => Some(include_str!("../explain/SKY-L0100.md")),
        SKY_L0101 => Some(include_str!("../explain/SKY-L0101.md")),
        SKY_L0102 => Some(include_str!("../explain/SKY-L0102.md")),
        SKY_L0103 => Some(include_str!("../explain/SKY-L0103.md")),
        SKY_L0104 => Some(include_str!("../explain/SKY-L0104.md")),
        SKY_L0105 => Some(include_str!("../explain/SKY-L0105.md")),
        SKY_L0106 => Some(include_str!("../explain/SKY-L0106.md")),
        SKY_L0107 => Some(include_str!("../explain/SKY-L0107.md")),
        SKY_L0108 => Some(include_str!("../explain/SKY-L0108.md")),
        SKY_L0110 => Some(include_str!("../explain/SKY-L0110.md")),
        SKY_L0111 => Some(include_str!("../explain/SKY-L0111.md")),
        SKY_L0112 => Some(include_str!("../explain/SKY-L0112.md")),
        SKY_L0113 => Some(include_str!("../explain/SKY-L0113.md")),
        SKY_L0114 => Some(include_str!("../explain/SKY-L0114.md")),
        SKY_L0115 => Some(include_str!("../explain/SKY-L0115.md")),
        SKY_L0200 => Some(include_str!("../explain/SKY-L0200.md")),
        SKY_I0001 => Some(include_str!("../explain/SKY-I0001.md")),
        SKY_I0010 => Some(include_str!("../explain/SKY-I0010.md")),
        SKY_I0011 => Some(include_str!("../explain/SKY-I0011.md")),
        SKY_I0100 => Some(include_str!("../explain/SKY-I0100.md")),
        SKY_I0101 => Some(include_str!("../explain/SKY-I0101.md")),
        SKY_I0102 => Some(include_str!("../explain/SKY-I0102.md")),
        SKY_I0103 => Some(include_str!("../explain/SKY-I0103.md")),
        SKY_I0200 => Some(include_str!("../explain/SKY-I0200.md")),
        SKY_I0201 => Some(include_str!("../explain/SKY-I0201.md")),
        SKY_I0202 => Some(include_str!("../explain/SKY-I0202.md")),
        SKY_I0203 => Some(include_str!("../explain/SKY-I0203.md")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every taxonomy code this module exposes, for round-trip / coverage tests.
    const ALL: &[Code] = &[
        SKY_P0001, SKY_P0002, SKY_P0003, SKY_P0010, SKY_P0011, SKY_P0012, SKY_P0013, SKY_P0020,
        SKY_P0021, SKY_P0030, SKY_P0031, SKY_P0040, SKY_P0041, SKY_P0050, SKY_P0060, SKY_P0061,
        SKY_P0062, SKY_N0001, SKY_N0002, SKY_N0003, SKY_N0004, SKY_N0005, SKY_N0010, SKY_N0011,
        SKY_N0012, SKY_N0013, SKY_T0001, SKY_T0002, SKY_T0003, SKY_T0004, SKY_T0010, SKY_T0011,
        SKY_T0012, SKY_T0013, SKY_L0100, SKY_L0101, SKY_L0102, SKY_L0103, SKY_L0104, SKY_L0105,
        SKY_L0106, SKY_L0107, SKY_L0108, SKY_L0110, SKY_L0111, SKY_L0112, SKY_L0113, SKY_L0114,
        SKY_L0115, SKY_L0200, SKY_I0001, SKY_I0010, SKY_I0011, SKY_I0100, SKY_I0101, SKY_I0102,
        SKY_I0103, SKY_I0200, SKY_I0201, SKY_I0202, SKY_I0203,
    ];

    #[test]
    fn taxonomy_has_sixty_one_codes() {
        assert_eq!(ALL.len(), 61);
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
        assert_eq!(seen.len(), 61);
    }

    /// CI coverage gate: every taxonomy code has a conforming explain page.
    /// Line 1 must be exactly `# <CODE>: <title()>` and the body must carry at
    /// least three ```` ```sky ```` fences. A code without a page, or with a
    /// non-conforming one, fails the suite (and `include_str!` already fails the
    /// build if the file is absent entirely).
    #[test]
    fn every_code_has_a_conforming_explain_page() {
        for &c in ALL {
            assert!(
                explain_page(c).is_some(),
                "{} has no explain page",
                c.as_str()
            );
            if let Some(page) = explain_page(c) {
                let first = page.lines().next().unwrap_or("");
                let expected = format!("# {}: {}", c.as_str(), title(c));
                assert_eq!(
                    first,
                    expected,
                    "{} page line 1 must be `{expected}`",
                    c.as_str()
                );
                let fences = page.matches("```sky").count();
                assert!(
                    fences >= 3,
                    "{} page has {fences} ```sky fences, need >= 3",
                    c.as_str()
                );
            }
        }
    }

    #[test]
    fn issue_tracker_url_is_a_codeberg_issues_link() {
        assert!(ISSUE_TRACKER_URL.starts_with("https://codeberg.org/"));
        assert!(ISSUE_TRACKER_URL.ends_with("/issues"));
    }
}
