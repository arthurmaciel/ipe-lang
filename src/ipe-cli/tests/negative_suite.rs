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
/// only spelling is now the explicitly-marked `unsafeRaw` in the dedicated
/// `Ipe.Html.Unsafe` escape-hatch submodule, so a raw injection can never be
/// written under a name that looks safe. The old name no longer resolves.
#[test]
fn security_html_raw_unmarked_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Html as Html\n\
               main = Html.raw \"<b>x</b>\"\n";
    assert_rejected("security_html_raw_unmarked", src, "IPE-N0005");
}

/// SECURITY: `unsafeRaw` no longer lives on the plain `Ipe.Html` surface — it
/// relocated to `Ipe.Html.Unsafe`. A program that imports only `Ipe.Html` and
/// reaches for `Html.unsafeRaw` must be rejected, so the escape hatch cannot be
/// used without the disclosing `Ipe.Html.Unsafe` import.
#[test]
fn security_html_unsafe_raw_off_plain_html_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Html as Html\n\
               main = Html.unsafeRaw \"<b>x</b>\"\n";
    assert_rejected("security_html_unsafe_raw_off_plain", src, "IPE-N0005");
}

/// SECURITY (contrapositive): the marked replacement, now homed in
/// `Ipe.Html.Unsafe`, still compiles — the raw capability is preserved, only
/// relocated to the disclosing submodule that names the risk.
#[test]
fn security_html_unsafe_raw_compiles() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Html.Unsafe exposing (unsafeRaw)\n\
               rawEl = unsafeRaw \"<b>x</b>\"\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println \"ok\"\n";
    assert_compiles("security_html_unsafe_raw", src);
}

/// SECURITY: the inline-`<script>` hatch `unsafeScript` is homed ONLY in
/// `Ipe.Html.Unsafe`, never on the plain `Ipe.Html` surface. A program that
/// imports only `Ipe.Html` and reaches for `Html.unsafeScript` must be rejected,
/// so the trusted-code injection surface cannot be used without the disclosing
/// `Ipe.Html.Unsafe` import.
#[test]
fn security_html_unsafe_script_off_plain_html_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Html as Html\n\
               main = Html.unsafeScript \"x\"\n";
    assert_rejected("security_html_unsafe_script_off_plain", src, "IPE-N0005");
}

/// SECURITY (contrapositive): `unsafeScript`, homed in `Ipe.Html.Unsafe`, still
/// compiles — the inline-`<script>` capability is preserved, reached only
/// through the disclosing submodule that names the trusted-code risk.
#[test]
fn security_html_unsafe_script_compiles() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Html.Unsafe exposing (unsafeScript)\n\
               scriptEl = unsafeScript \"console.log(1)\"\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println \"ok\"\n";
    assert_compiles("security_html_unsafe_script", src);
}

/// SECURITY (untouched bridge): `Ui.html` — the typed `Html msg -> Element msg`
/// bridge — is NOT a raw-string hole and stays fully working. It carries a
/// typed tree built from the escaped `Html.text` path, so tightening the raw
/// surface leaves the typed bridge intact.
#[test]
fn security_ui_html_typed_bridge_compiles() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Ui as Ui\n\
               import Ipe.Html as Html\n\
               bridgeEl = Ui.html (Html.text \"hello\")\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println \"ok\"\n";
    assert_compiles("security_ui_html_bridge", src);
}

/// SECURITY: the raw-SQL hatch `unsafeExecRaw` no longer lives on the plain
/// `Ipe.Db` surface — it relocated to `Ipe.Db.Unsafe`. A program that imports
/// only `Ipe.Db` and reaches for `Db.unsafeExecRaw` must be rejected, so the
/// verbatim-SQL injection surface cannot be used without the disclosing
/// `Ipe.Db.Unsafe` import.
#[test]
fn security_db_unsafe_exec_raw_off_plain_db_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Db as Db\n\
               import Ipe.Task as Task\n\
               main =\n\
                   Task.andThen (\\conn -> Db.unsafeExecRaw conn \"SELECT 1\") (Db.open \"sqlite\" \"sqlite::memory:\")\n";
    assert_rejected("security_db_unsafe_exec_raw_off_plain", src, "IPE-N0005");
}

/// SECURITY: the untyped row read `unsafeGetField` no longer lives on the plain
/// `Ipe.Db` surface — it relocated to `Ipe.Db.Unsafe`. Reaching it off a plain
/// `Ipe.Db` import must be rejected, so the decoder-bypassing read cannot be
/// used without the disclosing submodule.
#[test]
fn security_db_unsafe_get_field_off_plain_db_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Db as Db\n\
               import Ipe.Dict as Dict\n\
               main = Db.unsafeGetField \"k\" (Dict.fromList [ ( \"k\", \"v\" ) ])\n";
    assert_rejected("security_db_unsafe_get_field_off_plain", src, "IPE-N0005");
}

/// SECURITY (contrapositive): the marked replacements, homed in `Ipe.Db.Unsafe`,
/// still compile — the raw-SQL and untyped-read capabilities are preserved, only
/// relocated to the disclosing submodule that names the risk.
#[test]
fn security_db_unsafe_members_compile_off_submodule() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Db.Unsafe as Unsafe\n\
               import Ipe.Dict as Dict\n\
               fieldVal = Unsafe.unsafeGetField \"k\" (Dict.fromList [ ( \"k\", \"v\" ) ])\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println \"ok\"\n";
    assert_compiles("security_db_unsafe_members", src);
}

/// SECURITY: `unsafeFragment` — the un-validated anti-`Sql.column` — is homed in
/// `Ipe.Db.Unsafe`, mints a `SqlFragment` from an unchecked string, and compiles
/// there. The deliberate skip of `Sql.column`'s `valid_sql_ident` gate is the
/// disclosed hatch: it is reachable ONLY through the `.Unsafe` import.
#[test]
fn security_db_unsafe_fragment_compiles_off_submodule() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Db.Sql as Sql\n\
               import Ipe.Db.Unsafe as Unsafe\n\
               fragCond = Sql.eq (Unsafe.unsafeFragment \"users.id\") (Sql.int 1)\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println \"ok\"\n";
    assert_compiles("security_db_unsafe_fragment", src);
}

/// SECURITY: `unsafeFragment` did NOT leak onto the safe `Ipe.Db.Sql` surface —
/// it is a member of `Ipe.Db.Unsafe` only. Reaching `Sql.unsafeFragment` off a
/// plain `Ipe.Db.Sql` import must be rejected, so the un-validated mint cannot
/// be used without the disclosing submodule.
#[test]
fn security_db_unsafe_fragment_off_plain_sql_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Db.Sql as Sql\n\
               main = Sql.unsafeFragment \"users.id\"\n";
    assert_rejected(
        "security_db_unsafe_fragment_off_plain_sql",
        src,
        "IPE-N0005",
    );
}

/// SECURITY (untouched safe default): `Sql.column` — the VALIDATED identifier
/// path — stays on the plain `Ipe.Db.Sql` surface and compiles unchanged. Its
/// `valid_sql_ident` gate + poison-on-invalid behaviour is the safe default the
/// `unsafeFragment` hatch deliberately skips; tightening the raw surface leaves
/// the validated builder intact. (The runtime poison behaviour is proved by
/// `ipe_runtime::db::test_poisoned_column_surfaces_as_task_err`.)
#[test]
fn security_db_sql_column_still_validates_and_compiles() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Db.Sql as Sql\n\
               colCond = Sql.eq (Sql.column \"users.id\") (Sql.int 1)\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println \"ok\"\n";
    assert_compiles("security_db_sql_column_validates", src);
}

/// SECURITY: the verbatim JSON-LD `<script>` hatch `unsafeJsonLd` no longer
/// lives on the plain `Ipe.Web.Head` surface — it relocated to
/// `Ipe.Web.Head.Unsafe`. A program that imports only `Ipe.Web.Head` and reaches
/// for `Head.unsafeJsonLd` must be rejected, so the raw-script injection surface
/// cannot be used without the disclosing `Ipe.Web.Head.Unsafe` import.
#[test]
fn security_web_head_unsafe_json_ld_off_plain_head_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Web.Head as Head\n\
               main = Head.unsafeJsonLd \"{}\"\n";
    assert_rejected(
        "security_web_head_unsafe_json_ld_off_plain",
        src,
        "IPE-N0005",
    );
}

/// SECURITY (contrapositive): the marked member, homed in `Ipe.Web.Head.Unsafe`,
/// still compiles — the verbatim JSON-LD capability is preserved, only relocated
/// to the disclosing submodule that names the risk.
#[test]
fn security_web_head_unsafe_json_ld_compiles_off_submodule() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Web.Head.Unsafe as Unsafe\n\
               jsonLdTag = Unsafe.unsafeJsonLd \"{}\"\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println \"ok\"\n";
    assert_compiles("security_web_head_unsafe_json_ld", src);
}

/// SECURITY: the blunt raw secret-reveal `reveal` no longer lives on the plain
/// `Ipe.Secret` surface — it relocated to `Ipe.Secret.Unsafe.unsafeReveal`. A
/// program that imports only `Ipe.Secret` and reaches for `Secret.reveal` must
/// be rejected, so un-sealing a `Secret` into a bare `String` cannot happen
/// without the disclosing `Ipe.Secret.Unsafe` import.
#[test]
fn security_secret_reveal_off_plain_secret_is_rejected() {
    let src = "module Main exposing (main)\n\
               import Ipe.Secret as Secret\n\
               main =\n\
                   Secret.reveal (Secret.fromString \"sk\")\n";
    assert_rejected("security_secret_reveal_off_plain", src, "IPE-N0005");
}

/// SECURITY (contrapositive): the relocated `unsafeReveal`, homed in
/// `Ipe.Secret.Unsafe`, still compiles — the raw un-seal capability is
/// preserved, only relocated to the disclosing submodule that names the risk.
#[test]
fn security_secret_unsafe_reveal_compiles_off_submodule() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Secret as Secret\n\
               import Ipe.Secret.Unsafe as Unsafe\n\
               revealed = Unsafe.unsafeReveal (Secret.fromString \"sk\")\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println revealed\n";
    assert_compiles("security_secret_unsafe_reveal", src);
}

/// SECURITY (the safe scoped default): `Secret.use` — the scoped consume — stays
/// on the native `Ipe.Secret` surface and compiles off a plain `import
/// Ipe.Secret`, WITHOUT any `Ipe.Secret.Unsafe` import. It is capability-neutral
/// (the disclosure half is proved in `ipe_lower::capabilities`'s
/// `importing_ipe_secret_unsafe_discloses_unsafe` and its no-unsafe partition):
/// the common scoped case never touches the `unsafe` axis.
#[test]
fn security_secret_use_compiles_off_plain_secret() {
    let src = "module Main exposing (main)\n\
               import Ipe.Io as Io\n\
               import Ipe.Secret as Secret\n\
               main : Task Error ()\n\
               main =\n\
               \x20   Io.println (Secret.use (Secret.fromString \"sk\") (\\plain -> plain))\n";
    assert_compiles("security_secret_use_scoped", src);
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

/// A `CustomElement down up` annotation whose two parameters are plain, closed
/// value types (here two primitives) TYPE-RESOLVES at canon — the arity and SEAL
/// gates pass. It is not emittable: the widget transport is not shipped, so it is
/// rejected FAIL-CLOSED at emission with IPE-L0133, NOT at canon. That the
/// rejection is the emission code (not a canon `IPE-N0xxx`) proves canon accepted
/// the annotation; the SEAL contract — no untyped value ever reaches codegen — is
/// upheld by the emission-level gate.
#[test]
fn canon_custom_element_use_resolves_then_fail_closed_at_emit() {
    let src = format!(
        "{HEAD}editor : CustomElement Int String\n\
         editor = editor\n\
         main = 1\n"
    );
    assert_rejected("canon_custom_element_use", &src, "IPE-L0133");
}

/// A QUALIFIED spelling of the boundary type (`D.CustomElement …` through an
/// imported module) is checked by NAME regardless of its non-empty home: the
/// arity + SEAL gates pass for two primitives, so it too type-RESOLVES at canon
/// and is rejected only at emission (IPE-L0133) — never slipping past a home gate
/// into a build-time ICE.
#[test]
fn canon_custom_element_qualified_use_resolves_then_fail_closed_at_emit() {
    let src = format!(
        "{HEAD}import Ipe.Dict as D\n\
         editor : D.CustomElement Int String\n\
         editor = editor\n\
         main = 1\n"
    );
    assert_rejected("canon_custom_element_qualified", &src, "IPE-L0133");
}

/// The boundary SEAL accepts plain, closed, concrete value types transitively:
/// a `CustomElement` whose down-state is a user record alias and whose up-event
/// is a user ADT over primitives type-RESOLVES at canon (arity + seal pass), so
/// the only rejection is the emission-level IPE-L0133. This is the POSITIVE seal
/// case — the intended `CustomElement EditorState EditorEvent` shape is admitted.
#[test]
fn canon_custom_element_plain_user_seal_resolves() {
    let src = format!(
        "{HEAD}type alias EditorState = {{ text : String, cursor : Int }}\n\
         type EditorEvent = TextChanged String | CursorMoved Int\n\
         editor : CustomElement EditorState EditorEvent\n\
         editor = editor\n\
         main = 1\n"
    );
    assert_rejected("canon_custom_element_plain_seal", &src, "IPE-L0133");
}

/// `CustomElement` demands EXACTLY two type parameters — the sealed down-state
/// and the up-event. Too FEW is a clean arity error (IPE-N0031, the same code the
/// closed built-in containers use), checked before the seal so a mis-shaped
/// annotation never reaches emission.
#[test]
fn canon_custom_element_arity_too_few() {
    let src = format!("{HEAD}x : CustomElement Int\nx = x\nmain = 1\n");
    assert_rejected("canon_custom_element_arity1", &src, "IPE-N0031");
}

/// Too MANY type parameters is the same arity rejection (IPE-N0031).
#[test]
fn canon_custom_element_arity_too_many() {
    let src = format!("{HEAD}x : CustomElement Int String Bool\nx = x\nmain = 1\n");
    assert_rejected("canon_custom_element_arity3", &src, "IPE-N0031");
}

/// `Ipe.Db`'s external-connection handle is `Connection mode` — EXACTLY one
/// phantom access-mode argument. A bare `Connection` (too few) is a clean arity
/// error (IPE-N0031), NOT the empty-home lowerer ICE (IPE-I0001) it produced
/// before the arity gate covered the parametric reserved builtin.
#[test]
fn canon_connection_arity_too_few() {
    let src = format!("{HEAD}x : Connection\nx = x\nmain = 1\n");
    assert_rejected("canon_connection_arity0", &src, "IPE-N0031");
}

/// Too MANY arguments (`Connection a b`) is the same clean arity rejection
/// (IPE-N0031), again never the empty-home ICE.
#[test]
fn canon_connection_arity_too_many() {
    let src = format!("{HEAD}x : Connection ReadOnly ReadWrite\nx = x\nmain = 1\n");
    assert_rejected("canon_connection_arity2", &src, "IPE-N0031");
}

/// The SEAL rejects a function-carrying boundary parameter (IPE-N0039): a
/// function is behaviour, not a serialisable value, and must never cross the
/// Ipê↔JS seam. Fail-closed at the type level, before emission.
#[test]
fn canon_custom_element_seal_rejects_function() {
    let src = format!("{HEAD}x : CustomElement (Int -> Int) String\nx = x\nmain = 1\n");
    assert_rejected("canon_custom_element_seal_fn", &src, "IPE-N0039");
}

/// The SEAL rejects a `Secret`-carrying boundary parameter (IPE-N0039): a
/// secret-tier value must never be serialised across the JS seam. This is the
/// security-critical exclusion the seal adds over the plain-value gate.
#[test]
fn canon_custom_element_seal_rejects_secret() {
    let src = format!("{HEAD}x : CustomElement Secret String\nx = x\nmain = 1\n");
    assert_rejected("canon_custom_element_seal_secret", &src, "IPE-N0039");
}

/// The SEAL rejects a type-variable boundary parameter (IPE-N0039): the seal is
/// monomorphic and concrete, so a bare type variable — which has no single
/// generated codec — is refused fail-closed.
#[test]
fn canon_custom_element_seal_rejects_type_variable() {
    let src = format!("{HEAD}x : CustomElement a String\nx = x\nmain = 1\n");
    assert_rejected("canon_custom_element_seal_tyvar", &src, "IPE-N0039");
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

/// An `Ffi.kernel "Name"` alias in USER source — minting a kernel is reserved to
/// the vouched stdlib / FFI interface, so it is rejected by the origin gate
/// (IPE-N0042) before the registry is consulted, whether or not the named kernel
/// is registered.
#[test]
fn canon_user_kernel_alias_is_rejected() {
    let src = format!(
        "{HEAD}import Ipe.Ffi as Ffi\n\
         bogus : Int -> Int\n\
         bogus = Ffi.kernel \"No_such_kernel_at_all\"\n\
         main = bogus 1\n"
    );
    assert_rejected("canon_user_kernel_alias_is_rejected", &src, "IPE-N0042");
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

// A `List`/`Dict` value CAN store a function on the `Arc<dyn Fn>` carrier, but a
// higher-order kernel whose mapper/comparator carrier the lowerer does NOT align
// to that stored `Arc` cannot pass the function to its closure — it would emit an
// `Arc`-vs-`Box` mismatch. Each such open-frontier kernel over a
// function-carrying collection must fail closed at `ipe` time with IPE-L0134,
// never `ipe`-accept then `cargo`-fail (THE SEAL).

/// `List.member` over a `List (Int -> Int)`: the element is a stored function,
/// which is `Clone` but not `PartialEq` — membership needs `==` on the element,
/// so it must fail closed with IPE-L0134 (the equality-requiring case).
#[test]
fn lower_list_member_over_function_element_gated() {
    let src = format!(
        "{HEAD}import Ipe.List\n\
         steps : List (Int -> Int)\n\
         steps =\n    [ \\n -> n + 1, \\n -> n * 2 ]\n\
         main =\n\
         \x20   let result = List.member (\\n -> n + 1) steps\n\
         \x20   in\n\
         \x20   result\n"
    );
    assert_rejected("lower_list_member_fn_elem", &src, "IPE-L0134");
}

/// `Dict.map` over a `Dict String (Int -> Int)`: the value is a stored function,
/// and `dict_map`'s runtime `V: Clone` bound plus the un-aligned mapper closure
/// carrier make it unsound — fail closed with IPE-L0134.
#[test]
fn lower_dict_map_over_function_value_gated() {
    let src = format!(
        "{HEAD}import Ipe.Dict as Dict\n\
         table : Dict String (Int -> Int)\n\
         table =\n    Dict.fromList [ ( \"inc\", \\n -> n + 1 ) ]\n\
         main =\n\
         \x20   let result = Dict.map (\\_ f -> f) table\n\
         \x20   in\n\
         \x20   result\n"
    );
    assert_rejected("lower_dict_map_fn_value", &src, "IPE-L0134");
}

/// `Dict.foldl` over a function-valued dict: the fold closure receives the
/// stored function on the un-aligned `Box` carrier — fail closed with IPE-L0134.
#[test]
fn lower_dict_foldl_over_function_value_gated() {
    let src = format!(
        "{HEAD}import Ipe.Dict as Dict\n\
         table : Dict String (Int -> Int)\n\
         table =\n    Dict.fromList [ ( \"inc\", \\n -> n + 1 ) ]\n\
         main =\n\
         \x20   let result = Dict.foldl (\\_ f acc -> f acc) 0 table\n\
         \x20   in\n\
         \x20   result\n"
    );
    assert_rejected("lower_dict_foldl_fn_value", &src, "IPE-L0134");
}

/// `Dict.filter` over a function-valued dict: `dict_filter`'s `V: Clone` bound
/// plus the un-aligned predicate carrier make it unsound — fail closed.
#[test]
fn lower_dict_filter_over_function_value_gated() {
    let src = format!(
        "{HEAD}import Ipe.Dict as Dict\n\
         table : Dict String (Int -> Int)\n\
         table =\n    Dict.fromList [ ( \"inc\", \\n -> n + 1 ) ]\n\
         main =\n\
         \x20   let result = Dict.filter (\\_ _ -> True) table\n\
         \x20   in\n\
         \x20   result\n"
    );
    assert_rejected("lower_dict_filter_fn_value", &src, "IPE-L0134");
}

/// `Dict.partition` over a function-valued dict: same open-frontier class —
/// fail closed with IPE-L0134.
#[test]
fn lower_dict_partition_over_function_value_gated() {
    let src = format!(
        "{HEAD}import Ipe.Dict as Dict\n\
         table : Dict String (Int -> Int)\n\
         table =\n    Dict.fromList [ ( \"inc\", \\n -> n + 1 ) ]\n\
         main =\n\
         \x20   let result = Dict.partition (\\_ _ -> True) table\n\
         \x20   in\n\
         \x20   result\n"
    );
    assert_rejected("lower_dict_partition_fn_value", &src, "IPE-L0134");
}

/// `List.sortBy` over a `List (Int -> Int)`: the key extractor receives the
/// stored function on the un-aligned carrier — fail closed with IPE-L0134.
#[test]
fn lower_list_sort_by_over_function_element_gated() {
    let src = format!(
        "{HEAD}import Ipe.List\n\
         steps : List (Int -> Int)\n\
         steps =\n    [ \\n -> n + 1, \\n -> n * 2 ]\n\
         main =\n\
         \x20   let result = List.sortBy (\\f -> f 0) steps\n\
         \x20   in\n\
         \x20   result\n"
    );
    assert_rejected("lower_list_sort_by_fn_elem", &src, "IPE-L0134");
}

/// A generic union `Wrap a` at a non-`Clone` concrete payload (`Task Error Int`)
/// reused across two value-consuming positions: the union derives
/// `Clone where T: Clone`, but a `Task` is never `Clone`, so the value-reuse
/// rewrite cannot duplicate it — fail closed with IPE-L0135 rather than emit
/// Rust that fails cargo (E0382/E0277) after `ipe` exit 0.
#[test]
fn lower_union_task_reuse_gated() {
    let src = format!(
        "{HEAD}import Ipe.Task as Task\n\
         type Wrap a = Wrap a\n\
         unwrap : Wrap a -> a\n\
         unwrap w =\n\
         \x20   case w of\n\
         \x20       Wrap x -> x\n\
         pair : Wrap (Task Error Int) -> ( Task Error Int, Task Error Int )\n\
         pair w =\n    ( unwrap w, unwrap w )\n\
         main =\n    pair (Wrap (Task.succeed 7))\n"
    );
    assert_rejected("lower_union_task_reuse", &src, "IPE-L0135");
}

/// CONTRAPOSITIVE: a function-valued `Dict` used only through move/clone kernels
/// (`Dict.get`, projected out and applied) is sound over the `Arc` carrier and
/// must still compile — the fail-closed gate rejects only the open-frontier
/// higher-order kernels, never the storable-value path (the `dict_fn_dispatch`
/// golden shape).
#[test]
fn lower_dict_function_value_get_and_apply_compiles() {
    let src = format!(
        "{HEAD}import Ipe.Dict as Dict\n\
         import Ipe.Io as Io\n\
         import Ipe.String as String\n\
         table : Dict String (Int -> Int)\n\
         table =\n    Dict.fromList [ ( \"inc\", \\n -> n + 1 ) ]\n\
         applyNamed : String -> Int -> Int\n\
         applyNamed name x =\n\
         \x20   case Dict.get name table of\n\
         \x20       Just f ->\n\
         \x20           f x\n\
         \n\
         \x20       Nothing ->\n\
         \x20           x\n\
         main : Task Error ()\n\
         main =\n    Io.println (String.fromInt (applyNamed \"inc\" 41))\n"
    );
    assert_compiles("lower_dict_fn_value_get_apply", &src);
}

// A program's `main` is the single effect it runs, so it must be a `Task Error ()`
// — written directly (a script) or produced by an app entry (`Web.app` /
// `Terminal.appScreen` / `WebView.app`, each of which is itself a `Task Error ()`).
// A `main` of any other type (an `Int`, a `String`, a function) has no effect to
// run: the emitted entry wraps `main` in the runtime's single run site, which needs
// a `Task`, so a non-`Task` `main` would ship a crate that cannot build. That must
// fail closed at `ipe` time with IPE-L0136, never `ipe`-accept then cargo-fail on
// `block_on(<non-task>)` (THE SEAL for the program entry).

/// A `main` annotated `Int` is a value, not an effect — rejected with IPE-L0136
/// rather than accepted and emitted as `block_on(i64)` (which cannot build).
#[test]
fn lower_non_task_main_int_rejected() {
    let src = format!("{HEAD}main : Int\nmain = 42\n");
    assert_rejected("lower_non_task_main_int", &src, "IPE-L0136");
}

/// A `main` annotated `String` is likewise a value, not an effect — IPE-L0136.
#[test]
fn lower_non_task_main_string_rejected() {
    let src = format!("{HEAD}main : String\nmain = \"hello\"\n");
    assert_rejected("lower_non_task_main_string", &src, "IPE-L0136");
}

/// A `main` that is a bare function has no effect to run — IPE-L0136.
#[test]
fn lower_non_task_main_function_rejected() {
    let src = format!("{HEAD}main = \\x -> x\n");
    assert_rejected("lower_non_task_main_function", &src, "IPE-L0136");
}

/// A `main` that TAKES a parameter is a function, not the single effect the
/// program runs — the emitted entry calls `ipe_main()` with no arguments, so a
/// parameterised `main` (even one that returns a `Task`) cannot build. IPE-L0136.
#[test]
fn lower_parameterised_main_rejected() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         main : Int -> Task Error ()\n\
         main n =\n    Io.println \"hi\"\n"
    );
    assert_rejected("lower_parameterised_main", &src, "IPE-L0136");
}

/// CONTRAPOSITIVE: a `main : Task Error ()` script is a runnable entry and must be
/// accepted — the gate rejects only a `main` that is NOT a `Task` (nor an app
/// entry, which is itself a `Task`), never a genuine effect entry.
#[test]
fn lower_task_main_script_compiles() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         main : Task Error ()\n\
         main =\n    Io.println \"hello\"\n"
    );
    assert_compiles("lower_task_main_script", &src);
}

// `_` reserves an IGNORE slot INSIDE a larger pattern (`let (a, _) = …`), never
// a whole binding of its own. A `let` whose entire bound pattern is a bare `_`
// binds nothing — the value is computed only to be discarded — so it is rejected
// fail-closed with IPE-N0043, in every context (a `Task` effect OR a pure value,
// a `Task`-returning function OR a plain one). To run an effect and ignore its
// result, sequence it with `|> Task.andThen (\_ -> rest)`; to drop a pure value,
// delete the binding.

/// A bare `let _ = <task>` inside a plain (non-`Task`) function is rejected —
/// there is no whole-binding wildcard. Rejected with IPE-N0043.
#[test]
fn lower_bare_wildcard_let_in_sync_context_rejected() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         shout : String -> String\n\
         shout s =\n\
         \x20   let _ = Io.println s\n\
         \x20   in\n\
         \x20   s\n\
         main : Task Error ()\n\
         main =\n    Io.println (shout \"hi\")\n"
    );
    assert_rejected("lower_bare_wildcard_let_sync", &src, "IPE-N0043");
}

/// A bare `let _ = <task>` is ALSO rejected inside a `Task`-returning function —
/// the auto-desugar to a sequenced `TaskSeq` is gone; a discarded effect must be
/// threaded explicitly with `Task.andThen`. Rejected with IPE-N0043.
#[test]
fn lower_bare_wildcard_let_in_task_context_rejected() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         main : Task Error ()\n\
         main =\n\
         \x20   let _ = Io.println \"step one\"\n\
         \x20   in\n\
         \x20   Io.println \"step two\"\n"
    );
    assert_rejected("lower_bare_wildcard_let_task", &src, "IPE-N0043");
}

/// CONTRAPOSITIVE: the sanctioned rewrite — sequence the effect with
/// `Task.andThen (\_ -> rest)` — compiles. A `main : Task Error ()` that runs
/// one print, ignores its result, and returns another must still compile.
#[test]
fn lower_effect_sequenced_with_and_then_compiles() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         import Ipe.Task as Task\n\
         main : Task Error ()\n\
         main =\n\
         \x20   Io.println \"step one\"\n\
         \x20       |> Task.andThen (\\_ -> Io.println \"step two\")\n"
    );
    assert_compiles("lower_effect_sequenced_and_then", &src);
}

/// CONTRAPOSITIVE: a `_` NESTED inside a larger let pattern is NOT a bare
/// whole-binding wildcard and stays legal — `let (_, b) = (1, 2)` binds `b` and
/// ignores the first component.
#[test]
fn lower_nested_wildcard_in_let_pattern_compiles() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         main : Task Error ()\n\
         main =\n\
         \x20   let (_, b) = (\"one\", \"two\")\n\
         \x20   in\n\
         \x20   Io.println b\n"
    );
    assert_compiles("lower_nested_wildcard_let", &src);
}

/// CONTRAPOSITIVE: `Debug.log` is the sanctioned debug print — it returns its
/// value (`String -> a -> a`), not a `Task`, so it is usable inside a pure
/// function without escaping the effect discipline. A development build accepts
/// it (a production `--optimize` build rejects it with IPE-L0140, covered
/// separately).
#[test]
fn lower_debug_log_in_sync_context_compiles() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         import Ipe.Debug as Debug\n\
         shout : String -> String\n\
         shout s =\n    Debug.log \"shout\" s\n\
         main : Task Error ()\n\
         main =\n    Io.println (shout \"hi\")\n"
    );
    assert_compiles("lower_debug_log_sync", &src);
}

/// The applicative record-codec builder seed — `object ctor` for a `Builder`
/// wrapping a `{} -> Decoder ctor` factory — routes a curried constructor
/// through a decoder field by a type variable. The generic accumulator emits a
/// `Clone`-bounded carrier the boxed constructor cannot satisfy, so it must
/// fail closed with IPE-L0107 (the sanctioned form is the direct
/// `Decode.succeed ctor |> Pipeline.required …` pipeline, covered by the
/// `lower_pipeline_curried_constructor_compiles` contrapositive below).
#[test]
fn lower_codec_builder_seed_fn_through_type_var_gated() {
    let src = format!(
        "{HEAD}import Ipe.Json.Encode as Encode\n\
         import Ipe.Json.Decode as Decode\n\
         type Builder a\n\
         \x20   = Builder\n\
         \x20       {{ enc : a -> Value\n\
         \x20       , dec : Decoder a\n\
         \x20       }}\n\
         object : ctor -> Builder ctor\n\
         object ctor =\n\
         \x20   Builder\n\
         \x20       {{ enc = \\_ -> Encode.null\n\
         \x20       , dec = Decode.succeed ctor\n\
         \x20       }}\n\
         type alias Person =\n    {{ name : String, age : Int }}\n\
         mkPerson : String -> Int -> Person\n\
         mkPerson name age =\n    {{ name = name, age = age }}\n\
         personBuilder : Builder (String -> Int -> Person)\n\
         personBuilder =\n    object mkPerson\n\
         main =\n    personBuilder\n"
    );
    assert_rejected("lower_codec_builder_seed", &src, "IPE-L0107");
}

/// The full applicative builder chain — `object ctor |> field … |> field …`
/// applying the curried constructor one argument at a time through the
/// accumulator's decoder — requires nested-closure lowering of a curried
/// constructor across a generic carrier frontier that is not implemented (the
/// `and_map_curried_stays_gated` boundary). It must fail closed with IPE-L0107,
/// never a silent accept that later `cargo`-fails (probed: bypassing the gate
/// emits a curry-arity `E0308` plus a `Box`/`Arc` decoder-payload frontier
/// `E0308`).
#[test]
fn lower_codec_builder_field_chain_gated() {
    let src = format!(
        "{HEAD}import Ipe.Json.Encode as Encode\n\
         import Ipe.Json.Decode as Decode\n\
         type Codec a\n\
         \x20   = Codec {{ enc : a -> Value, mkDec : {{}} -> Decoder a }}\n\
         type ObjectCodec rec fn\n\
         \x20   = ObjectCodec\n\
         \x20       {{ encField : rec -> List ( String, Value )\n\
         \x20       , mkDecPartial : {{}} -> Decoder fn\n\
         \x20       }}\n\
         object : fn -> ObjectCodec rec fn\n\
         object ctor =\n\
         \x20   ObjectCodec {{ encField = \\_ -> [], mkDecPartial = \\_ -> Decode.succeed ctor }}\n\
         field : String -> (rec -> f) -> Codec f -> ObjectCodec rec (f -> fn) -> ObjectCodec rec fn\n\
         field key get valueCodec acc =\n\
         \x20   case acc of\n\
         \x20       ObjectCodec a ->\n\
         \x20           case valueCodec of\n\
         \x20               Codec v ->\n\
         \x20                   ObjectCodec\n\
         \x20                       {{ encField = \\rec -> ( key, v.enc (get rec) ) :: a.encField rec\n\
         \x20                       , mkDecPartial = \\_ -> Decode.map2 (\\fn x -> fn x) (a.mkDecPartial {{}}) (Decode.field key (v.mkDec {{}}))\n\
         \x20                       }}\n\
         intCodec : Codec Int\n\
         intCodec =\n    Codec {{ enc = \\n -> Encode.int n, mkDec = \\_ -> Decode.int }}\n\
         type alias P =\n    {{ a : Int, b : Int }}\n\
         mkP : Int -> Int -> P\n\
         mkP a b =\n    {{ a = a, b = b }}\n\
         acc : ObjectCodec P P\n\
         acc =\n\
         \x20   object mkP\n\
         \x20       |> field \"a\" .a intCodec\n\
         \x20       |> field \"b\" .b intCodec\n\
         main =\n    acc\n"
    );
    assert_rejected("lower_codec_builder_chain", &src, "IPE-L0107");
}

/// CONTRAPOSITIVE: the sanctioned record-codec decode form — a monomorphic
/// `Decode.succeed ctor` threading a curried constructor through the
/// `Pipeline.required` chain — must still compile. This is the working shape the
/// `Ipe.Codec` module doc and the IPE-L0107 explain page point users to; the two
/// gated cases above must reject WITHOUT closing this door.
#[test]
fn lower_pipeline_curried_constructor_compiles() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         import Ipe.Json.Decode as Decode\n\
         import Ipe.Json.Decode.Pipeline as Pipeline\n\
         type alias User =\n    {{ id : String, age : Int }}\n\
         mkUser : String -> Int -> User\n\
         mkUser id age =\n    {{ id = id, age = age }}\n\
         userDecoder : Decoder User\n\
         userDecoder =\n\
         \x20   Decode.succeed mkUser\n\
         \x20       |> Pipeline.required \"id\" Decode.string\n\
         \x20       |> Pipeline.required \"age\" Decode.int\n\
         main : Task Error ()\n\
         main =\n\
         \x20   Io.println \"ok\"\n"
    );
    assert_compiles("lower_pipeline_curried_constructor", &src);
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
