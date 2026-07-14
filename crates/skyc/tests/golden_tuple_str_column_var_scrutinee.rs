//! #182 — a VARIABLE (non-literal) tuple `case` with a STRING-LITERAL column.
//!
//! `Std.Ui.Transform.propsToCss` matches `case pair of ( "transform", v ) -> …`
//! (a variable scrutinee, a string-literal column) — the last blocker for
//! `Std.Ui.Transform` / `Std.Ui.Animation` / `26-ui-showcase`. #174 admitted a
//! variable tuple scrutinee but fail-closed any string-literal column to
//! SKY-L0115 (no per-column `.as_str()` coercion machinery on the by-value path).
//!
//! #182 lowers it via the reference's guard mechanism: each `PStr` column
//! becomes a fresh binder plus an `if __sgN.as_str() == "lit"` match guard, so it
//! emits `match pair { (__sg0, v) if __sg0.as_str() == "transform" => …, … }` — a
//! by-value binder + guard, sound for a variable scrutinee. A list / cons column
//! still requires the literal-tuple coerced-column path and stays fail-closed.
//!
//! Gate check (fast, always): `skyc build` succeeds — no SKY-L0115.
//! Green check (`SKY_E2E=1`): the emitted Rust cargo-builds AND runs, the right
//! arm firing for every input — proving the seal (skyc-0 ⟹ cargo-0) AND runtime
//! correctness (a mis-coerced guard firing the wrong arm is the worst outcome).
//!
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_tuple_str_column_var_scrutinee
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn fixture_entry() -> PathBuf {
    repo_root()
        .join("tests")
        .join("golden")
        .join("i182_tuple_str_column_var_scrutinee")
        .join("Main.sky")
}

/// Fast gate: a multi-arm tuple `case` on a VARIABLE scrutinee with a
/// string-literal column builds cleanly — the SKY-L0115 gap is closed for the
/// `Std.Ui.Transform.propsToCss` shape.
#[test]
fn str_column_var_scrutinee_builds() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tuple_str_col_var_scrut_gate");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently rather than fail
    };
    let built = skyc::build(&fixture_entry(), &out, &runtime);
    assert!(
        built.is_ok(),
        "variable-scrutinee string-column tuple `case` must build (#182); got {:?}",
        built.err()
    );
}

/// Seal + runtime correctness: the emitted Rust cargo-builds and runs, the right
/// arm firing for every input. The program prints
/// `scale(0.9)|other|F|T|1121` (see the fixture's arithmetic).
#[test]
fn str_column_var_scrutinee_cargo_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let out = std::env::temp_dir().join("skyc_tuple_str_col_var_scrut_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };

    let built = skyc::build(&fixture_entry(), &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i182_tuple_str_column_var_scrutinee: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i182_tuple_str_column_var_scrutinee", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "emitted string-column tuple-match program must exit 0"
    );
    // The exact string proves the RIGHT arm fired for every probe — a mis-coerced
    // guard would return a different arm's value.
    assert!(
        outcome.stdout.contains("scale(0.9)|other|F|T|1121"),
        "expected 'scale(0.9)|other|F|T|1121'; got:\n{}",
        outcome.stdout
    );
}
