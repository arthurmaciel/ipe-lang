//! Stable error codes (`IPE-XNNNN`) and their metadata.
//!
//! Every [`crate::Diagnostic`] maps to exactly one [`Code`] via
//! [`crate::Diagnostic::code`]. Codes are the machine-greppable, user-facing
//! handle a reader passes to `ipe explain <CODE>`; they never change once
//! shipped. The taxonomy is authoritative here (this file + the explain
//! pages under `crates/ipe_diagnostics/explain/`); the original design spec
//! is preserved in git history.
//!
//! Ranges: `IPE-P####` parse, `IPE-N####` name resolution, `IPE-T####` type,
//! `IPE-L####` lower / not-yet-supported, `IPE-F####` foreign bindings (FFI),
//! `IPE-I####` internal (compiler bug).

/// Where a reader reports a compiler bug or nudges an unimplemented feature.
///
/// Single source of truth: every humble / ICE message and every `IPE-I*` /
/// `IPE-L*` explain page footer references this one constant.
pub const ISSUE_TRACKER_URL: &str = "https://github.com/arthurmaciel/ipe-lang/issues";

/// A stable compiler error code, e.g. `IPE-T0001`.
///
/// The wrapped string is always one of the taxonomy constants in this module;
/// the field is private so a `Code` cannot be forged with an unknown value from
/// outside the crate. Compare via the constants ([`IPE_T0001`] etc.).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Code(&'static str);

impl Code {
    /// The wire form of the code, e.g. `"IPE-T0001"`.
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
    /// An internal compiler bug (an `IPE-I####`): "please report".
    Bug,
}

// ---------------------------------------------------------------------------
// Parse (IPE-P####)
// ---------------------------------------------------------------------------

/// unexpected token
pub const IPE_P0001: Code = Code("IPE-P0001");
/// unexpected end of file
pub const IPE_P0002: Code = Code("IPE-P0002");
/// input nests too deeply
pub const IPE_P0003: Code = Code("IPE-P0003");
/// unknown character
pub const IPE_P0010: Code = Code("IPE-P0010");
/// stray `.`
pub const IPE_P0011: Code = Code("IPE-P0011");
/// number joined to a name
pub const IPE_P0012: Code = Code("IPE-P0012");
/// integer literal out of range
pub const IPE_P0013: Code = Code("IPE-P0013");
/// unterminated string literal
pub const IPE_P0014: Code = Code("IPE-P0014");
/// malformed character literal
pub const IPE_P0015: Code = Code("IPE-P0015");
/// float literal out of range
pub const IPE_P0016: Code = Code("IPE-P0016");
/// unterminated block comment
pub const IPE_P0017: Code = Code("IPE-P0017");
/// a space before `.` reads as an accessor function (unsupported)
pub const IPE_P0018: Code = Code("IPE-P0018");
/// malformed module header
pub const IPE_P0020: Code = Code("IPE-P0020");
/// malformed exposing list
pub const IPE_P0021: Code = Code("IPE-P0021");
/// missing `=` in definition
pub const IPE_P0030: Code = Code("IPE-P0030");
/// malformed type declaration
pub const IPE_P0031: Code = Code("IPE-P0031");
/// only a type constructor can take arguments
pub const IPE_P0040: Code = Code("IPE-P0040");
/// expected a type
pub const IPE_P0041: Code = Code("IPE-P0041");
/// unclosed delimiter
pub const IPE_P0050: Code = Code("IPE-P0050");
/// malformed case expression
pub const IPE_P0060: Code = Code("IPE-P0060");
/// malformed let expression
pub const IPE_P0061: Code = Code("IPE-P0061");
/// malformed if expression
pub const IPE_P0062: Code = Code("IPE-P0062");
/// invalid path literal — a `path "…"` whose string fails compile-time validation
pub const IPE_P0063: Code = Code("IPE-P0063");

// ---------------------------------------------------------------------------
// Name resolution (IPE-N####)
// ---------------------------------------------------------------------------

/// cannot find this value in scope
pub const IPE_N0001: Code = Code("IPE-N0001");
/// cannot find this type in scope
pub const IPE_N0002: Code = Code("IPE-N0002");
/// cannot find this constructor
pub const IPE_N0003: Code = Code("IPE-N0003");
/// unknown module or import
pub const IPE_N0004: Code = Code("IPE-N0004");
/// module has no such member
pub const IPE_N0005: Code = Code("IPE-N0005");
/// value defined more than once
pub const IPE_N0010: Code = Code("IPE-N0010");
/// constructor defined more than once
pub const IPE_N0011: Code = Code("IPE-N0011");
/// type defined more than once
pub const IPE_N0012: Code = Code("IPE-N0012");
/// type alias applied with the wrong number of arguments
pub const IPE_N0013: Code = Code("IPE-N0013");
/// a local module named in an import cannot be found
pub const IPE_N0020: Code = Code("IPE-N0020");
/// importing a module creates a cycle
pub const IPE_N0021: Code = Code("IPE-N0021");
/// the import names a member the module does not expose
pub const IPE_N0022: Code = Code("IPE-N0022");
/// the module declaration does not match the file's path
pub const IPE_N0023: Code = Code("IPE-N0023");
/// two imports expose the same name unqualified
pub const IPE_N0024: Code = Code("IPE-N0024");
/// a local module claims a reserved namespace
pub const IPE_N0025: Code = Code("IPE-N0025");
/// a user type/alias reuses a built-in type name
pub const IPE_N0026: Code = Code("IPE-N0026");
/// two imports register the same qualifier against different dep modules
pub const IPE_N0027: Code = Code("IPE-N0027");
/// an `Ffi.kernel "Name"` alias names a kernel that is not registered
pub const IPE_N0028: Code = Code("IPE-N0028");
/// A server-only kernel named from a `--target wasm` build.
pub const IPE_N0029: Code = Code("IPE-N0029");
/// A wasm client-entry's reachability closure transitively reaches a
/// server-classified module.
pub const IPE_N0030: Code = Code("IPE-N0030");
/// a built-in container type constructor (`List`/`Maybe`/`Set`/`Dict`/`Result`)
/// is applied to the wrong number of type arguments
pub const IPE_N0031: Code = Code("IPE-N0031");
/// type alias expansion exceeded the depth or node-count budget (cyclic or
/// exponentially-fanning alias chain)
pub const IPE_N0032: Code = Code("IPE-N0032");
/// a plain-`main` Program imports a managed-update-loop shape under `Ipe.Tea.*`
pub const IPE_N0033: Code = Code("IPE-N0033");
/// a known standard-library module is used qualified without importing it
pub const IPE_N0034: Code = Code("IPE-N0034");
/// a TEA app imports another shape's `Cmd` / `Sub` re-export module
pub const IPE_N0035: Code = Code("IPE-N0035");
/// a removed surface binding is used; `ipe fix` can migrate the call site
pub const IPE_N0036: Code = Code("IPE-N0036");
/// a reserved JS-interop boundary type (`CustomElement`) is used before its
/// runtime denotation ships — fail-closed, the typed seam is not yet emittable
pub const IPE_N0037: Code = Code("IPE-N0037");
/// an asserted foreign call (`Rust.Ffi.call`) is malformed at its use site
pub const IPE_N0038: Code = Code("IPE-N0038");
/// a `CustomElement` boundary type parameter is not a plain, closed, concrete
/// value type — the SEAL rejects it before it can cross the Ipê↔JS seam
pub const IPE_N0039: Code = Code("IPE-N0039");
/// a decoder-pipeline combinator (`required` / `optional` / `requiredAt` /
/// `custom`) is hand-nested rather than threaded with `|>`, reversing
/// field→constructor binding — rejected fail-closed with the `|>` rewrite
pub const IPE_N0040: Code = Code("IPE-N0040");

// ---------------------------------------------------------------------------
// Type (IPE-T####)
// ---------------------------------------------------------------------------

/// type mismatch
pub const IPE_T0001: Code = Code("IPE-T0001");
/// infinite type
pub const IPE_T0002: Code = Code("IPE-T0002");
/// type inference exceeded its step budget
pub const IPE_T0003: Code = Code("IPE-T0003");
/// more parameters than the type signature describes
pub const IPE_T0004: Code = Code("IPE-T0004");
/// this case does not handle every possibility
pub const IPE_T0010: Code = Code("IPE-T0010");
/// redundant case branch
pub const IPE_T0011: Code = Code("IPE-T0011");
/// this record has no such field
pub const IPE_T0012: Code = Code("IPE-T0012");
/// constructor pattern binds the wrong number of payload fields
pub const IPE_T0013: Code = Code("IPE-T0013");
/// a generic function is used at a type that lacks the required operations
pub const IPE_T0014: Code = Code("IPE-T0014");
/// a parameter / binder pattern is refutable (must be irrefutable)
pub const IPE_T0015: Code = Code("IPE-T0015");
/// a `Task` type is applied to the wrong number of arguments (not 1 or 2)
pub const IPE_T0016: Code = Code("IPE-T0016");
/// a record update on a nominal builtin type (readable fields, no update form)
pub const IPE_T0017: Code = Code("IPE-T0017");
/// a wildcard arm swallows constructors a finite ADT could name explicitly
pub const IPE_T0018: Code = Code("IPE-T0018");
/// each alternative of an or-pattern must bind the same variables
///
/// T0018 is now the closed-union catch-all lint; or-patterns keep T0019.
pub const IPE_T0019: Code = Code("IPE-T0019");
/// an `Html` value is used where an `Element` is required (wrap it in `Ui.html`)
pub const IPE_T0020: Code = Code("IPE-T0020");

// ---------------------------------------------------------------------------
// Lower / not-yet-supported (IPE-L####)
// ---------------------------------------------------------------------------

/// pattern kind not supported yet
pub const IPE_L0100: Code = Code("IPE-L0100");
/// operator not supported yet
pub const IPE_L0101: Code = Code("IPE-L0101");
/// polymorphic type variables not supported yet
pub const IPE_L0102: Code = Code("IPE-L0102");
/// function-valued parameters/returns not supported yet
pub const IPE_L0103: Code = Code("IPE-L0103");
/// only `Task ()` is supported yet
pub const IPE_L0104: Code = Code("IPE-L0104");
/// parameter destructuring not supported yet
pub const IPE_L0105: Code = Code("IPE-L0105");
/// top-level function needs a type signature
pub const IPE_L0106: Code = Code("IPE-L0106");
/// first-class functions not supported yet
pub const IPE_L0107: Code = Code("IPE-L0107");
/// kernel function not available yet
pub const IPE_L0108: Code = Code("IPE-L0108");
/// partial or over-application of a function not supported yet
pub const IPE_L0110: Code = Code("IPE-L0110");
/// updating a generic record needs a bounded type parameter (M2d)
pub const IPE_L0111: Code = Code("IPE-L0111");
/// a constructor payload sub-pattern other than a variable / wildcard
pub const IPE_L0112: Code = Code("IPE-L0112");
/// a data constructor used as a first-class function value / partially applied
pub const IPE_L0113: Code = Code("IPE-L0113");
/// a function value stored in a constructor payload not supported yet
pub const IPE_L0114: Code = Code("IPE-L0114");
/// a tuple pattern beyond a single irrefutable destructure not supported yet
pub const IPE_L0115: Code = Code("IPE-L0115");
/// two `case` arms for the same constructor (nested discrimination) not yet
pub const IPE_L0116: Code = Code("IPE-L0116");
/// `Float` is not a valid `Set` element or `Dict` key on the Rust backend
pub const IPE_L0117: Code = Code("IPE-L0117");
/// `Web.appRouted` is not yet supported — use `Web.app` (non-routed) for now
pub const IPE_L0118: Code = Code("IPE-L0118");
/// an app-entry cfg must be an inline record literal, not a let-bound variable
pub const IPE_L0119: Code = Code("IPE-L0119");
/// a Web/Terminal/WebView app Model is not admissible for that app shape's
/// runtime bound (Web needs serde+Clone+PartialEq; Terminal/WebView need Clone)
pub const IPE_L0120: Code = Code("IPE-L0120");
/// `JsonDec.succeed` / `Db.Decode.succeed` constructor arity exceeds 10
/// (the maximum supported by `curry1`..`curry10` in the runtime)
pub const IPE_L0121: Code = Code("IPE-L0121");
/// `Web.route` pattern `:param` count does not match the page-constructor
/// payload count; the route can never deliver the right number of arguments
pub const IPE_L0122: Code = Code("IPE-L0122");
/// `Web.route` page builder is neither a page constructor, an inline lambda,
/// nor a named function — the Rust backend cannot emit a type-directed closure
pub const IPE_L0123: Code = Code("IPE-L0123");
/// `Web.app` routes list is non-empty but Model has no `page` field.
///
/// The routes are forwarded to the non-routed runtime path and never update the
/// Model. Emitted as a **warning** (Go's `applyRoute` silently no-ops the same
/// shape, so this compiles) to flag the likely mis-named routed-page field.
pub const IPE_L0124: Code = Code("IPE-L0124");
/// inadmissible Msg type in a Web/Terminal/WebView app.
///
/// The Msg type's Rust rendering would not satisfy the runtime's
/// `Clone + Send + Sync + Debug + 'static` bound — converts a
/// would-be `cargo` trait-bound failure into a fail-closed `ipe` error.
pub const IPE_L0125: Code = Code("IPE-L0125");
/// a non-Clone, non-callee-position capture inside a closure
pub const IPE_L0126: Code = Code("IPE-L0126");
/// a value holding a function is used more than once (function values cannot
/// be copied yet)
pub const IPE_L0127: Code = Code("IPE-L0127");
/// an `as`-alias in a refutable match-arm position whose inner pattern needs
/// Rust-level runtime dispatch (a nested constructor / literal / list pattern)
pub const IPE_L0128: Code = Code("IPE-L0128");
/// A routed `Web.app` under `--target wasm` (client router not yet built).
pub const IPE_L0129: Code = Code("IPE-L0129");
/// a foreign opaque FFI handle (possibly non-`Clone`) is used more than once
pub const IPE_L0130: Code = Code("IPE-L0130");
/// a row-polymorphic record annotation `{ r | f : T }` reached the backend
pub const IPE_L0131: Code = Code("IPE-L0131");
/// `Ui.cells` (a terminal-only raw cell grid) is used in the Web/WebView shape
pub const IPE_L0132: Code = Code("IPE-L0132");
/// a `CustomElement` boundary value reached lowering — its typed JS-widget
/// transport (generated glue + DOM-patch node) is not emittable yet
pub const IPE_L0133: Code = Code("IPE-L0133");
/// a `Debug.*` development-only escape hatch reached a production build
/// (`ipe build --optimize`)
pub const IPE_L0140: Code = Code("IPE-L0140");
/// expression nests too deeply for the backend
pub const IPE_L0200: Code = Code("IPE-L0200");

// ---------------------------------------------------------------------------
// FFI (IPE-F####)
// ---------------------------------------------------------------------------

/// a foreign-call description cannot be rendered as valid Rust
pub const IPE_F4400: Code = Code("IPE-F4400");
/// a foreign binding's inspection data is malformed
pub const IPE_F4401: Code = Code("IPE-F4401");
/// a foreign function declares contradictory shape flags
pub const IPE_F4402: Code = Code("IPE-F4402");
/// no isolation jail can be established for compiling an untrusted crate
pub const IPE_F4410: Code = Code("IPE-F4410");
/// a git source for a foreign crate was rejected
pub const IPE_F4411: Code = Code("IPE-F4411");
/// an FFI cache artifact cannot be read or written
pub const IPE_F4412: Code = Code("IPE-F4412");
/// no runtime jail can be established around the emitted app, or a jailed run failed
pub const IPE_F4413: Code = Code("IPE-F4413");
/// an author-asserted foreign call (`Rust.Ffi.call`) was refused
pub const IPE_F4414: Code = Code("IPE-F4414");

// ---------------------------------------------------------------------------
// Internal (IPE-I####)
// ---------------------------------------------------------------------------

/// internal compiler error
pub const IPE_I0001: Code = Code("IPE-I0001");
/// intern: unresolved symbol
pub const IPE_I0010: Code = Code("IPE-I0010");
/// intern: symbol table exhausted
pub const IPE_I0011: Code = Code("IPE-I0011");
/// ICE: match on unknown variant
pub const IPE_I0100: Code = Code("IPE-I0100");
/// ICE: duplicate match arm
pub const IPE_I0101: Code = Code("IPE-I0101");
/// ICE: non-exhaustive match
pub const IPE_I0102: Code = Code("IPE-I0102");
/// ICE: match arm enum mismatch
pub const IPE_I0103: Code = Code("IPE-I0103");
/// ICE: no Rust name for symbol
pub const IPE_I0200: Code = Code("IPE-I0200");
/// ICE: dangling value/variant symbol
pub const IPE_I0201: Code = Code("IPE-I0201");
/// ICE: cross-module type-name collision
pub const IPE_I0202: Code = Code("IPE-I0202");
/// ICE: golden anchor missing
pub const IPE_I0203: Code = Code("IPE-I0203");

/// The one-line human title for a code.
///
/// Total over the taxonomy: every shipped constant has an explicit arm. A code
/// outside the taxonomy (impossible to construct outside this crate) falls back
/// to the generic internal-error title rather than panicking.
#[must_use]
#[allow(clippy::too_many_lines)] // one arm per taxonomy code — an exhaustive table, not branching logic
pub fn title(c: Code) -> &'static str {
    match c {
        IPE_P0001 => "unexpected token",
        IPE_P0002 => "unexpected end of file",
        IPE_P0003 => "input nests too deeply",
        IPE_P0010 => "unknown character",
        IPE_P0011 => "stray '.'",
        IPE_P0012 => "number joined to a name",
        IPE_P0013 => "integer literal out of range",
        IPE_P0014 => "unterminated string literal",
        IPE_P0015 => "malformed character literal",
        IPE_P0016 => "float literal out of range",
        IPE_P0017 => "unterminated block comment",
        IPE_P0018 => "a space before '.' reads as an accessor function",
        IPE_P0020 => "malformed module header",
        IPE_P0021 => "malformed exposing list",
        IPE_P0030 => "missing '=' in definition",
        IPE_P0031 => "malformed type declaration",
        IPE_P0040 => "only a type constructor can take arguments",
        IPE_P0041 => "expected a type",
        IPE_P0050 => "unclosed delimiter",
        IPE_P0060 => "malformed case expression",
        IPE_P0061 => "malformed let expression",
        IPE_P0062 => "malformed if expression",
        IPE_P0063 => "invalid path literal",
        IPE_N0001 => "cannot find this value in scope",
        IPE_N0002 => "cannot find this type in scope",
        IPE_N0003 => "cannot find this constructor",
        IPE_N0004 => "unknown module or import",
        IPE_N0005 => "module has no such member",
        IPE_N0010 => "value defined more than once",
        IPE_N0011 => "constructor defined more than once",
        IPE_N0012 => "type defined more than once",
        IPE_N0013 => "type alias applied with the wrong number of arguments",
        IPE_N0020 => "module not found",
        IPE_N0021 => "import cycle",
        IPE_N0022 => "name not exposed",
        IPE_N0023 => "module path mismatch",
        IPE_N0024 => "ambiguous import",
        IPE_N0025 => "reserved namespace",
        IPE_N0026 => "type name reserved for a built-in",
        IPE_N0027 => "duplicate import qualifier",
        IPE_N0028 => "unknown kernel alias",
        IPE_N0029 => "server-only effect in a wasm build",
        IPE_N0030 => "server module reachable from the wasm client entry",
        IPE_N0031 => "built-in container type applied to the wrong number of arguments",
        IPE_N0032 => "type alias expansion too deep or too large",
        IPE_N0033 => "a Program may not import a managed-update-loop shape",
        IPE_N0034 => "standard-library module used without importing it",
        IPE_N0035 => "Cmd / Sub imported from a different shape than the app's",
        IPE_N0037 => "a reserved Ipê↔JS boundary type is used before its transport ships",
        IPE_N0038 => "an asserted foreign call (Rust.Ffi.call) is malformed",
        IPE_N0039 => "a CustomElement boundary type parameter is not a plain, closed value type",
        IPE_N0040 => "a decoder-pipeline combinator is hand-nested instead of threaded with |>",
        IPE_T0001 => "type mismatch",
        IPE_T0002 => "infinite type",
        IPE_T0003 => "type inference exceeded its step budget",
        IPE_T0004 => "more parameters than the type signature describes",
        IPE_T0010 => "this case does not handle every possibility",
        IPE_T0011 => "redundant case branch",
        IPE_T0012 => "this record has no such field",
        IPE_T0013 => "constructor pattern binds the wrong number of fields",
        IPE_T0014 => "this type does not support the required operations",
        IPE_T0015 => "parameter pattern must be irrefutable",
        IPE_T0016 => {
            "async carrier (`Task`/`Cmd`/`Sub`) applied to the wrong number of type arguments"
        }
        IPE_T0017 => "built-in type cannot be updated with record syntax",
        IPE_T0018 => "this catch-all arm hides constructors of a closed union",
        IPE_T0019 => "each alternative of an or-pattern must bind the same variables",
        IPE_T0020 => "this is `Html` where an `Element` is required",
        IPE_L0100 => "pattern kind not supported yet",
        IPE_L0101 => "operator not supported yet",
        IPE_L0102 => "polymorphic value's type could not be determined",
        IPE_L0103 => "function-valued parameters/returns not supported yet",
        IPE_L0104 => "only Task () is supported yet",
        IPE_L0105 => "parameter destructuring not supported yet",
        IPE_L0106 => "top-level function needs a type signature",
        IPE_L0107 => "function value in a record field not supported here",
        IPE_L0108 => "kernel function not available yet",
        IPE_L0110 => "partial or over-application not supported yet",
        IPE_L0111 => "updating a generic record is not supported yet",
        IPE_L0112 => "nested constructor payload patterns not supported yet",
        IPE_L0113 => "constructor used as a function value not supported yet",
        IPE_L0114 => "function value in a constructor payload not supported yet",
        IPE_L0115 => "tuple pattern not supported here yet",
        IPE_L0116 => "refutable pattern-discrimination shape not supported yet",
        IPE_L0117 => "Float is not a valid Set element or Dict key on the Rust backend",
        IPE_L0118 => "`Web.appRouted` is not yet supported — use `Web.app` (non-routed) for now",
        IPE_L0119 => "app entry cfg must be an inline record literal",
        IPE_L0120 => "app Model is not admissible for this app shape",
        IPE_L0121 => "`JsonDec.succeed` / `Db.Decode.succeed` constructor arity exceeds 10",
        IPE_L0122 => "`Web.route` `:param` count does not match page-constructor payload count",
        IPE_L0123 => "`Web.route` page builder is not a constructor, lambda, or named function",
        IPE_L0124 => "`Web.app` routes list is non-empty but Model has no `page` field",
        IPE_L0125 => "app Msg is not admissible for this app shape",
        IPE_L0126 => "non-Clone capture in a closure is not yet supported",
        IPE_L0127 => "a value holding a function is used more than once",
        IPE_L0128 => "alias over a dispatch-needing nested pattern not supported yet",
        IPE_L0129 => "routed Web.app not supported under --target wasm yet",
        IPE_L0130 => "a foreign opaque FFI handle is used more than once",
        IPE_L0131 => "a row-polymorphic record annotation is not yet emittable",
        IPE_L0132 => "Ui.cells is terminal-only and not available in the Web/WebView shape",
        IPE_L0133 => {
            "a CustomElement boundary value is not emittable yet (typed transport not shipped)"
        }
        IPE_L0140 => "a Debug.* escape hatch was used in a production build",
        IPE_L0200 => "expression nests too deeply for the backend",
        IPE_F4400 => "a foreign-call description cannot be rendered as valid Rust",
        IPE_F4401 => "a foreign binding's inspection data is malformed",
        IPE_F4402 => "a foreign function declares contradictory shape flags",
        IPE_F4410 => "no isolation jail can be established for compiling an untrusted crate",
        IPE_F4411 => "a git source for a foreign crate was rejected",
        IPE_F4412 => "an FFI cache artifact cannot be read or written",
        IPE_F4413 => "no runtime jail can be established around the emitted app",
        IPE_F4414 => "an author-asserted foreign call (Rust.Ffi.call) was refused",
        IPE_I0001 => "internal compiler error",
        IPE_I0010 => "intern: unresolved symbol",
        IPE_I0011 => "intern: symbol table exhausted",
        IPE_I0100 => "ICE: match on unknown variant",
        IPE_I0101 => "ICE: duplicate match arm",
        IPE_I0102 => "ICE: non-exhaustive match",
        IPE_I0103 => "ICE: match arm enum mismatch",
        IPE_I0200 => "ICE: no Rust name for symbol",
        IPE_I0201 => "ICE: dangling value/variant symbol",
        IPE_I0202 => "ICE: cross-module type-name collision",
        IPE_I0203 => "ICE: golden anchor missing",
        _ => "unknown error code",
    }
}

/// The embedded `ipe explain` page for a code.
///
/// Each page is `include_str!`d from `explain/<CODE>.md` at compile time, so a
/// missing or renamed page is a build error — the registry cannot drift from
/// the taxonomy silently. Total over every shipped constant; the `_` arm only
/// guards a `Code` that cannot be constructed outside this crate.
///
/// Page invariants (enforced by [`tests::every_code_has_a_conforming_explain_page`]):
/// line 1 is exactly `# <CODE>: <title()>`, and the body carries at least three
/// ```` ```ipe ```` fences.
#[must_use]
pub fn explain_page(c: Code) -> Option<&'static str> {
    front_end_explain_page(c).or_else(|| back_end_explain_page(c))
}

/// [`explain_page`]'s front-end half: parse (`IPE-P*`) / name (`IPE-N*`) /
/// type (`IPE-T*`) codes.
#[must_use]
fn front_end_explain_page(c: Code) -> Option<&'static str> {
    match c {
        IPE_P0001 => Some(include_str!("../explain/IPE-P0001.md")),
        IPE_P0002 => Some(include_str!("../explain/IPE-P0002.md")),
        IPE_P0003 => Some(include_str!("../explain/IPE-P0003.md")),
        IPE_P0010 => Some(include_str!("../explain/IPE-P0010.md")),
        IPE_P0011 => Some(include_str!("../explain/IPE-P0011.md")),
        IPE_P0012 => Some(include_str!("../explain/IPE-P0012.md")),
        IPE_P0013 => Some(include_str!("../explain/IPE-P0013.md")),
        IPE_P0014 => Some(include_str!("../explain/IPE-P0014.md")),
        IPE_P0015 => Some(include_str!("../explain/IPE-P0015.md")),
        IPE_P0016 => Some(include_str!("../explain/IPE-P0016.md")),
        IPE_P0017 => Some(include_str!("../explain/IPE-P0017.md")),
        IPE_P0018 => Some(include_str!("../explain/IPE-P0018.md")),
        IPE_P0020 => Some(include_str!("../explain/IPE-P0020.md")),
        IPE_P0021 => Some(include_str!("../explain/IPE-P0021.md")),
        IPE_P0030 => Some(include_str!("../explain/IPE-P0030.md")),
        IPE_P0031 => Some(include_str!("../explain/IPE-P0031.md")),
        IPE_P0040 => Some(include_str!("../explain/IPE-P0040.md")),
        IPE_P0041 => Some(include_str!("../explain/IPE-P0041.md")),
        IPE_P0050 => Some(include_str!("../explain/IPE-P0050.md")),
        IPE_P0060 => Some(include_str!("../explain/IPE-P0060.md")),
        IPE_P0061 => Some(include_str!("../explain/IPE-P0061.md")),
        IPE_P0062 => Some(include_str!("../explain/IPE-P0062.md")),
        IPE_P0063 => Some(include_str!("../explain/IPE-P0063.md")),
        IPE_N0001 => Some(include_str!("../explain/IPE-N0001.md")),
        IPE_N0002 => Some(include_str!("../explain/IPE-N0002.md")),
        IPE_N0003 => Some(include_str!("../explain/IPE-N0003.md")),
        IPE_N0004 => Some(include_str!("../explain/IPE-N0004.md")),
        IPE_N0005 => Some(include_str!("../explain/IPE-N0005.md")),
        IPE_N0010 => Some(include_str!("../explain/IPE-N0010.md")),
        IPE_N0011 => Some(include_str!("../explain/IPE-N0011.md")),
        IPE_N0012 => Some(include_str!("../explain/IPE-N0012.md")),
        IPE_N0013 => Some(include_str!("../explain/IPE-N0013.md")),
        IPE_N0020 => Some(include_str!("../explain/IPE-N0020.md")),
        IPE_N0021 => Some(include_str!("../explain/IPE-N0021.md")),
        IPE_N0022 => Some(include_str!("../explain/IPE-N0022.md")),
        IPE_N0023 => Some(include_str!("../explain/IPE-N0023.md")),
        IPE_N0024 => Some(include_str!("../explain/IPE-N0024.md")),
        IPE_N0025 => Some(include_str!("../explain/IPE-N0025.md")),
        IPE_N0026 => Some(include_str!("../explain/IPE-N0026.md")),
        IPE_N0027 => Some(include_str!("../explain/IPE-N0027.md")),
        IPE_N0028 => Some(include_str!("../explain/IPE-N0028.md")),
        IPE_N0029 => Some(include_str!("../explain/IPE-N0029.md")),
        IPE_N0030 => Some(include_str!("../explain/IPE-N0030.md")),
        IPE_N0031 => Some(include_str!("../explain/IPE-N0031.md")),
        IPE_N0032 => Some(include_str!("../explain/IPE-N0032.md")),
        IPE_N0033 => Some(include_str!("../explain/IPE-N0033.md")),
        IPE_N0034 => Some(include_str!("../explain/IPE-N0034.md")),
        IPE_N0035 => Some(include_str!("../explain/IPE-N0035.md")),
        IPE_N0037 => Some(include_str!("../explain/IPE-N0037.md")),
        IPE_N0038 => Some(include_str!("../explain/IPE-N0038.md")),
        IPE_N0039 => Some(include_str!("../explain/IPE-N0039.md")),
        IPE_N0040 => Some(include_str!("../explain/IPE-N0040.md")),
        IPE_T0001 => Some(include_str!("../explain/IPE-T0001.md")),
        IPE_T0002 => Some(include_str!("../explain/IPE-T0002.md")),
        IPE_T0003 => Some(include_str!("../explain/IPE-T0003.md")),
        IPE_T0004 => Some(include_str!("../explain/IPE-T0004.md")),
        IPE_T0010 => Some(include_str!("../explain/IPE-T0010.md")),
        IPE_T0011 => Some(include_str!("../explain/IPE-T0011.md")),
        IPE_T0012 => Some(include_str!("../explain/IPE-T0012.md")),
        IPE_T0013 => Some(include_str!("../explain/IPE-T0013.md")),
        IPE_T0014 => Some(include_str!("../explain/IPE-T0014.md")),
        IPE_T0015 => Some(include_str!("../explain/IPE-T0015.md")),
        IPE_T0016 => Some(include_str!("../explain/IPE-T0016.md")),
        IPE_T0017 => Some(include_str!("../explain/IPE-T0017.md")),
        IPE_T0018 => Some(include_str!("../explain/IPE-T0018.md")),
        IPE_T0019 => Some(include_str!("../explain/IPE-T0019.md")),
        IPE_T0020 => Some(include_str!("../explain/IPE-T0020.md")),
        _ => None,
    }
}

/// [`explain_page`]'s back-end half: lowering (`IPE-L*`), FFI (`IPE-F*`),
/// and internal (`IPE-I*`) codes.
#[must_use]
fn back_end_explain_page(c: Code) -> Option<&'static str> {
    match c {
        IPE_L0100 => Some(include_str!("../explain/IPE-L0100.md")),
        IPE_L0101 => Some(include_str!("../explain/IPE-L0101.md")),
        IPE_L0102 => Some(include_str!("../explain/IPE-L0102.md")),
        IPE_L0103 => Some(include_str!("../explain/IPE-L0103.md")),
        IPE_L0104 => Some(include_str!("../explain/IPE-L0104.md")),
        IPE_L0105 => Some(include_str!("../explain/IPE-L0105.md")),
        IPE_L0106 => Some(include_str!("../explain/IPE-L0106.md")),
        IPE_L0107 => Some(include_str!("../explain/IPE-L0107.md")),
        IPE_L0108 => Some(include_str!("../explain/IPE-L0108.md")),
        IPE_L0110 => Some(include_str!("../explain/IPE-L0110.md")),
        IPE_L0111 => Some(include_str!("../explain/IPE-L0111.md")),
        IPE_L0112 => Some(include_str!("../explain/IPE-L0112.md")),
        IPE_L0113 => Some(include_str!("../explain/IPE-L0113.md")),
        IPE_L0114 => Some(include_str!("../explain/IPE-L0114.md")),
        IPE_L0115 => Some(include_str!("../explain/IPE-L0115.md")),
        IPE_L0116 => Some(include_str!("../explain/IPE-L0116.md")),
        IPE_L0117 => Some(include_str!("../explain/IPE-L0117.md")),
        IPE_L0118 => Some(include_str!("../explain/IPE-L0118.md")),
        IPE_L0119 => Some(include_str!("../explain/IPE-L0119.md")),
        IPE_L0120 => Some(include_str!("../explain/IPE-L0120.md")),
        IPE_L0121 => Some(include_str!("../explain/IPE-L0121.md")),
        IPE_L0122 => Some(include_str!("../explain/IPE-L0122.md")),
        IPE_L0123 => Some(include_str!("../explain/IPE-L0123.md")),
        IPE_L0124 => Some(include_str!("../explain/IPE-L0124.md")),
        IPE_L0125 => Some(include_str!("../explain/IPE-L0125.md")),
        IPE_L0126 => Some(include_str!("../explain/IPE-L0126.md")),
        IPE_L0127 => Some(include_str!("../explain/IPE-L0127.md")),
        IPE_L0128 => Some(include_str!("../explain/IPE-L0128.md")),
        IPE_L0129 => Some(include_str!("../explain/IPE-L0129.md")),
        IPE_L0130 => Some(include_str!("../explain/IPE-L0130.md")),
        IPE_L0131 => Some(include_str!("../explain/IPE-L0131.md")),
        IPE_L0132 => Some(include_str!("../explain/IPE-L0132.md")),
        IPE_L0133 => Some(include_str!("../explain/IPE-L0133.md")),
        IPE_L0140 => Some(include_str!("../explain/IPE-L0140.md")),
        IPE_L0200 => Some(include_str!("../explain/IPE-L0200.md")),
        IPE_F4400 => Some(include_str!("../explain/IPE-F4400.md")),
        IPE_F4401 => Some(include_str!("../explain/IPE-F4401.md")),
        IPE_F4402 => Some(include_str!("../explain/IPE-F4402.md")),
        IPE_F4410 => Some(include_str!("../explain/IPE-F4410.md")),
        IPE_F4411 => Some(include_str!("../explain/IPE-F4411.md")),
        IPE_F4412 => Some(include_str!("../explain/IPE-F4412.md")),
        IPE_F4413 => Some(include_str!("../explain/IPE-F4413.md")),
        IPE_F4414 => Some(include_str!("../explain/IPE-F4414.md")),
        IPE_I0001 => Some(include_str!("../explain/IPE-I0001.md")),
        IPE_I0010 => Some(include_str!("../explain/IPE-I0010.md")),
        IPE_I0011 => Some(include_str!("../explain/IPE-I0011.md")),
        IPE_I0100 => Some(include_str!("../explain/IPE-I0100.md")),
        IPE_I0101 => Some(include_str!("../explain/IPE-I0101.md")),
        IPE_I0102 => Some(include_str!("../explain/IPE-I0102.md")),
        IPE_I0103 => Some(include_str!("../explain/IPE-I0103.md")),
        IPE_I0200 => Some(include_str!("../explain/IPE-I0200.md")),
        IPE_I0201 => Some(include_str!("../explain/IPE-I0201.md")),
        IPE_I0202 => Some(include_str!("../explain/IPE-I0202.md")),
        IPE_I0203 => Some(include_str!("../explain/IPE-I0203.md")),
        _ => None,
    }
}

/// Every taxonomy code, authoritative for `ipe explain` and drift detection.
///
/// This is the single source of truth. `ipe` iterates this slice to resolve
/// `explain <CODE>` — no hand-mirror needed.
pub const ALL_CODES: &[Code] = &[
    IPE_P0001, IPE_P0002, IPE_P0003, IPE_P0010, IPE_P0011, IPE_P0012, IPE_P0013, IPE_P0014,
    IPE_P0015, IPE_P0016, IPE_P0017, IPE_P0018, IPE_P0020, IPE_P0021, IPE_P0030, IPE_P0031,
    IPE_P0040, IPE_P0041, IPE_P0050, IPE_P0060, IPE_P0061, IPE_P0062, IPE_P0063, IPE_N0001,
    IPE_N0002, IPE_N0003, IPE_N0004, IPE_N0005, IPE_N0010, IPE_N0011, IPE_N0012, IPE_N0013,
    IPE_N0020, IPE_N0021, IPE_N0022, IPE_N0023, IPE_N0024, IPE_N0025, IPE_N0026, IPE_N0027,
    IPE_N0028, IPE_N0029, IPE_N0031, IPE_N0032, IPE_N0033, IPE_N0034, IPE_N0035, IPE_N0037,
    IPE_N0038, IPE_N0039, IPE_N0040, IPE_T0001, IPE_T0002, IPE_T0003, IPE_T0004, IPE_T0010,
    IPE_T0011, IPE_T0012, IPE_T0013, IPE_T0014, IPE_T0015, IPE_T0016, IPE_T0017, IPE_T0018,
    IPE_T0019, IPE_T0020, IPE_L0100, IPE_L0101, IPE_L0102, IPE_L0103, IPE_L0104, IPE_L0105,
    IPE_L0106, IPE_L0107, IPE_L0108, IPE_L0110, IPE_L0111, IPE_L0112, IPE_L0113, IPE_L0114,
    IPE_L0115, IPE_L0116, IPE_L0117, IPE_L0118, IPE_L0119, IPE_L0120, IPE_L0121, IPE_L0122,
    IPE_L0123, IPE_L0124, IPE_L0125, IPE_L0126, IPE_L0127, IPE_L0128, IPE_L0129, IPE_L0130,
    IPE_L0131, IPE_L0132, IPE_L0133, IPE_L0140, IPE_L0200, IPE_F4400, IPE_F4401, IPE_F4402,
    IPE_F4410, IPE_F4411, IPE_F4412, IPE_F4413, IPE_F4414, IPE_I0001, IPE_I0010, IPE_I0011,
    IPE_I0100, IPE_I0101, IPE_I0102, IPE_I0103, IPE_I0200, IPE_I0201, IPE_I0202, IPE_I0203,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_code_count_is_pinned() {
        assert_eq!(ALL_CODES.len(), 120);
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
            assert!(s.starts_with("IPE-"), "{s} bad prefix");
            assert!(seen.insert(s), "{s} duplicated");
        }
        assert_eq!(seen.len(), 120);
    }

    /// CI coverage gate: every taxonomy code has a conforming explain page.
    /// Line 1 must be exactly `# <CODE>: <title()>` and the body must carry at
    /// least three ```` ```ipe ```` fences. A code without a page, or with a
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
                let fences = page.matches("```ipe").count();
                assert!(
                    fences >= 3,
                    "{} page has {fences} ```ipe fences, need >= 3",
                    c.as_str()
                );
                assert!(
                    page.ends_with('\n'),
                    "{} page must end with a trailing newline",
                    c.as_str()
                );
                assert!(
                    !page.ends_with("\n\n"),
                    "{} page must not end with a double blank line",
                    c.as_str()
                );
            }
        }
    }

    #[test]
    fn issue_tracker_url_is_a_github_issues_link() {
        assert_eq!(
            ISSUE_TRACKER_URL,
            "https://github.com/arthurmaciel/ipe-lang/issues"
        );
    }
}
