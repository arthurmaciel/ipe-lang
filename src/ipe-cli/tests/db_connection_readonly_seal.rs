//! The external `Ipe.Db.Connection`'s read-only posture is a TYPE, not a runtime
//! flag: `Ipe.Db.Dsn.open` yields a `Connection ReadOnly`, and every mutating
//! kernel requires a `Connection ReadWrite`. So a write against a connection from
//! `open` cannot type-check — the read-only guarantee is a COMPILE error at `ipe`
//! time, never a caught runtime `Err` and never an accept-then-`cargo`-fail.
//!
//! This pins that load-bearing security property from both sides: the negative
//! (a write against `Connection ReadOnly` is rejected at `ipe` time) and the
//! positive (a read/connect against the same connection, and a write against an
//! explicit `Connection ReadWrite`, both type-check and the emitted crate builds —
//! THE SEAL).

use std::fs;
use std::path::{Path, PathBuf};

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    ipe::resolve_runtime().expect("runtime must resolve for this test")
}

#[allow(clippy::expect_used)]
fn write_project(test_name: &str, main_ipe: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipe_conn_ro_seal_{test_name}"));
    let _ = fs::remove_dir_all(&dir);
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("create src/");
    fs::write(src.join("Main.ipe"), main_ipe).expect("write Main.ipe");
    fs::write(dir.join("ipe.toml"), "[project]\nname = \"connroseal\"\n").expect("write ipe.toml");
    dir
}

fn build(dir: &Path, test_name: &str) -> Result<(), ipe::CliError> {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("conn_ro_seal_{test_name}"));
    let _ = fs::remove_dir_all(&out);
    ipe::build_project(&dir.join("ipe.toml"), &out, &runtime())
}

/// A write against a `Connection ReadOnly` — the type a program obtains from
/// `Ipe.Db.Dsn.open` — is a COMPILE error. `unsafeExecRawOn` requires a
/// `Connection ReadWrite`, so handing it a read-only connection cannot unify.
/// This is the centerpiece: the read-only posture is unrepresentable-as-a-write,
/// not a runtime gate.
#[test]
fn write_against_read_only_connection_is_a_type_error() {
    let dir = write_project(
        "ro_write_rejected",
        "\
module Main exposing (main)
import Ipe.Db.Dsn as Dsn exposing (Connection, ReadOnly)
import Ipe.Db.Unsafe as DbU
import Ipe.Task as Task
import Ipe.Error exposing (Error)

badWrite : Connection ReadOnly -> Task Error Int
badWrite conn =
    DbU.unsafeExecRawOn conn \"DELETE FROM t\"

main : Task Error ()
main =
    Task.succeed ()
",
    );
    let built = build(&dir, "ro_write_rejected");
    assert!(
        built.is_err(),
        "a write against a `Connection ReadOnly` must be an `ipe`-time TYPE error — \
         the read-only posture is a compile-time guarantee, never a runtime flag"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The positive half: connecting, reading the read-only connection, and closing
/// it all type-check, AND a write against an explicit `Connection ReadWrite`
/// type-checks. THE SEAL — `ipe`-accept ⇒ the emitted crate `cargo`-builds.
#[test]
fn read_only_connect_and_read_write_write_both_type_check() {
    let dir = write_project(
        "positive_modes",
        "\
module Main exposing (main)
import Ipe.Db.Dsn as Dsn exposing (Dsn, Connection, ReadOnly, ReadWrite)
import Ipe.Db.Unsafe as DbU
import Ipe.Task as Task
import Ipe.Error exposing (Error)

-- open yields Connection ReadOnly; close accepts either mode
connectAndClose : Dsn -> Task Error ()
connectAndClose dsn =
    Task.andThen Dsn.close (Dsn.open dsn)

-- a write against a ReadWrite connection is well-typed
okWrite : Connection ReadWrite -> Task Error Int
okWrite conn =
    DbU.unsafeExecRawOn conn \"DELETE FROM t\"

main : Dsn -> Task Error ()
main dsn =
    Task.map (\\_ -> ()) (connectAndClose dsn)
",
    );
    let built = build(&dir, "positive_modes");
    assert!(
        built.is_ok(),
        "connecting a Dsn (Connection ReadOnly), closing it, and a write against a \
         Connection ReadWrite must all type-check and the emitted crate must build \
         (THE SEAL): {:?}",
        built.err()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The typed READ stack accepts an external `Connection ReadOnly`: `Db.queryDecodeOn`
/// (a per-row decoder) and `Store.findWhereOn` / `Store.getOn` (over one `read`
/// codec) both type-check on a read-only connection AND the emitted crate builds
/// (THE SEAL). This is the §4 monitoring/ETL target — typed reads from a foreign
/// source through one codec — proven end-to-end (`ipe`-accept ⇒ `cargo`-build), so
/// there is no accept-then-`cargo`-fail on the external read path.
#[test]
fn typed_reads_over_read_only_connection_type_check_and_build() {
    let dir = write_project(
        "ro_reads_build",
        "\
module Main exposing (main)
import Ipe.Db.Dsn as Dsn exposing (Connection, ReadOnly)
import Ipe.Db as Db
import Ipe.Db.Decode
import Ipe.Db.Store as Store exposing (Store)
import Ipe.Db.Sql as Sql
import Ipe.Task as Task
import Ipe.Result as Result exposing (Result(..))
import Ipe.Maybe exposing (Maybe)
import Ipe.Error exposing (Error)

-- A per-row decoder read through the raw external queryDecode kernel.
readCode : Connection ReadOnly -> Task Error (List Int)
readCode conn =
    Db.queryDecodeOn conn \"SELECT amount FROM ledger\" [] (Db.Decode.int \"amount\")

-- The Store read surface over the SAME external connection and one `read` codec.
readAll : Connection ReadOnly -> Store -> (Store.Row -> Result Error Int) -> Task Error (List Int)
readAll conn store read =
    Store.allOn conn store read

readOne : Connection ReadOnly -> Store -> (Store.Row -> Result Error Int) -> SqlValue -> Task Error (Maybe Int)
readOne conn store read key =
    Store.getOn conn store read key

readWhere : Connection ReadOnly -> Store -> (Store.Row -> Result Error Int) -> SqlFragment -> Task Error (List Int)
readWhere conn store read cond =
    Store.findWhereOn conn store read cond

main : Task Error ()
main =
    Task.succeed ()
",
    );
    let built = build(&dir, "ro_reads_build");
    assert!(
        built.is_ok(),
        "typed reads (Db.queryDecodeOn, Store.allOn/getOn/findWhereOn) over a \
         `Connection ReadOnly` must type-check and the emitted crate must build \
         (THE SEAL): {:?}",
        built.err()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The write half of the read-only guarantee at the STORE layer: `Store.insert`
/// requires the app `Db`, so handing it an external `Connection ReadOnly` cannot
/// unify — a foreign write through the typed store is unrepresentable, an
/// `ipe`-time type error rather than a runtime gate.
#[test]
fn store_write_against_read_only_connection_is_a_type_error() {
    let dir = write_project(
        "ro_store_write_rejected",
        "\
module Main exposing (main)
import Ipe.Db.Dsn as Dsn exposing (Connection, ReadOnly)
import Ipe.Db.Store as Store exposing (Store)
import Ipe.Task as Task
import Ipe.Error exposing (Error)

badStoreWrite : Connection ReadOnly -> Store -> List ( String, SqlValue ) -> Task Error Int
badStoreWrite conn store cols =
    Store.insert conn store cols

main : Task Error ()
main =
    Task.succeed ()
",
    );
    let built = build(&dir, "ro_store_write_rejected");
    assert!(
        built.is_err(),
        "a `Store.insert` against a `Connection ReadOnly` must be an `ipe`-time TYPE \
         error — the external default has no typed write path"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// `Ipe.Db.Dsn.open` is the SAFE connector — it takes a parsed `Dsn` and needs no
/// `.Unsafe` import, so a program that only connects and reads discloses the
/// `network` and `database` axes but NOT `unsafe`. The raw `unsafeExecRawOn` hatch
/// is the one that discloses `unsafe`, proven by the marker suite.
#[test]
fn safe_open_needs_no_unsafe_import() {
    let dir = write_project(
        "safe_open",
        "\
module Main exposing (main)
import Ipe.Db.Dsn as Dsn exposing (Dsn)
import Ipe.Task as Task
import Ipe.Error exposing (Error)

main : Dsn -> Task Error ()
main dsn =
    Task.andThen Dsn.close (Dsn.open dsn)
",
    );
    let built = build(&dir, "safe_open");
    assert!(
        built.is_ok(),
        "the safe `Ipe.Db.Dsn.open` connector must type-check with no `.Unsafe` \
         import — the parsed-`Dsn` path is the terse, clean default: {:?}",
        built.err()
    );
    let _ = fs::remove_dir_all(&dir);
}
