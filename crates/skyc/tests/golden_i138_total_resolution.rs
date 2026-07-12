//! #138 regression gate — total type-name resolution.
//!
//! `sky_canon::canonicalise_type` used to call `unwrap_or_default()` for any
//! unqualified type name absent from `type_home_map`, giving it `home = []`.
//! Genuine builtins (in `RESERVED_BUILTIN_TYPES` / `EXTRA_BUILTIN_TYPE_NAMES`)
//! legitimately carry that empty-home sentinel; the lowerer resolves them by
//! explicit name arm.  Unknown names (user ADTs referenced without importing
//! their module) also got `home = []`, so they fell through to the lowerer's
//! unique-match heuristic, which ICE'd with SKY-I0001 when zero or multiple
//! matches existed.
//!
//! The fix makes resolution TOTAL:
//!   • Unknown unqualified upper-case names → SKY-N0002 (`TypeNotFound`) at
//!     canon time, with a did-you-mean suggestion list.
//!   • Known builtins → empty-home sentinel as before.
//!   • The unique-match heuristic in `ir_type_from_canon` / `ir_type_from_ty`
//!     is removed; an empty-home non-builtin Con now ICEs with a clear message
//!     ("should have been caught by canon `TypeNotFound`") to flag the invariant
//!     violation without obscuring the root cause.
//!
//! Tests:
//!   1. `i138_empty_home_bridge` — `Token` used in annotation, defined in both
//!      `ModA` and `ModB` but not imported as a type → SKY-N0002.
//!   2. `i138_optbridge` — same shape, different payload types → SKY-N0002.
//!   3. `i138_kernel_implicit_positive` — `Request` used without explicit import;
//!      it is in `RESERVED_BUILTIN_TYPES`, so the fix must NOT reject it → exit 0.
//!
//! Run:
//! ```text
//! cargo test -p skyc --test golden_i138_total_resolution
//! ```

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

// ---------------------------------------------------------------------------
// Helper: compile a golden fixture, return the error string on failure.
// ---------------------------------------------------------------------------

fn try_build(name: &str) -> Result<(), String> {
    let root = repo_root();
    let entry = golden_dir(&root, name).join("src").join("Main.sky");
    if !entry.exists() {
        return Err(format!("fixture not found: {}", entry.display()));
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_skyc_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        // No runtime available — skip gracefully so the test suite is not
        // broken by a minimal checkout.
        eprintln!("SKIP {name}: runtime not available");
        return Ok(());
    };
    skyc::build_with_sibling_discovery(&entry, &out, &runtime).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Error cases — must fail closed with SKY-N0002
// ---------------------------------------------------------------------------

/// `Token` used in an annotation but never imported as a type; both `ModA` and
/// `ModB` define their own `Token` ADT.  The ambiguity must surface as
/// SKY-N0002, not an ICE.
#[test]
fn i138_empty_home_bridge_fails_n0002() {
    let err = try_build("i138_empty_home_bridge")
        .expect_err("i138_empty_home_bridge must fail (Token is not in scope)");
    assert!(err.contains("SKY-N0002"), "expected SKY-N0002, got:\n{err}");
    assert!(
        !err.contains("SKY-I0001"),
        "must NOT be an ICE (SKY-I0001), got:\n{err}"
    );
}

/// Same shape as `empty_home_bridge` but with different payload types — a
/// separate repro that caught a distinct ICE path in the original heuristic.
#[test]
fn i138_optbridge_fails_n0002() {
    let err =
        try_build("i138_optbridge").expect_err("i138_optbridge must fail (Token is not in scope)");
    assert!(err.contains("SKY-N0002"), "expected SKY-N0002, got:\n{err}");
    assert!(
        !err.contains("SKY-I0001"),
        "must NOT be an ICE (SKY-I0001), got:\n{err}"
    );
}

// ---------------------------------------------------------------------------
// Positive control — kernel-implicit builtin must still compile
// ---------------------------------------------------------------------------

/// `Request` is in `RESERVED_BUILTIN_TYPES` and receives the empty-home
/// sentinel regardless of whether the user explicitly imports
/// `Sky.Http.Server`.  The `TypeNotFound` gate must NOT reject it.
#[test]
fn i138_kernel_implicit_positive_exits_zero() {
    try_build("i138_kernel_implicit_positive")
        .expect("i138_kernel_implicit_positive must compile (Request is a kernel builtin)");
}

/// `Value` is a kernel-implicit Prelude type (#576) that was missing from all
/// three builtin allowlists (`RESERVED_BUILTIN_TYPES`, `EXTRA_BUILTIN_TYPE_NAMES`,
/// `KERNEL_IMPLICIT_PRELUDE_TYPE_NAMES` — newly added by #138 fix).
/// After the fix it must receive the empty-home sentinel and compile clean.
/// The lowerer handles `Value` via an explicit arm (`IrType::Json`) placed
/// after the `enum_variants` guard.
#[test]
fn i138_kernel_implicit_value_exits_zero() {
    try_build("i138_kernel_implicit_value").expect(
        "i138_kernel_implicit_value must compile (Value is a kernel-implicit Prelude type)",
    );
}
