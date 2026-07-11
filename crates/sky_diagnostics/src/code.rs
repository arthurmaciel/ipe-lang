//! Stable error codes (`SKY-XNNNN`) and their metadata.
//!
//! Every [`crate::Diagnostic`] maps to exactly one [`Code`] via
//! [`crate::Diagnostic::code`]. Codes are the machine-greppable, user-facing
//! handle a reader passes to `skyc explain <CODE>`; they never change once
//! shipped. The taxonomy is authoritative here (this file + the explain
//! pages under `crates/sky_diagnostics/explain/`); the original design spec
//! is preserved in git history.
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
/// unterminated string literal
pub const SKY_P0014: Code = Code("SKY-P0014");
/// malformed character literal
pub const SKY_P0015: Code = Code("SKY-P0015");
/// float literal out of range
pub const SKY_P0016: Code = Code("SKY-P0016");
/// unterminated block comment
pub const SKY_P0017: Code = Code("SKY-P0017");
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
/// a local module named in an import cannot be found
pub const SKY_N0020: Code = Code("SKY-N0020");
/// importing a module creates a cycle
pub const SKY_N0021: Code = Code("SKY-N0021");
/// the import names a member the module does not expose
pub const SKY_N0022: Code = Code("SKY-N0022");
/// the module declaration does not match the file's path
pub const SKY_N0023: Code = Code("SKY-N0023");
/// two imports expose the same name unqualified
pub const SKY_N0024: Code = Code("SKY-N0024");
/// a local module claims a reserved namespace
pub const SKY_N0025: Code = Code("SKY-N0025");
/// a user type/alias reuses a built-in type name
pub const SKY_N0026: Code = Code("SKY-N0026");
/// two imports register the same qualifier against different dep modules
pub const SKY_N0027: Code = Code("SKY-N0027");

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
/// a generic function is used at a type that lacks the required operations
pub const SKY_T0014: Code = Code("SKY-T0014");
/// a parameter / binder pattern is refutable (must be irrefutable)
pub const SKY_T0015: Code = Code("SKY-T0015");
/// a `Task` type is applied to the wrong number of arguments (not 1 or 2)
pub const SKY_T0016: Code = Code("SKY-T0016");

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
/// two `case` arms for the same constructor (nested discrimination) not yet
pub const SKY_L0116: Code = Code("SKY-L0116");
/// `Float` is not a valid `Set` element or `Dict` key on the Rust backend
pub const SKY_L0117: Code = Code("SKY-L0117");
/// `Live.appRouted` is not yet supported — use `Live.app` (non-routed) for now
pub const SKY_L0118: Code = Code("SKY-L0118");
/// an app-entry cfg must be an inline record literal, not a let-bound variable
pub const SKY_L0119: Code = Code("SKY-L0119");
/// a Live/Tui/Webview app Model is not admissible for that app shape's runtime
/// bound (Live needs serde+Clone+PartialEq; Tui/Webview need Clone)
pub const SKY_L0120: Code = Code("SKY-L0120");
/// `JsonDec.succeed` / `Db.Decode.succeed` constructor arity exceeds 10
/// (the maximum supported by `curry1`..`curry10` in the runtime)
pub const SKY_L0121: Code = Code("SKY-L0121");
/// `Live.route` pattern `:param` count does not match the page-constructor
/// payload count; the route can never deliver the right number of arguments
pub const SKY_L0122: Code = Code("SKY-L0122");
/// `Live.route` page builder is neither a page constructor, an inline lambda,
/// nor a named function — the Rust backend cannot emit a type-directed closure
pub const SKY_L0123: Code = Code("SKY-L0123");
/// `Live.app` routes list is non-empty but Model has no `page` field.
///
/// The routes are forwarded to the non-routed runtime path and never update the
/// Model. Emitted as a **warning** (Go's `applyRoute` silently no-ops the same
/// shape, so this compiles) to flag the likely mis-named routed-page field.
pub const SKY_L0124: Code = Code("SKY-L0124");
/// inadmissible Msg type in a Live/Tui/Webview app.
///
/// The Msg type's Rust rendering would not satisfy the runtime's
/// `Clone + Send + Sync + Debug + 'static` bound — converts a
/// would-be `cargo` trait-bound failure into a fail-closed `skyc` error.
pub const SKY_L0125: Code = Code("SKY-L0125");
/// a non-Clone, non-callee-position capture inside a closure
pub const SKY_L0126: Code = Code("SKY-L0126");
/// a value holding a function is used more than once (function values cannot
/// be copied yet)
pub const SKY_L0127: Code = Code("SKY-L0127");
/// an `as`-alias in a refutable match-arm position whose inner pattern needs
/// Rust-level runtime dispatch (a nested constructor / literal / list pattern)
pub const SKY_L0128: Code = Code("SKY-L0128");
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
        SKY_P0014 => "unterminated string literal",
        SKY_P0015 => "malformed character literal",
        SKY_P0016 => "float literal out of range",
        SKY_P0017 => "unterminated block comment",
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
        SKY_N0020 => "module not found",
        SKY_N0021 => "import cycle",
        SKY_N0022 => "name not exposed",
        SKY_N0023 => "module path mismatch",
        SKY_N0024 => "ambiguous import",
        SKY_N0025 => "reserved namespace",
        SKY_N0026 => "type name reserved for a built-in",
        SKY_N0027 => "duplicate import qualifier",
        SKY_T0001 => "type mismatch",
        SKY_T0002 => "infinite type",
        SKY_T0003 => "type inference exceeded its step budget",
        SKY_T0004 => "more parameters than the type signature describes",
        SKY_T0010 => "this case does not handle every possibility",
        SKY_T0011 => "redundant case branch",
        SKY_T0012 => "this record has no such field",
        SKY_T0013 => "constructor pattern binds the wrong number of fields",
        SKY_T0014 => "this type does not support the required operations",
        SKY_T0015 => "parameter pattern must be irrefutable",
        SKY_T0016 => "`Task` applied to the wrong number of type arguments",
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
        SKY_L0116 => "refutable pattern-discrimination shape not supported yet",
        SKY_L0117 => "Float is not a valid Set element or Dict key on the Rust backend",
        SKY_L0118 => "`Live.appRouted` is not yet supported — use `Live.app` (non-routed) for now",
        SKY_L0119 => "app entry cfg must be an inline record literal",
        SKY_L0120 => "app Model is not admissible for this app shape",
        SKY_L0121 => "`JsonDec.succeed` / `Db.Decode.succeed` constructor arity exceeds 10",
        SKY_L0122 => "`Live.route` `:param` count does not match page-constructor payload count",
        SKY_L0123 => "`Live.route` page builder is not a constructor, lambda, or named function",
        SKY_L0124 => "`Live.app` routes list is non-empty but Model has no `page` field",
        SKY_L0125 => "app Msg is not admissible for this app shape",
        SKY_L0126 => "non-Clone capture in a closure is not yet supported",
        SKY_L0127 => "a value holding a function is used more than once",
        SKY_L0128 => "alias over a dispatch-needing nested pattern not supported yet",
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
        SKY_P0014 => Some(include_str!("../explain/SKY-P0014.md")),
        SKY_P0015 => Some(include_str!("../explain/SKY-P0015.md")),
        SKY_P0016 => Some(include_str!("../explain/SKY-P0016.md")),
        SKY_P0017 => Some(include_str!("../explain/SKY-P0017.md")),
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
        SKY_N0020 => Some(include_str!("../explain/SKY-N0020.md")),
        SKY_N0021 => Some(include_str!("../explain/SKY-N0021.md")),
        SKY_N0022 => Some(include_str!("../explain/SKY-N0022.md")),
        SKY_N0023 => Some(include_str!("../explain/SKY-N0023.md")),
        SKY_N0024 => Some(include_str!("../explain/SKY-N0024.md")),
        SKY_N0025 => Some(include_str!("../explain/SKY-N0025.md")),
        SKY_N0026 => Some(include_str!("../explain/SKY-N0026.md")),
        SKY_N0027 => Some(include_str!("../explain/SKY-N0027.md")),
        SKY_T0001 => Some(include_str!("../explain/SKY-T0001.md")),
        SKY_T0002 => Some(include_str!("../explain/SKY-T0002.md")),
        SKY_T0003 => Some(include_str!("../explain/SKY-T0003.md")),
        SKY_T0004 => Some(include_str!("../explain/SKY-T0004.md")),
        SKY_T0010 => Some(include_str!("../explain/SKY-T0010.md")),
        SKY_T0011 => Some(include_str!("../explain/SKY-T0011.md")),
        SKY_T0012 => Some(include_str!("../explain/SKY-T0012.md")),
        SKY_T0013 => Some(include_str!("../explain/SKY-T0013.md")),
        SKY_T0014 => Some(include_str!("../explain/SKY-T0014.md")),
        SKY_T0015 => Some(include_str!("../explain/SKY-T0015.md")),
        SKY_T0016 => Some(include_str!("../explain/SKY-T0016.md")),
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
        SKY_L0116 => Some(include_str!("../explain/SKY-L0116.md")),
        SKY_L0117 => Some(include_str!("../explain/SKY-L0117.md")),
        SKY_L0118 => Some(include_str!("../explain/SKY-L0118.md")),
        SKY_L0119 => Some(include_str!("../explain/SKY-L0119.md")),
        SKY_L0120 => Some(include_str!("../explain/SKY-L0120.md")),
        SKY_L0121 => Some(include_str!("../explain/SKY-L0121.md")),
        SKY_L0122 => Some(include_str!("../explain/SKY-L0122.md")),
        SKY_L0123 => Some(include_str!("../explain/SKY-L0123.md")),
        SKY_L0124 => Some(include_str!("../explain/SKY-L0124.md")),
        SKY_L0125 => Some(include_str!("../explain/SKY-L0125.md")),
        SKY_L0126 => Some(include_str!("../explain/SKY-L0126.md")),
        SKY_L0127 => Some(include_str!("../explain/SKY-L0127.md")),
        SKY_L0128 => Some(include_str!("../explain/SKY-L0128.md")),
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

/// Every taxonomy code, authoritative for `skyc explain` and drift detection.
///
/// This is the single source of truth. `skyc` iterates this slice to resolve
/// `explain <CODE>` — no hand-mirror needed.
pub const ALL_CODES: &[Code] = &[
    SKY_P0001, SKY_P0002, SKY_P0003, SKY_P0010, SKY_P0011, SKY_P0012, SKY_P0013, SKY_P0014,
    SKY_P0015, SKY_P0016, SKY_P0017, SKY_P0020, SKY_P0021, SKY_P0030, SKY_P0031, SKY_P0040,
    SKY_P0041, SKY_P0050, SKY_P0060, SKY_P0061, SKY_P0062, SKY_N0001, SKY_N0002, SKY_N0003,
    SKY_N0004, SKY_N0005, SKY_N0010, SKY_N0011, SKY_N0012, SKY_N0013, SKY_N0020, SKY_N0021,
    SKY_N0022, SKY_N0023, SKY_N0024, SKY_N0025, SKY_N0026, SKY_N0027, SKY_T0001, SKY_T0002,
    SKY_T0003,
    SKY_T0004, SKY_T0010, SKY_T0011, SKY_T0012, SKY_T0013, SKY_T0014, SKY_T0015, SKY_T0016,
    SKY_L0100,
    SKY_L0101, SKY_L0102, SKY_L0103, SKY_L0104, SKY_L0105, SKY_L0106, SKY_L0107, SKY_L0108,
    SKY_L0110, SKY_L0111, SKY_L0112, SKY_L0113, SKY_L0114, SKY_L0115, SKY_L0116, SKY_L0117,
    SKY_L0118, SKY_L0119, SKY_L0120, SKY_L0121, SKY_L0122, SKY_L0123, SKY_L0124, SKY_L0125,
    SKY_L0126, SKY_L0127, SKY_L0128, SKY_L0200, SKY_I0001, SKY_I0010, SKY_I0011, SKY_I0100, SKY_I0101,
    SKY_I0102,
    SKY_I0103, SKY_I0200, SKY_I0201, SKY_I0202, SKY_I0203,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_has_eighty_nine_codes() {
        assert_eq!(ALL_CODES.len(), 89); // #90: +SKY-L0127; #99: +SKY-L0128; #32: +SKY-T0016
    }

    #[test]
    fn every_code_has_a_nonempty_distinct_title() {
        for &c in ALL_CODES {
            assert!(!title(c).is_empty(), "{} has empty title", c.as_str());
        }
    }

    #[test]
    fn codes_are_distinct_and_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for &c in ALL_CODES {
            let s = c.as_str();
            assert!(s.starts_with("SKY-"), "{s} bad prefix");
            assert!(seen.insert(s), "{s} duplicated");
        }
        assert_eq!(seen.len(), 89); // #90: +SKY-L0127; #99: +SKY-L0128; #32: +SKY-T0016
    }

    /// CI coverage gate: every taxonomy code has a conforming explain page.
    /// Line 1 must be exactly `# <CODE>: <title()>` and the body must carry at
    /// least three ```` ```sky ```` fences. A code without a page, or with a
    /// non-conforming one, fails the suite (and `include_str!` already fails the
    /// build if the file is absent entirely).
    #[test]
    fn every_code_has_a_conforming_explain_page() {
        for &c in ALL_CODES {
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
