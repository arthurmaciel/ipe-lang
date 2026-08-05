//! The stringly row-read surface is `Ipe.Db.Unsafe.unsafeQuery` /
//! `unsafeGetField` / `unsafeGetString` / `unsafeGetInt` / `unsafeGetBool`, and
//! neither the old unmarked names nor the relocated `Db.unsafe*` off a plain
//! `import Ipe.Db` exist. This pins the schema-drift property that the
//! silently-coercing row readers are only reachable through the lexically-marked
//! `unsafe` names in the disclosed `Ipe.Db.Unsafe` submodule — a caller cannot
//! fall into a `"" `/`0`-on-drift read by using an unmarked default, nor without
//! disclosing the `unsafe` capability. The typed `Db.queryDecode` +
//! `Db.Decode.*` path (which a missing/renamed column fails closed on) is the
//! unmarked row-read default and stays in `Ipe.Db`.
//!
//! The guarantee is checked on BOTH resolution routes:
//!
//! * the surface qualifier (`Unsafe.unsafeGetField` off `import Ipe.Db.Unsafe`,
//!   and — for the relocation — `Db.unsafeGetField` off plain `Ipe.Db` failing),
//!   and
//! * the `Ffi.kernel "Db_unsafeGetField"` string-alias route
//!   (`detect_kernel_alias` splits the string into `(module, function)` and
//!   looks the pair up in the kernel registry). The alias keys off the UNCHANGED
//!   canonical `("Db", "unsafeGetField")` pair — only the SURFACE home moved —
//!   so the alias still resolves; the old unmarked `("Db", "getField")` pair
//!   does not. This mirrors `db_unsafe_exec_raw_marker.rs`.

use std::fs;
use std::path::{Path, PathBuf};

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    ipe::resolve_runtime().expect("runtime must resolve for this test")
}

#[allow(clippy::expect_used)]
fn write_project(test_name: &str, main_ipe: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipe_db_row_marker_{test_name}"));
    let _ = fs::remove_dir_all(&dir);
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("create src/");
    fs::write(src.join("Main.ipe"), main_ipe).expect("write Main.ipe");
    fs::write(dir.join("ipe.toml"), "[project]\nname = \"dbrowmarker\"\n").expect("write ipe.toml");
    dir
}

fn build(dir: &Path, test_name: &str) -> Result<(), ipe::CliError> {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("db_row_marker_{test_name}"));
    let _ = fs::remove_dir_all(&out);
    ipe::build_project(&dir.join("ipe.toml"), &out, &runtime())
}

// ── Direct-qualifier route ────────────────────────────────────────────────────

/// `Ipe.Db.Unsafe.unsafeGetField` is the marked stringly row accessor — it
/// type-checks when imported from its `.Unsafe` submodule (the untyped
/// convenience stays available for genuinely dynamic reads).
#[test]
fn marked_get_field_type_checks() {
    let dir = write_project(
        "marked",
        "\
module Main exposing (main)
import Ipe.Db.Unsafe
import Ipe.Dict as Dict
import Ipe.Io as Io

main =
    let
        row =
            Dict.fromList [ ( \"name\", \"ada\" ) ]
    in
    Io.println (Unsafe.unsafeGetField \"name\" row)
",
    );
    let built = build(&dir, "marked");
    assert!(
        built.is_ok(),
        "Ipe.Db.Unsafe.unsafeGetField must type-check as the marked stringly accessor: {:?}",
        built.err()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The relocated `unsafeGetField` no longer resolves off a plain `import Ipe.Db`:
/// the untyped row-read surface LEFT `Ipe.Db` for `Ipe.Db.Unsafe`, so reaching
/// it without importing the disclosed `.Unsafe` submodule is a compile error.
#[test]
fn relocated_get_field_off_plain_db_no_longer_compiles() {
    let dir = write_project(
        "relocated_direct",
        "\
module Main exposing (main)
import Ipe.Db
import Ipe.Dict as Dict
import Ipe.Io as Io

main =
    let
        row =
            Dict.fromList [ ( \"name\", \"ada\" ) ]
    in
    Io.println (Db.unsafeGetField \"name\" row)
",
    );
    let built = build(&dir, "relocated_direct");
    assert!(
        built.is_err(),
        "Db.unsafeGetField off a plain `import Ipe.Db` must NOT compile — the \
         untyped row-read surface relocated to the disclosed `Ipe.Db.Unsafe` submodule"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The old unmarked `Db.getField` name is gone: reaching the silently-coercing
/// row read without typing `unsafe` is a compile error, so the drift-prone
/// default is unreachable through the direct qualifier.
#[test]
fn unmarked_get_field_no_longer_compiles() {
    let dir = write_project(
        "unmarked_direct",
        "\
module Main exposing (main)
import Ipe.Db
import Ipe.Dict as Dict
import Ipe.Io as Io

main =
    let
        row =
            Dict.fromList [ ( \"name\", \"ada\" ) ]
    in
    Io.println (Db.getField \"name\" row)
",
    );
    let built = build(&dir, "unmarked_direct");
    assert!(
        built.is_err(),
        "the unmarked Db.getField name must NOT compile — the stringly row read \
         may only be reached through the marked Db.unsafeGetField"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ── Ffi.kernel string-alias route ─────────────────────────────────────────────

/// The alias `Ffi.kernel "Db_unsafeGetField"` names the marked kernel by its
/// registry `(module, function)` pair, so it resolves — the marked surface is
/// reachable through the alias route too.
#[test]
fn marked_kernel_alias_resolves() {
    let dir = write_project(
        "marked_alias",
        "\
module Main exposing (main)
import Ipe.Ffi as Ffi
import Ipe.Dict as Dict
import Ipe.Io as Io

getField : String -> Dict String String -> String
getField =
    Ffi.kernel \"Db_unsafeGetField\"

main =
    let
        row =
            Dict.fromList [ ( \"name\", \"ada\" ) ]
    in
    Io.println (getField \"name\" row)
",
    );
    let built = build(&dir, "marked_alias");
    assert!(
        built.is_ok(),
        "Ffi.kernel \"Db_unsafeGetField\" must resolve to the marked kernel: {:?}",
        built.err()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The old unmarked alias `Ffi.kernel "Db_getField"` names no registered kernel
/// (the `(Db, getField)` pair was renamed away), so it fails closed with
/// `NameError::UnknownKernelAlias` — a smuggled string alias cannot reopen the
/// unmarked row read.
#[test]
fn unmarked_kernel_alias_fails_closed() {
    let dir = write_project(
        "unmarked_alias",
        "\
module Main exposing (main)
import Ipe.Ffi as Ffi
import Ipe.Dict as Dict
import Ipe.Io as Io

getField : String -> Dict String String -> String
getField =
    Ffi.kernel \"Db_getField\"

main =
    let
        row =
            Dict.fromList [ ( \"name\", \"ada\" ) ]
    in
    Io.println (getField \"name\" row)
",
    );
    let built = build(&dir, "unmarked_alias");
    let is_unknown_alias = matches!(
        &built,
        Err(ipe::CliError::Pipeline { diag, .. })
            if matches!(
                &**diag,
                ipe_diagnostics::Diagnostic::Name {
                    msg: ipe_diagnostics::NameError::UnknownKernelAlias { .. },
                    ..
                }
            )
    );
    assert!(
        is_unknown_alias,
        "Ffi.kernel \"Db_getField\" must fail closed with IPE-N0028 — the unmarked \
         stringly row read may not be reached through a smuggled kernel alias: {built:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}
