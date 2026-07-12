//! Regression gate — a post-solve field-access error (SKY-T0012) must attribute
//! to the module that actually OWNS the access, not to a byte-offset collision
//! in another merged module.
//!
//! Historical context (12-skyvote): `info.message` in `Lib/AuthHandlers.sky`
//! failed to resolve (parked `Error`/`ErrorInfo` ADT leaves `info` a flex
//! record). The deferred field-access pass (`resolve_deferred`) returned the
//! diagnostic with an *empty* home, so the driver fell back to `source_for_span`
//! — the byte-offset heuristic — which picked a numerically-closer `class` call
//! in an unrelated `Page/Roadmap.sky` def. The user saw
//! "`class` … has no field `message`" pointing at code that has nothing to do
//! with the failing access.
//!
//! Fix: `FieldAccess` / `RecordUpdate` now carry their def's `home` module path,
//! and `resolve_deferred` returns it alongside the diagnostic so
//! `infer_attributed` maps the error to the correct source file exactly, the
//! same way solver errors already do.
//!
//! This fixture is tuned so the failing `Dep.sky` access sits at a byte offset
//! that the old heuristic mis-attributes to `Main.sky` (its `main` body span
//! numerically contains the offset with a smaller `lo_dist` than `Dep`'s `bad`
//! body). Pre-fix this rendered `--> Main.sky:9`; post-fix it renders
//! `--> Dep.sky:15`.
//!
//! Run:
//! ```text
//! cargo test -p skyc --test golden_t0012_cross_module_attr
//! ```

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn try_build(name: &str) -> Result<(), String> {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(name)
        .join("src")
        .join("Main.sky");
    if !entry.exists() {
        return Err(format!("fixture not found: {}", entry.display()));
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_skyc_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return Ok(());
    };
    skyc::build_with_sibling_discovery(&entry, &out, &runtime).map_err(|e| e.to_string())
}

/// The SKY-T0012 must blame `Dep.sky` (which owns `rec.missing`), never the
/// byte-colliding `Main.sky`.
#[test]
fn t0012_field_error_attributes_to_owning_module() {
    // Runtime unavailable → try_build returns Ok as a skip. Nothing to assert.
    let Err(err) = try_build("t0012_cross_module_attr") else {
        return;
    };
    assert!(err.contains("SKY-T0012"), "expected SKY-T0012, got:\n{err}");
    assert!(
        err.contains("Dep.sky:15"),
        "field error must attribute to the owning module Dep.sky:15, got:\n{err}"
    );
    assert!(
        !err.contains("Main.sky"),
        "field error must NOT mis-attribute to the byte-colliding Main.sky, got:\n{err}"
    );
}
