//! BACKLOG #177 regression — `examples/27-multi-session-chat`'s `cargo build`.
//!
//! Pre-fix: `skyc build` exits 0, but the emitted Rust fails `cargo build`
//! with E0277 (`the trait bound T1: SkyRow is not satisfied`) at every
//! `db_get_string("field".to_string(), &(payload))` call inside a function
//! whose only generic is the wildcard `any` PARAM (example 27's
//! `decodeChatMessage : any -> ChatMessage` / `init : any -> …`).
//!
//! Root cause: the runtime's field accessors are generic
//! `db_get_*<R: SkyRow>(field, &row)`, but the emitted enclosing function
//! carried an unbounded `<T1: Clone>` param, so its body's
//! `db_get_string(_, &payload)` could not prove `payload: SkyRow`.
//!
//! Fix (`crates/sky_ir/src/ir.rs` + `crates/sky_backend_rust/src/emit_expr.rs`):
//! a new `BoundSet::SKY_ROW` flag, rendered by `render_bounds` as
//! `sky_runtime::db::SkyRow`; `emit_func` scans the RENDERED body for a
//! `db_get_` call and, when present, appends the bound to EXACTLY the wildcard
//! `any` generic param (interned with the `anyp_` prefix by the lowerer's
//! `any_param_binders` pool) — never a genuine named tvar, never the record
//! STRUCT. Mirrors the Haskell reference's `ModuleEmitter.hs` `bodyHasDbGet`
//! gate; the record struct stays unbounded (reusable in non-row contexts),
//! mirroring `TypeEmitter.hs:166-177`.
//!
//! Run:
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_i177_db_get_skyrow_bound
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn entry_path(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("i177_db_get_skyrow_bound")
        .join("Main.sky")
}

/// skyc-0: the compiler must accept the program AND emit `+ SkyRow` on the
/// wildcard-`any` decoder function's own generic while leaving the record
/// STRUCT unbounded — checked unconditionally (cheap, no `cargo`), independent
/// of the `SKY_E2E` gate below.
#[test]
fn i177_skyc_accepts_and_bounds_fn_not_struct() {
    let root = repo_root();
    let entry = entry_path(&root);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i177_db_get_skyrow_bound_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP i177_db_get_skyrow_bound: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i177_db_get_skyrow_bound: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    // The decoder function's wildcard-`any` generic gains the `SkyRow` bound so
    // its `db_get_string(_, &payload)` body type-checks (E0277 half).
    assert!(
        emitted.contains("sky_runtime::db::SkyRow"),
        "the wildcard-`any` decoder function must carry the `SkyRow` bound so \
         its `db_get_string` body type-checks (#177); got main.rs:\n{emitted}"
    );
    assert!(
        emitted.contains("pub fn main_decode_row<T1: Clone + sky_runtime::db::SkyRow>"),
        "the bound belongs on the decoder FUNCTION's generic param, not \
         elsewhere (#177); got main.rs:\n{emitted}"
    );

    // The record struct itself MUST stay unbounded — it is reused in non-row
    // contexts (mirrors the reference's `TypeEmitter.hs` policy). A `struct`
    // line carrying `SkyRow` would be an over-bound regression.
    for line in emitted.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
            assert!(
                !line.contains("SkyRow"),
                "the record struct must NOT carry a `SkyRow` bound — it stays \
                 reusable in non-row contexts (#177); offending line: {line}"
            );
        }
    }
}

/// cargo-0 ∧ run-0: the emitted project actually compiles with `rustc` and
/// prints the decoded record fields. Gated on `SKY_E2E=1` — a real
/// `cargo build`, the only check that would have caught the original SEAL
/// violation (E0277 on `examples/27-multi-session-chat`, `skyc build` clean).
#[test]
fn i177_cargo_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = entry_path(&root);
    let out = std::env::temp_dir().join("skyc_i177_db_get_skyrow_bound_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i177_db_get_skyrow_bound: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i177_db_get_skyrow_bound", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "i177_db_get_skyrow_bound binary must exit 0; got {:?} (stdout: {:?})",
        outcome.exit_code,
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim(),
        "ada,hello",
        "must print the record fields decoded out of the `Dict String String` \
         payload via the wildcard-`any` `Db.getString` decoder; got: {:?}",
        outcome.stdout
    );
}
