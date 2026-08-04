//! Decoder-pipeline footgun gate (IPE-N0040), end to end through the driver.
//!
//! The `Db.Decode` / `Json.Decode.Pipeline` `required` / `optional` /
//! `requiredAt` / `custom` combinators take the accumulated constructor-decoder
//! as their LAST argument. Written as a hand-nested application, the innermost
//! combinator binds to the constructor's FIRST parameter, so first-in-source
//! binds to the LAST parameter — a silent field↔parameter reversal that raises
//! no type error whenever adjacent fields share a runtime type. The idiomatic
//! `|>` pipe form threads the accumulator the other way and is correct.
//!
//! These tests pin both directions at the driver boundary: the nested form is
//! rejected with IPE-N0040, and the idiomatic pipe form is accepted unchanged.

use std::path::{Path, PathBuf};

use ipe::CliError;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[allow(clippy::expect_used)] // test helper: a failed scratch-dir setup IS the failure
fn write_entry(dir: &Path, source: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mkdir scratch");
    let entry = dir.join("Main.ipe");
    std::fs::write(&entry, source).expect("write entry");
    entry
}

#[allow(clippy::expect_used)] // test helper: an unresolvable runtime IS the failure
fn build(entry: &Path, out: &Path) -> Result<(), CliError> {
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");
    ipe::build(entry, out, &runtime)
}

/// The hand-nested `Db.Decode.required` form is rejected with IPE-N0040
/// (`ReverseNestedDecoderPipeline`) before type-checking, so the silent
/// field-reversal miscompile can never reach the emitted crate.
#[test]
fn nested_db_required_is_rejected_with_n0040() {
    let dir = scratch("dec_gate_db_nested");
    let entry = write_entry(
        &dir.join("srcdir"),
        "module Main exposing (main)\n\
         import Ipe.Db.Decode as DbDecode\n\
         import Ipe.Io as Io\n\
         \n\
         decoder : DbDecode.Decoder ( Maybe String, Maybe String )\n\
         decoder =\n\
         \x20   DbDecode.required \"a\" (DbDecode.nullable (DbDecode.string \"a\"))\n\
         \x20       (DbDecode.required \"b\" (DbDecode.nullable (DbDecode.string \"b\"))\n\
         \x20           (DbDecode.succeed (\\a b -> ( a, b ))))\n\
         \n\
         main = Io.println \"unused\"\n",
    );
    let out = dir.join("out");
    let err = build(&entry, &out).expect_err("nested Db.Decode.required must be rejected");
    let CliError::Pipeline { diag, .. } = err else {
        return assert_eq!(
            format!("{err:?}"),
            "Pipeline",
            "expected a pipeline diagnostic"
        );
    };
    let rendered = format!("{diag:?}");
    assert!(
        rendered.contains("ReverseNestedDecoderPipeline"),
        "expected IPE-N0040 ReverseNestedDecoderPipeline, got: {rendered}"
    );
}

/// The hand-nested `Json.Decode.Pipeline.required` form (issue sibling) is
/// rejected identically — the Json pipeline shares the applicative shape.
#[test]
fn nested_json_pipeline_required_is_rejected_with_n0040() {
    let dir = scratch("dec_gate_json_nested");
    let entry = write_entry(
        &dir.join("srcdir"),
        "module Main exposing (main)\n\
         import Ipe.Json.Decode as JsonDec\n\
         import Ipe.Json.Decode.Pipeline as JsonDecP\n\
         import Ipe.Io as Io\n\
         \n\
         mk : String -> String -> String\n\
         mk a b = a ++ b\n\
         \n\
         decoder : JsonDec.Decoder String\n\
         decoder =\n\
         \x20   JsonDecP.required \"a\" JsonDec.string\n\
         \x20       (JsonDecP.required \"b\" JsonDec.string\n\
         \x20           (JsonDec.succeed mk))\n\
         \n\
         main = Io.println \"unused\"\n",
    );
    let out = dir.join("out");
    let err = build(&entry, &out).expect_err("nested JsonDecP.required must be rejected");
    let CliError::Pipeline { diag, .. } = err else {
        return assert_eq!(
            format!("{err:?}"),
            "Pipeline",
            "expected a pipeline diagnostic"
        );
    };
    let rendered = format!("{diag:?}");
    assert!(
        rendered.contains("ReverseNestedDecoderPipeline"),
        "expected IPE-N0040 ReverseNestedDecoderPipeline, got: {rendered}"
    );
}

/// The idiomatic `|>` pipe form must be accepted unchanged — it is the one true
/// spelling and threads fields in source order. A build error here would mean
/// the gate falsely rejects a correct decoder.
#[test]
fn piped_db_form_is_accepted() {
    let dir = scratch("dec_gate_db_pipe");
    let entry = write_entry(
        &dir.join("srcdir"),
        "module Main exposing (main)\n\
         import Ipe.Db.Decode as DbDecode\n\
         import Ipe.Io as Io\n\
         \n\
         decoder : DbDecode.Decoder ( Maybe String, Maybe String )\n\
         decoder =\n\
         \x20   DbDecode.succeed (\\a b -> ( a, b ))\n\
         \x20       |> DbDecode.required \"a\" (DbDecode.nullable (DbDecode.string \"a\"))\n\
         \x20       |> DbDecode.required \"b\" (DbDecode.nullable (DbDecode.string \"b\"))\n\
         \n\
         main = Io.println \"unused\"\n",
    );
    let out = dir.join("out");
    build(&entry, &out).expect("the idiomatic Db pipe form must be accepted");
}

/// The idiomatic Json pipeline `|>` form must be accepted unchanged.
#[test]
fn piped_json_form_is_accepted() {
    let dir = scratch("dec_gate_json_pipe");
    let entry = write_entry(
        &dir.join("srcdir"),
        "module Main exposing (main)\n\
         import Ipe.Json.Decode as JsonDec\n\
         import Ipe.Json.Decode.Pipeline as JsonDecP\n\
         import Ipe.Io as Io\n\
         \n\
         mk : String -> String -> String\n\
         mk a b = a ++ b\n\
         \n\
         decoder : JsonDec.Decoder String\n\
         decoder =\n\
         \x20   JsonDec.succeed mk\n\
         \x20       |> JsonDecP.required \"a\" JsonDec.string\n\
         \x20       |> JsonDecP.required \"b\" JsonDec.string\n\
         \n\
         main = Io.println \"unused\"\n",
    );
    let out = dir.join("out");
    build(&entry, &out).expect("the idiomatic Json pipe form must be accepted");
}
