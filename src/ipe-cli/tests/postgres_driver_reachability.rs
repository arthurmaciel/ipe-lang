//! Class 7 §3 regression: `ipe.toml`'s `[database] driver = "postgres"` must
//! actually change what gets emitted — before this fix it was a silent no-op
//! (`crates/ipe/src/project.rs`'s manifest parser didn't even recognise a
//! `[database]` section, and `ipe_backend_rust::project::emit_program` always
//! wrote the sqlite `config.rs` template regardless of what `ipe.toml` said).
//!
//! No live Postgres needed: this only proves the STRUCTURAL wiring —
//! manifest → `RustBackend::with_db_driver` → `EmitCtx::db_driver` →
//! `emit_program`'s config.rs/Cargo.toml selection — actually threads the
//! driver choice through to the emitted project's files. `crates/ipe/src/project.rs`'s
//! `mod tests` covers the manifest-parsing half in isolation;
//! `crates/ipe_backend_rust/src/project.rs`'s `mod tests` covers the
//! `db_cargo_toml` / template-selection half in isolation. This test proves
//! the two halves are actually wired together end-to-end through
//! `ipe::build_project`.

use std::fs;
use std::path::PathBuf;

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    ipe::resolve_runtime().expect("runtime must resolve for this test")
}

/// Minimal Db-kernel-using Ipê program — enough to set `EmitCtx::uses_db =
/// true` (any `Db.*` call site does), nothing more. Never built/run (no
/// `IPE_E2E` gate here) — this test only inspects the EMITTED files, not
/// runtime behaviour (which needs a live Postgres and is out of scope for the
/// default `cargo test` gate per the Class 7 spec's two-tier test strategy).
const MAIN_IPE: &str = "\
module Main exposing (main)
import Ipe.Db
import Ipe.Task

main =
    Task.andThen
        (\\conn ->
            Db.unsafeExecRaw conn \"CREATE TABLE t (id INTEGER)\"
        )
        (Db.open \"sqlite\" \"sqlite::memory:\")
";

/// Write a minimal project (`ipe.toml` + `src/Main.ipe`) under a fresh temp
/// dir, with the given `[database]` section spliced in verbatim (empty string
/// → no section at all, i.e. the default driver).
#[allow(clippy::expect_used)]
fn write_project(test_name: &str, database_section: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipec_pg_reachability_{test_name}"));
    let _ = fs::remove_dir_all(&dir);
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("create src/");
    fs::write(src.join("Main.ipe"), MAIN_IPE).expect("write Main.ipe");
    fs::write(
        dir.join("ipe.toml"),
        format!("[project]\nname = \"pgtest\"\n{database_section}"),
    )
    .expect("write ipe.toml");
    dir
}

/// The core structural fix: `driver = "postgres"` in `ipe.toml` must cause
/// the emitted `src/ipe_runtime/config.rs` to declare `sqlx::postgres::PgPool`
/// / `PgRow` and `DB_USES_RETURNING_ID: bool = true` — NOT the sqlite
/// template. Before this fix, `RUNTIME_CONFIG_RS_DB` was a single
/// unconditional `include_str!`, so this assertion would have failed (the
/// emitted config.rs would be the sqlite template regardless of `ipe.toml`).
#[test]
fn postgres_driver_selects_postgres_config_template() {
    let dir = write_project("postgres_select", "[database]\ndriver = \"postgres\"\n");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pg_reachability_postgres_select");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_project(&dir.join("ipe.toml"), &out, &runtime());
    assert!(
        built.is_ok(),
        "build_project must succeed for a postgres-driver db-using project: {:?}",
        built.err()
    );

    let config_rs = fs::read_to_string(out.join("src").join("ipe_runtime").join("config.rs"))
        .expect("emitted config.rs must exist");
    assert!(
        config_rs.contains("sqlx::postgres::PgPool"),
        "driver = \"postgres\" must emit the Postgres config.rs template:\n{config_rs}"
    );
    assert!(
        config_rs.contains("sqlx::postgres::PgRow"),
        "driver = \"postgres\" must declare PgRow:\n{config_rs}"
    );
    assert!(
        config_rs.contains("DB_USES_RETURNING_ID: bool = true"),
        "driver = \"postgres\" must set DB_USES_RETURNING_ID = true \
         (Postgres has no LastInsertId — db_insert_row/db_insert_fields key \
         their RETURNING-id branch on this):\n{config_rs}"
    );

    let cargo_toml =
        fs::read_to_string(out.join("Cargo.toml")).expect("emitted Cargo.toml must exist");
    assert!(
        cargo_toml.contains(r#"features = ["runtime-tokio-rustls", "sqlite", "postgres"]"#),
        "driver = \"postgres\" must enable the postgres sqlx feature in Cargo.toml \
         IN ADDITION TO sqlite (the always-emitted telemetry_spill/web::hub/ \
         web::store runtime modules hardcode SqlitePool independently of the \
         app's [database] driver choice — dropping sqlite here was the \
         compile-time gap that made Postgres structurally unreachable, closed \
         2026-07-10 after an independent review caught a cargo-build failure \
         it produced):\n{cargo_toml}"
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&out);
}

/// `IPE_E2E` tier: the emitted Postgres-driver project must actually
/// `cargo build` (isolated target dir) — proves the seal (ipe exit 0
/// implies cargo build exit 0) for the whole Postgres codegen path, not just
/// the config.rs/Cargo.toml source-text assertions above. This is the check
/// that catches a SEAL violation where an exclusive
/// sqlite-vs-postgres sqlx feature selection compiles fine as SOURCE TEXT
/// but fails `cargo build` because always-emitted runtime modules unrelated
/// to the `[database]` driver hardcode `SqlitePool` — source-text greps
/// alone cannot catch a missing Cargo feature dependency.
#[test]
fn postgres_driver_project_cargo_builds() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let dir = write_project(
        "postgres_cargo_build",
        "[database]\ndriver = \"postgres\"\n",
    );
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pg_reachability_postgres_cargo_build");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_project(&dir.join("ipe.toml"), &out, &runtime());
    assert!(
        built.is_ok(),
        "build_project must succeed for a postgres-driver db-using project: {:?}",
        built.err()
    );

    let target = std::env::temp_dir()
        .join("r_class7")
        .join("postgres_driver_cargo_build");
    #[allow(clippy::expect_used)]
    let check_output = std::process::Command::new("cargo")
        .arg("check")
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&out)
        .output()
        .expect("cargo must spawn");
    assert!(
        check_output.status.success(),
        "emitted driver=\"postgres\" project must cargo-check clean \
         (no live Postgres connection needed for `cargo check`)\n\
         --- cargo stderr ---\n{}",
        String::from_utf8_lossy(&check_output.stderr),
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&out);
}

/// Non-regression: a project with NO `[database]` section (or an explicit
/// `driver = "sqlite"`) must still emit the sqlite `config.rs` template —
/// this feature is additive, so every existing sqlite-driver project's
/// emitted output is unaffected.
#[test]
fn no_database_section_still_selects_sqlite_config_template() {
    let dir = write_project("sqlite_default", "");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pg_reachability_sqlite_default");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_project(&dir.join("ipe.toml"), &out, &runtime());
    assert!(
        built.is_ok(),
        "build_project must succeed for the default (no [database] section) project: {:?}",
        built.err()
    );

    let config_rs = fs::read_to_string(out.join("src").join("ipe_runtime").join("config.rs"))
        .expect("emitted config.rs must exist");
    assert!(
        config_rs.contains("sqlx::sqlite::SqlitePool"),
        "no [database] section must still default to the sqlite config.rs template:\n{config_rs}"
    );
    assert!(
        config_rs.contains("DB_USES_RETURNING_ID: bool = false"),
        "sqlite driver must keep DB_USES_RETURNING_ID = false:\n{config_rs}"
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&out);
}
