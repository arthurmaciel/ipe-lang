//! Systematic negative-test suite: malformed Ipê programs that MUST be
//! rejected by `ipe` (a typed `Err(Diagnostic)`, never exit-0) and must never
//! emit Rust. This guards the CONTRAPOSITIVE of THE SEAL — a malformed program
//! rejected at parse/canon/type/effect/lower time can never reach emission and
//! ship broken Rust.
//!
//! Each test pins the SPECIFIC expected `IPE-####` code, so a wrong-reason
//! rejection is caught, not silently passed. One malformed case per language
//! feature, walking the taxonomy (`ipe_diagnostics::code`) as the coverage map:
//! parse (`IPE-P*`), name resolution / canon (`IPE-N*`), type (`IPE-T*`),
//! lowering / not-yet-supported (`IPE-L*`), plus the target/secret gates.
//!
//! Compile-only: every fixture is ill-formed, so there is nothing to run — no
//! oracle / `IPE_E2E` gate. Each test returns early when the embedded runtime
//! cannot be located (the pipeline needs the compiled stdlib source).

use std::fmt::Write as _;
use std::path::PathBuf;

use ipe::{BuildOptions, CliError};
use ipe_ir::Target;

/// A runtime `false` the optimiser cannot fold, so `assert!(false_marker(), …)`
/// reads as a deliberate unconditional failure rather than a suspicious
/// constant condition — mirrors the compiler crates' own test helper, and keeps
/// this file free of the `clippy::panic` deny (no bare `panic!`).
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

/// Write `source` as a single-file `Main.ipe` under a fresh scratch dir keyed
/// by `name`, returning the entry path (or `None` if scratch setup fails — the
/// caller then skips). The scratch dir lives in the test crate's
/// `CARGO_TARGET_TMPDIR`, never the repo tree.
fn write_entry(name: &str, source: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("negsuite")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let entry = dir.join("Main.ipe");
    std::fs::write(&entry, source).ok()?;
    Some(entry)
}

/// The outcome of running the pipeline over a fixture.
enum Outcome {
    /// Compilation was rejected — the pipeline diagnostic's wire code
    /// (e.g. `"IPE-N0028"`). The wire string is compared so codes not
    /// re-exported from the diagnostics crate root can still be pinned.
    Rejected(&'static str),
    /// Compilation SUCCEEDED (a potential SEAL hole for a malformed input) or
    /// failed for a non-pipeline reason (I/O, usage). Carries a description.
    Accepted(String),
    /// Runtime / scratch unavailable — the caller skips.
    Skip,
}

fn compile(name: &str, source: &str, target: Target) -> Outcome {
    let Some(entry) = write_entry(name, source) else {
        return Outcome::Skip;
    };
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("negsuite-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return Outcome::Skip;
    };
    let options = BuildOptions {
        target,
        ..BuildOptions::default()
    };
    match ipe::build_with_options(&entry, &out, &runtime, options) {
        Ok(()) => Outcome::Accepted("compiled successfully (exit 0)".to_owned()),
        Err(CliError::Pipeline { diag, .. }) => Outcome::Rejected(diag.code().as_str()),
        Err(other) => Outcome::Accepted(format!("non-pipeline error: {other:?}")),
    }
}

/// Run the full `ipe` pipeline over a multi-file project (sibling discovery):
/// `files` are written under a fresh `src/` keyed by `name`, and `Main.ipe` is
/// the entry. Needed for cross-module gates (e.g. duplicate import qualifier)
/// that the single-file path cannot observe.
fn compile_project(name: &str, files: &[(&str, &str)]) -> Outcome {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("negsuite-proj")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    if std::fs::create_dir_all(&src).is_err() {
        return Outcome::Skip;
    }
    for (fname, contents) in files {
        if std::fs::write(src.join(fname), contents).is_err() {
            return Outcome::Skip;
        }
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("negsuite-proj-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return Outcome::Skip;
    };
    let entry = src.join("Main.ipe");
    match ipe::build_with_sibling_discovery(&entry, &out, &runtime) {
        Ok(()) => Outcome::Accepted("compiled successfully (exit 0)".to_owned()),
        Err(CliError::Pipeline { diag, .. }) => Outcome::Rejected(diag.code().as_str()),
        Err(other) => Outcome::Accepted(format!("non-pipeline error: {other:?}")),
    }
}

/// Fail the current test: the malformed program was ACCEPTED when `expected`
/// was the intended rejection — a potential SEAL hole. Uses [`false_marker`] so
/// no bare `panic!` is needed (clippy deny-set).
#[track_caller]
fn fail_accepted(name: &str, expected: &str, how: &str) {
    assert!(
        false_marker(),
        "{name}: expected rejection with {expected}, but ipe ACCEPTED the malformed program \
         ({how}) — a potential SEAL hole"
    );
}

/// Assert that `source`, compiled for `target`, is rejected by `ipe` with
/// exactly the wire code `expected`. A wrong code, an accept (a SEAL hole), or
/// a non-pipeline failure fails the test naming what happened.
#[track_caller]
fn assert_rejected_on(name: &str, source: &str, expected: &str, target: Target) {
    match compile(name, source, target) {
        Outcome::Skip => {}
        Outcome::Rejected(got) => assert_eq!(
            got, expected,
            "{name}: expected {expected}, got {got} — a rejection for the WRONG reason"
        ),
        Outcome::Accepted(how) => fail_accepted(name, expected, &how),
    }
}

/// [`assert_rejected_on`] on the native target.
#[track_caller]
fn assert_rejected(name: &str, source: &str, expected: &str) {
    assert_rejected_on(name, source, expected, Target::Native);
}

/// [`assert_rejected_on`] under `--target wasm` (the wasm capability gate is
/// target-keyed).
#[track_caller]
fn assert_rejected_wasm(name: &str, source: &str, expected: &str) {
    assert_rejected_on(name, source, expected, Target::WasmClient);
}

/// Assert that `source` compiles cleanly (the CONTRAPOSITIVE of a rejection):
/// a well-formed program the pipeline must accept. Used to prove that a
/// tightened surface still admits its legitimate replacement.
#[track_caller]
fn assert_compiles(name: &str, source: &str) {
    match compile(name, source, Target::Native) {
        Outcome::Skip => {}
        Outcome::Accepted(how) if how.starts_with("compiled successfully") => {}
        Outcome::Accepted(how) => assert!(
            false_marker(),
            "{name}: expected a clean compile, got a non-pipeline failure ({how})"
        ),
        Outcome::Rejected(got) => assert!(
            false_marker(),
            "{name}: expected a clean compile, but ipe REJECTED it with {got}"
        ),
    }
}

// A minimal well-formed prelude preamble reused across fixtures.
const HEAD: &str = "module Main exposing (main)\n";

// ===========================================================================
// Parse — IPE-P####
// ===========================================================================

/// A `case` with no arms cannot parse — malformed case expression.
#[test]
fn parse_malformed_case_no_arms() {
    let src = format!("{HEAD}main =\n    case 1 of\n");
    assert_rejected("parse_case_no_arms", &src, "IPE-P0060");
}

/// A `let` with no `in` cannot parse — malformed let expression.
#[test]
fn parse_malformed_let_no_in() {
    let src = format!("{HEAD}main =\n    let x = 1\n    x\n");
    assert_rejected("parse_let_no_in", &src, "IPE-P0061");
}

/// An `if` missing its `then`/`else` cannot parse — malformed if expression.
#[test]
fn parse_malformed_if() {
    let src = format!("{HEAD}main =\n    if True\n");
    assert_rejected("parse_if_incomplete", &src, "IPE-P0062");
}

/// An unterminated string literal is a lex error.
#[test]
fn parse_unterminated_string() {
    let src = format!("{HEAD}main =\n    \"unterminated\n");
    assert_rejected("parse_unterminated_string", &src, "IPE-P0014");
}

/// A definition missing its `=` cannot parse.
#[test]
fn parse_missing_equals() {
    let src = format!("{HEAD}main\n    1\n");
    assert_rejected("parse_missing_equals", &src, "IPE-P0030");
}

/// A malformed module header (garbage where `exposing` belongs).
#[test]
fn parse_malformed_module_header() {
    let src = "module Main whoops (main)\nmain = 1\n";
    assert_rejected("parse_module_header", src, "IPE-P0020");
}

/// An unclosed delimiter (open paren, never closed).
#[test]
fn parse_unclosed_delimiter() {
    let src = format!("{HEAD}main =\n    (1 + 2\n");
    assert_rejected("parse_unclosed_delim", &src, "IPE-P0050");
}

/// A malformed type declaration (`type` with no `=` body).
#[test]
fn parse_malformed_type_decl() {
    let src = format!("{HEAD}type Foo\nmain = 1\n");
    assert_rejected("parse_type_decl", &src, "IPE-P0031");
}

/// An unknown character in source (a raw control/garbage byte the lexer cannot
/// classify) — lexical rejection.
#[test]
fn parse_unknown_character() {
    let src = format!("{HEAD}main =\n    1 \u{0007} 2\n");
    assert_rejected("parse_unknown_char", &src, "IPE-P0010");
}

/// A malformed character literal (empty `''`).
#[test]
fn parse_malformed_char_literal() {
    let src = format!("{HEAD}main =\n    ''\n");
    assert_rejected("parse_malformed_char", &src, "IPE-P0015");
}

/// A stray `.` where a name/expression belongs.
#[test]
fn parse_stray_dot() {
    let src = format!("{HEAD}main =\n    . 1\n");
    assert_rejected("parse_stray_dot", &src, "IPE-P0011");
}

/// A number joined directly to a name (`1abc`) is a lex error, not a valid
/// identifier or literal.
#[test]
fn parse_number_joined_to_name() {
    let src = format!("{HEAD}main =\n    1abc\n");
    assert_rejected("parse_num_name", &src, "IPE-P0012");
}

/// An integer literal beyond the 64-bit range.
#[test]
fn parse_integer_out_of_range() {
    let src = format!("{HEAD}main =\n    99999999999999999999999999999999\n");
    assert_rejected("parse_int_range", &src, "IPE-P0013");
}

/// An unterminated block comment (`{-` never closed).
#[test]
fn parse_unterminated_block_comment() {
    let src = format!("{HEAD}main =\n    1\n{{- never closed\n");
    assert_rejected("parse_block_comment", &src, "IPE-P0017");
}

/// A malformed exposing list (a trailing comma with no name).
#[test]
fn parse_malformed_exposing_list() {
    let src = "module Main exposing (main,\nmain = 1\n";
    assert_rejected("parse_exposing", src, "IPE-P0021");
}

/// Source that ends before an expression is complete — unexpected EOF.
#[test]
fn parse_unexpected_eof() {
    let src = format!("{HEAD}main =\n    1 +");
    assert_rejected("parse_unexpected_eof", &src, "IPE-P0002");
}

/// In a type, only a type constructor may take arguments — a lowercase type var
/// cannot be applied to args.
#[test]
fn parse_only_ctor_takes_args() {
    let src = format!("{HEAD}main : a Int\nmain =\n    1\n");
    assert_rejected("parse_only_ctor_args", &src, "IPE-P0040");
}

/// A type annotation whose right-hand side is not a type at all.
#[test]
fn parse_expected_a_type() {
    let src = format!("{HEAD}main : 123\nmain =\n    1\n");
    assert_rejected("parse_expected_type", &src, "IPE-P0041");
}

// ===========================================================================
// Name resolution / canonicalisation — IPE-N####
// ===========================================================================

/// A bare name that is not bound anywhere in scope.
#[test]
fn canon_unbound_value() {
    let src = format!("{HEAD}main =\n    unboundName\n");
    assert_rejected("canon_unbound_value", &src, "IPE-N0001");
}

/// A type annotation referencing a type that does not exist.
#[test]
fn canon_unknown_type() {
    let src = format!("{HEAD}main : NoSuchType\nmain =\n    1\n");
    assert_rejected("canon_unknown_type", &src, "IPE-N0002");
}

/// A pattern naming a constructor that is not defined.
#[test]
fn canon_unknown_constructor() {
    let src = format!("{HEAD}type Msg = A\nmain =\n    case A of\n        Nonexistent -> 1\n");
    assert_rejected("canon_unknown_ctor", &src, "IPE-N0003");
}

/// A reference through a qualifier bound to a module that does not exist. The
/// unknown-module diagnostic fires at the USE site (an unused import of a
/// nonexistent module is not itself an error, matching Elm), so the qualifier
/// must be referenced to trip the gate.
#[test]
fn canon_unknown_module() {
    let src = "module Main exposing (main)\n\
               import Ipe.NoSuchModule as X\n\
               main = X.foo\n";
    assert_rejected("canon_unknown_module", src, "IPE-N0004");
}

/// Importing a member a real module does not expose.
#[test]
fn canon_member_not_exposed() {
    let src = "module Main exposing (main)\n\
               import Ipe.String exposing (thisFunctionDoesNotExist)\n\
               main = 1\n";
    assert_rejected("canon_member_not_exposed", src, "IPE-N0022");
}

/// SECURITY: the un-escaped raw-String→HTML surface `Html.raw` is REMOVED — its
/// only spelling is now the explicitly-marked `Html.unsafeRaw`, so a raw
/// injection can never be written under a name that looks safe. The old name no
/// longer resolves.
#[test]
fn security_html_raw_unmarked_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Html as Html\n\
               main = Html.raw \"<b>x</b>\"\n";
    assert_rejected("security_html_raw_unmarked", src, "IPE-N0005");
}

/// SECURITY (contrapositive): the marked replacement `Html.unsafeRaw` still
/// compiles — the raw capability is preserved, only renamed to name the risk.
#[test]
fn security_html_unsafe_raw_compiles() {
    let src = "module Main exposing (main)\n\
               import Ipe.Html as Html\n\
               main = Html.unsafeRaw \"<b>x</b>\"\n";
    assert_compiles("security_html_unsafe_raw", src);
}

/// SECURITY (untouched bridge): `Ui.html` — the typed `Html msg -> Element msg`
/// bridge — is NOT a raw-string hole and stays fully working. It carries a
/// typed tree built from the escaped `Html.text` path, so tightening the raw
/// surface leaves the typed bridge intact.
#[test]
fn security_ui_html_typed_bridge_compiles() {
    let src = "module Main exposing (main)\n\
               import Ipe.Ui as Ui\n\
               import Ipe.Html as Html\n\
               main = Ui.html (Html.text \"hello\")\n";
    assert_compiles("security_ui_html_bridge", src);
}

/// The same top-level value defined twice.
#[test]
fn canon_duplicate_value() {
    let src = format!("{HEAD}dup = 1\ndup = 2\nmain = dup\n");
    assert_rejected("canon_dup_value", &src, "IPE-N0010");
}

/// The same constructor name defined twice across types.
#[test]
fn canon_duplicate_constructor() {
    let src = format!("{HEAD}type A = Dup\ntype B = Dup\nmain = 1\n");
    assert_rejected("canon_dup_ctor", &src, "IPE-N0011");
}

/// The same type name declared twice.
#[test]
fn canon_duplicate_type() {
    let src = format!("{HEAD}type Foo = A\ntype Foo = B\nmain = 1\n");
    assert_rejected("canon_dup_type", &src, "IPE-N0012");
}

/// A type alias applied with the wrong number of arguments.
#[test]
fn canon_alias_wrong_arity() {
    let src =
        format!("{HEAD}type alias Pair a b = ( a, b )\nmain : Pair Int\nmain =\n    ( 1, 2 )\n");
    assert_rejected("canon_alias_arity", &src, "IPE-N0013");
}

/// A diamond alias chain that doubles the expansion work at every level
/// — 31 levels of `(Prev, Prev)` exceed the node budget (2^31 > 100 000) and
/// must be rejected at name resolution with IPE-N0032, not a stack overflow,
/// hang, or OOM. The program is otherwise well-typed; only the alias shape is
/// pathological.
#[test]
fn canon_type_alias_expansion_node_budget() {
    // Build 31 alias levels: A0 = Int, A1 = (A0, A0), ..., A30 = (A29, A29).
    // Level n produces 2^n expansion nodes, so A30 alone would need > 1 billion.
    let mut src = format!("{HEAD}type alias A0 = Int\n");
    for i in 1..=30_u32 {
        // Writing into a String is infallible.
        let _ = writeln!(src, "type alias A{i} = ( A{}, A{} )", i - 1, i - 1);
    }
    src.push_str("main : A30\nmain =\n    (1, 1)\n");
    assert_rejected("canon_alias_node_budget", &src, "IPE-N0032");
}

/// A straight alias chain of depth 300 — deeper than the 256 recursion-depth
/// cap — must be rejected with IPE-N0032 (depth limit), not a native-stack
/// overflow.
#[test]
fn canon_type_alias_expansion_depth_limit() {
    let mut src = format!("{HEAD}type alias A0 = Int\n");
    for i in 1..=300_u32 {
        // Writing into a String is infallible.
        let _ = writeln!(src, "type alias A{i} = A{}", i - 1);
    }
    src.push_str("main : A300\nmain =\n    1\n");
    assert_rejected("canon_alias_depth_limit", &src, "IPE-N0032");
}

/// A user type that reuses a built-in type name (`Int`).
#[test]
fn canon_reserved_builtin_type_name() {
    let src = format!("{HEAD}type Int = MyInt\nmain = 1\n");
    assert_rejected("canon_reserved_builtin", &src, "IPE-N0026");
}

/// The JS-widget boundary type name `CustomElement` is reserved: a user
/// `type CustomElement …` declaration must be rejected exactly like any other
/// security-tier reserved builtin, so the typed seam cannot be shadowed by a
/// user-forged untyped widget type.
#[test]
fn canon_custom_element_definition_reserved() {
    let src = format!("{HEAD}type CustomElement d u = Ce\nmain = 1\n");
    assert_rejected("canon_custom_element_def", &src, "IPE-N0026");
}

/// Using the reserved `CustomElement down up` boundary type in an annotation is
/// rejected FAIL-CLOSED (IPE-N0037) until its runtime transport ships: the seam
/// is not yet emittable, and there is deliberately no untyped fallback. This is
/// the contrapositive of THE SEAL pointed at the JS boundary — a `CustomElement`
/// annotation can never reach emission and pass an untyped value across it.
#[test]
fn canon_custom_element_use_fail_closed() {
    let src = format!(
        "{HEAD}editor : CustomElement Int String\n\
         editor = editor\n\
         main = 1\n"
    );
    assert_rejected("canon_custom_element_use", &src, "IPE-N0037");
}

/// A QUALIFIED spelling of the reserved boundary type (`D.CustomElement …`
/// through an imported module) must fail closed with the same IPE-N0037 at
/// check time — never slip past the boundary gate on its non-empty home into a
/// build-time ICE. The name is reserved against every origin, so no qualifier
/// can name a legitimate `CustomElement`.
#[test]
fn canon_custom_element_qualified_use_fail_closed() {
    let src = format!(
        "{HEAD}import Ipe.Dict as D\n\
         editor : D.CustomElement Int String\n\
         editor = editor\n\
         main = 1\n"
    );
    assert_rejected("canon_custom_element_qualified", &src, "IPE-N0037");
}

/// Two imports registering the same qualifier for DIFFERENT sibling modules.
/// The clash is only observable across a multi-file project (sibling
/// discovery), so this uses the project harness rather than the single-file
/// path — the qualifier is referenced so it reaches a use site.
#[test]
fn canon_duplicate_qualifier() {
    match compile_project(
        "canon_dup_qualifier",
        &[
            ("A.ipe", "module A exposing (label)\nlabel = \"from A\"\n"),
            ("B.ipe", "module B exposing (label)\nlabel = \"from B\"\n"),
            (
                "Main.ipe",
                "module Main exposing (main)\n\
                 import A as Utils\n\
                 import B as Utils\n\
                 main = Utils.label\n",
            ),
        ],
    ) {
        Outcome::Skip => {}
        Outcome::Rejected(got) => assert_eq!(
            got, "IPE-N0027",
            "canon_dup_qualifier: expected IPE-N0027, got {got} — WRONG reason"
        ),
        Outcome::Accepted(how) => fail_accepted("canon_dup_qualifier", "IPE-N0027", &how),
    }
}

/// An `Ffi.kernel "Name"` alias naming a kernel that is not registered.
#[test]
fn canon_unknown_kernel_alias() {
    let src = format!(
        "{HEAD}import Ipe.Ffi as Ffi\n\
         bogus : Int -> Int\n\
         bogus = Ffi.kernel \"No_such_kernel_at_all\"\n\
         main = bogus 1\n"
    );
    assert_rejected("canon_unknown_kernel_alias", &src, "IPE-N0028");
}

// ===========================================================================
// Type — IPE-T####
// ===========================================================================

/// Adding an `Int` and a `String` — a plain HM unification failure.
#[test]
fn type_mismatch_int_plus_string() {
    let src = format!("{HEAD}main =\n    1 + \"two\"\n");
    assert_rejected("type_mismatch", &src, "IPE-T0001");
}

/// A declared signature contradicted by the body's type.
#[test]
fn type_signature_body_mismatch() {
    let src = format!("{HEAD}main : Int\nmain =\n    \"not an int\"\n");
    assert_rejected("type_sig_mismatch", &src, "IPE-T0001");
}

/// A `case` that does not cover every constructor is non-exhaustive.
#[test]
fn type_non_exhaustive_case() {
    let src = format!(
        "{HEAD}type Color = Red | Green | Blue\n\
         describe : Color -> Int\n\
         describe c =\n    case c of\n        Red -> 1\n        Green -> 2\n"
    );
    assert_rejected("type_non_exhaustive", &src, "IPE-T0010");
}

/// A `case` over a Prelude built-in ADT (`ErrorKind` NESTED under `Maybe`) that
/// omits variants must be caught as non-exhaustive at `ipe` time (IPE-T0010),
/// not slip to cargo as E0004. Guards CO-TYPES-001 — `types::exhaust` must
/// analyse EVERY built-in union (via the shared `ipe_canon::builtins` table),
/// not just Maybe/Result. Before the fix the nested `ErrorKind` arm set was
/// skipped as an "unknown constructor" and the missing 9 variants shipped to
/// rustc.
#[test]
fn exhaust_builtin_adt_nested_nonexhaustive() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         describe : Maybe ErrorKind -> String\n\
         describe m =\n    case m of\n        \
         Just Io      -> \"io\"\n        \
         Just Network -> \"net\"\n        \
         Nothing      -> \"none\"\n\n\
         main = Io.println (describe Nothing)\n"
    );
    assert_rejected("exhaust_builtin_adt_nested", &src, "IPE-T0010");
}

/// A TOP-level `case` over a Prelude built-in ADT (`ErrorKind`) that omits
/// variants must ALSO be IPE-T0010 — not a `Diagnostic::CompilerBug` ("top
/// constructors cover 2 of 11"), the shape the pre-fix lower backstop produced.
/// Guards the second CO-TYPES-001 variant.
#[test]
fn exhaust_builtin_adt_toplevel_nonexhaustive() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         classify : ErrorKind -> String\n\
         classify k =\n    case k of\n        \
         Io      -> \"io\"\n        Network -> \"net\"\n\n\
         main = Io.println (classify Io)\n"
    );
    assert_rejected("exhaust_builtin_toplevel", &src, "IPE-T0010");
}

// NOTE: IPE-T0011 (redundant case branch) is intentionally `Severity::Warning`
// (see `types::exhaust` — "collect it but do not abort"), so a redundant arm
// does NOT reject compilation. It is therefore out of scope for a
// rejection-suite; it belongs to a warnings test, not a negative gate.

/// A record accessed on a field it does not have.
#[test]
fn type_record_no_such_field() {
    let src = format!("{HEAD}main =\n    let r = {{ x = 1 }} in\n    r.y\n");
    assert_rejected("type_no_such_field", &src, "IPE-T0012");
}

/// A constructor pattern binding the wrong number of payload fields.
#[test]
fn type_ctor_pattern_wrong_arity() {
    let src = format!(
        "{HEAD}type Box = Box Int\n\
         unwrap : Box -> Int\n\
         unwrap b =\n    case b of\n        Box x y -> x\n"
    );
    assert_rejected("type_ctor_pat_arity", &src, "IPE-T0013");
}

/// A parameter pattern that is refutable (a constructor pattern in a function
/// head, where an irrefutable binder is required). `f (Just x) = x` cannot bind
/// every input, so the parameter position rejects it.
#[test]
fn type_refutable_param_pattern() {
    let src = format!(
        "{HEAD}f : Maybe Int -> Int\n\
         f (Just x) =\n    x\n\
         main = f (Just 1)\n"
    );
    assert_rejected("type_refutable_param", &src, "IPE-T0015");
}

/// More parameters than the signature's arrow chain describes.
#[test]
fn type_too_many_params() {
    let src = format!(
        "{HEAD}f : Int -> Int\n\
         f a b =\n    a\n\
         main = f 1\n"
    );
    assert_rejected("type_too_many_params", &src, "IPE-T0004");
}

/// A record update on a nominal built-in type. The field IS readable
/// (`p.message`), but a nominal builtin has no user-writable update form — the
/// dedicated IPE-T0017, distinct from the "no such field" IPE-T0012.
#[test]
fn type_record_update_on_builtin() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         f : PanicInfo -> PanicInfo\n\
         f p =\n    {{ p | message = \"x\" }}\n\
         main =\n    Io.println \"never\"\n"
    );
    assert_rejected("type_update_builtin", &src, "IPE-T0017");
}

// ===========================================================================
// Effect boundary + secret + event-handler gates
// ===========================================================================

/// An `Ipe.Html.Events` handler whose payload shape is wrong (a `Bool` handler
/// on a `String`-payload event) is a type mismatch, never an exit-0 deferral.
#[test]
fn effect_illtyped_event_handler() {
    let src = format!(
        "{HEAD}import Ipe.Html as Html\n\
         import Ipe.Html.Events as Event\n\
         type Msg = SetChecked Bool\n\
         view : Html.Html Msg\n\
         view =\n    Html.input [ Event.onInput (\\b -> SetChecked b) ] []\n\
         main = 1\n"
    );
    assert_rejected("effect_illtyped_event", &src, "IPE-T0001");
}

/// A `Secret` concatenated with `++` into a plain `String` — `Secret` does not
/// satisfy the append obligation, so the accidental-leak path is a compile-time
/// type mismatch, never a silent stringification of a secret.
#[test]
fn effect_secret_concat_rejected() {
    let src = format!(
        "{HEAD}import Ipe.Secret as Secret\n\
         main =\n    \"using key \" ++ Secret.fromString \"sk_live_abc123\"\n"
    );
    assert_rejected("effect_secret_concat", &src, "IPE-T0001");
}

/// A `Secret` in a `Ipe.Web` app Model must be rejected — `Secret` is non-serde
/// by design, so it can never round-trip through the session store. IPE-L0120
/// (Model not admissible) at compile time, never a runtime session-store leak.
#[test]
fn effect_secret_in_live_model() {
    let src = "module Main exposing (main)\n\
         import Ipe.Secret as Secret\n\
         import Ipe.Tea.Web exposing (app)\n\
         import Ipe.Cmd as Cmd\n\
         import Ipe.Sub as Sub\n\
         import Ipe.Ui as Ui\n\
         \n\
         type Page = HomePage\n\
         type Msg = Noop\n\
         type alias Model = { count : Int, apiKey : Secret }\n\
         \n\
         init : a -> ( Model, Cmd Msg )\n\
         init _req = ( { count = 0, apiKey = Secret.fromString \"sk_live_x\" }, Cmd.none )\n\
         update : Msg -> Model -> ( Model, Cmd Msg )\n\
         update _msg model = ( model, Cmd.none )\n\
         subscriptions : Model -> Sub Msg\n\
         subscriptions _model = Sub.none\n\
         view : Model -> Element Msg\n\
         view _model = Ui.text \"hi\"\n\
         \n\
         main =\n\
         \x20   app\n\
         \x20       { init = init\n\
         \x20       , update = update\n\
         \x20       , view = view\n\
         \x20       , subscriptions = subscriptions\n\
         \x20       , routes = []\n\
         \x20       , notFound = HomePage\n\
         \x20       }\n";
    assert_rejected("effect_secret_live_model", src, "IPE-L0120");
}

// ===========================================================================
// Target capability gate (--target wasm) — IPE-N0029 / IPE-L0129
// ===========================================================================

/// A server-only kernel (`File.readFile`) named under `--target wasm` has no
/// browser denotation — IPE-N0029 at compile time, never a cargo failure.
#[test]
fn wasm_server_only_kernel_rejected() {
    let src = format!(
        "{HEAD}import Ipe.File as File\n\
         import Ipe.Path as Path\n\
         import Ipe.Task as Task\n\
         main =\n\
         \x20   case Path.fromString \"/etc/passwd\" of\n\
         \x20       Ok p -> File.readFile p\n\
         \x20       Err e -> Task.fail e\n"
    );
    assert_rejected_wasm("wasm_server_only_kernel", &src, "IPE-N0029");
}

/// The same server-only program builds cleanly for the NATIVE target — the gate
/// is target-keyed, not a global ban. (Positive control for the wasm gate.)
#[test]
fn wasm_server_only_kernel_native_ok() {
    let src = format!(
        "{HEAD}import Ipe.File as File\n\
         import Ipe.Path as Path\n\
         import Ipe.Task as Task\n\
         main =\n\
         \x20   case Path.fromString \"/etc/passwd\" of\n\
         \x20       Ok p -> File.readFile p\n\
         \x20       Err e -> Task.fail e\n"
    );
    if let Outcome::Rejected(got) = compile("wasm_native_control", &src, Target::Native) {
        assert!(
            false_marker(),
            "the native build of a server-only program must stay green, got rejection {got}"
        );
    }
}

// ===========================================================================
// Lowering / not-yet-supported — IPE-L####
// ===========================================================================

/// A `Float` used as a `Dict` key has no valid backend rendering (`f64` is not
/// `Ord`/`Hash` on the Rust backend) — IPE-L0117.
#[test]
fn lower_float_dict_key() {
    let src = format!(
        "{HEAD}import Ipe.Dict as Dict\n\
         main =\n    Dict.insert 1.5 \"x\" Dict.empty\n"
    );
    assert_rejected("lower_float_dict_key", &src, "IPE-L0117");
}

/// A `Float` used as a `Set` element — same backend restriction, IPE-L0117.
#[test]
fn lower_float_set_element() {
    let src = format!(
        "{HEAD}import Ipe.Set as Set\n\
         main =\n    Set.insert 1.5 Set.empty\n"
    );
    assert_rejected("lower_float_set_elem", &src, "IPE-L0117");
}

/// An app-entry cfg must be an inline record literal, never a let-bound
/// variable — IPE-L0119.
#[test]
fn lower_let_bound_app_cfg() {
    let src = "module Main exposing (main)\n\
         import Ipe.Tea.Web exposing (app)\n\
         import Ipe.Cmd as Cmd\n\
         import Ipe.Sub as Sub\n\
         import Ipe.Ui as Ui\n\
         \n\
         type Page = HomePage\n\
         type Msg = Noop\n\
         type alias Model = { count : Int }\n\
         \n\
         init : a -> ( Model, Cmd Msg )\n\
         init _req = ( { count = 0 }, Cmd.none )\n\
         update : Msg -> Model -> ( Model, Cmd Msg )\n\
         update _msg model = ( model, Cmd.none )\n\
         subscriptions : Model -> Sub Msg\n\
         subscriptions _model = Sub.none\n\
         view : Model -> Element Msg\n\
         view _model = Ui.text \"hi\"\n\
         \n\
         main =\n\
         \x20   let cfg =\n\
         \x20           { init = init\n\
         \x20           , update = update\n\
         \x20           , view = view\n\
         \x20           , subscriptions = subscriptions\n\
         \x20           , routes = []\n\
         \x20           , notFound = HomePage\n\
         \x20           }\n\
         \x20   in\n\
         \x20   app cfg\n";
    assert_rejected("lower_let_bound_cfg", src, "IPE-L0119");
}

// ===========================================================================
// FFI trust boundary (T1) — the decode/emit gate rejects injection-bearing
// inspector data, and the warm-cache load re-derives `_bindings.rs` from the
// validated inspection document so a planted `_bindings.rs` is inert. These
// guard the CONTRAPOSITIVE of THE SEAL at the FFI seam: an injection-bearing
// type/path/selector string can never reach the unsandboxed `<slug>_bindings.rs`
// that compiles into the user crate, and a representable-but-illegal type (an
// unbalanced `<`) rejects at decode rather than exit-0-then-cargo-fail.
// ===========================================================================

use ipe_ffi::diag::{Diagnostic, WireDefect};
use ipe_ffi::driver::{FfiCache, load_catalog};
use ipe_ffi::pkginfo::PkgInfo;

/// A `PkgInfo` inspection document with one function carrying the given
/// `rustType` on its sole parameter.
fn pkg_json_with_param_rust_type(rust_type: &str) -> String {
    format!(
        "{{\"pkg\":\"x\",\"name\":\"x\",\"version\":\"1.0.0\",\
          \"functions\":[{{\"name\":\"f\",\
          \"params\":[{{\"name\":\"a\",\"type\":\"u64\",\"rustType\":{rt}}}],\
          \"results\":[{{\"name\":\"\",\"type\":\"u64\"}}],\
          \"effect\":\"pure\"}}],\"errors\":[]}}",
        rt = serde_json::to_string(rust_type).unwrap_or_else(|_| "\"\"".to_owned())
    )
}

/// An injection-bearing `rustType` drops its binding at decode with the typed
/// `InvalidType` defect — the raw string never reaches emission.
#[test]
fn ffi_injection_bearing_rust_type_is_refused_at_decode() {
    let doc = pkg_json_with_param_rust_type("u64; std::process::Command::new(\"sh\")");
    let pkg = PkgInfo::decode_json(&doc).expect("package header survives");
    assert!(
        pkg.fns().is_empty(),
        "the injection-bearing binding must be dropped"
    );
    assert!(
        matches!(
            pkg.dropped().first(),
            Some(Diagnostic::WireMalformed {
                defect: WireDefect::InvalidType { .. },
                ..
            })
        ),
        "expected InvalidType, got {:?}",
        pkg.dropped().first()
    );
}

/// SEAL corollary: an unbalanced `<` in a `rustType` is a representable but
/// illegal Rust type. It rejects at DECODE (drops the binding), never
/// producing an `ipe`-exit-0 emission that a later `cargo build` would reject.
#[test]
fn ffi_unbalanced_angle_rust_type_rejects_at_decode_not_cargo() {
    let doc = pkg_json_with_param_rust_type("Vec<u64");
    let pkg = PkgInfo::decode_json(&doc).expect("package header survives");
    assert!(
        pkg.fns().is_empty(),
        "the unbalanced type drops its binding"
    );
    assert!(
        matches!(
            pkg.dropped().first(),
            Some(Diagnostic::WireMalformed {
                defect: WireDefect::InvalidType { .. },
                ..
            })
        ),
        "expected InvalidType at decode, got {:?}",
        pkg.dropped().first()
    );
}

/// A hand-planted `_bindings.rs` carrying an injected wrapper body is INERT:
/// `load_catalog` re-derives the wrappers from the validated `pkg.json`, so the
/// planted text never reaches the emitted `src/ffi.rs`.
#[test]
fn ffi_planted_bindings_file_is_ignored_on_load() {
    let doc = "{\"pkg\":\"semver\",\"name\":\"semver\",\"version\":\"1.0.0\",\
        \"functions\":[{\"name\":\"parse\",\
        \"params\":[{\"name\":\"text\",\"type\":\"String\",\"ipeType\":\"String\",\"rustType\":\"&str\"}],\
        \"results\":[{\"name\":\"\",\"type\":\"Result Error Version\",\"rustType\":\"Result<Version, Error>\"}],\
        \"effect\":\"fallible\"}],\"errors\":[]}";
    let Some(dir) = write_entry("ffi_planted_cache", "") else {
        return;
    };
    let root = dir
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or(dir);
    let cache = FfiCache::at_project_root(&root);
    let Ok((_pkg, paths)) = ipe_ffi::driver::install_from_inspection(&cache, doc) else {
        return;
    };
    // Plant an injected item into a reached wrapper region of _bindings.rs.
    if let Ok(text) = std::fs::read_to_string(&paths.bindings) {
        let planted = text.replace(
            "pub fn semver_parse",
            "pub fn pwned(){ std::process::Command::new(\"sh\"); } pub fn semver_parse",
        );
        let _ = std::fs::write(&paths.bindings, planted);
    }
    let catalog = load_catalog(cache.root()).expect("loads");
    for c in &catalog {
        assert!(
            !c.bindings_source.contains("pwned"),
            "a planted _bindings.rs must not survive re-derivation:\n{}",
            c.bindings_source
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
