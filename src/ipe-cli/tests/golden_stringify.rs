//! The STRINGIFY obligation (`Basics.toString : a -> String`, the shared
//! lever for the whole Stringify-bounded family). The argument carries a bounded
//! super-var → Rust `IpeStringify`. A scalar / record / ADT satisfies it; a bare
//! function is rejected AT TYPE-CHECK (fail-closed), never emitting an unbounded
//! `basics_to_string::<T>` that `cargo` would reject — the seal is preserved.
//!
//! Positive case is `IPE_E2E`-gated (build + run). The negative case is a pure
//! skyc compile (no cargo), so it always runs.

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

fn compile_golden(name: &str) -> PathBuf {
    let root = repo_root();
    let entry = golden_dir(&root, name).join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);
    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else {
        return out;
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());
    out
}

fn e2e_enabled() -> bool {
    std::env::var("IPE_E2E").is_ok()
}

/// `toString` on scalars compiles + runs (Go `%v`: bool lowercases).
#[test]
fn tostring_scalars_run() {
    if !e2e_enabled() {
        return;
    }
    let dir = compile_golden("m_tostring");
    let out = support::build_and_run_emitted("m_tostring", &dir);
    assert_eq!(out.exit_code, Some(0), "got {:?}", out.exit_code);
    assert_eq!(out.stdout.trim(), "42 true 3");
}

/// `Log.infoWith : String -> List a -> Task Error ()` with Stringify attrs
/// compiles (the obligation on the list element) — a pure skyc compile.
#[test]
fn log_info_with_stringify_attrs_compiles() {
    let root = repo_root();
    let entry = golden_dir(&root, "m_log_with").join("Main.sky");
    let out = std::env::temp_dir().join("skyc_m_log_with_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve");
    let Ok(runtime) = runtime else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "Log.infoWith with String attrs must compile: {:?}",
        built.err()
    );
}

/// SEAL-PRESERVING negative gate: `toString` on a FUNCTION is rejected at skyc
/// type-check (the Stringify obligation's `Fun` head-rejection), NOT deferred to
/// a cargo failure. A pure compile — no `IPE_E2E` needed.
#[test]
fn tostring_on_function_is_rejected_at_typecheck() {
    let root = repo_root();
    let entry = golden_dir(&root, "m_tostring_fn_rejected").join("Main.sky");
    let out = std::env::temp_dir().join("skyc_m_tostring_fn_rejected_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve");
    let Ok(runtime) = runtime else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_err(),
        "toString on a function MUST fail at skyc type-check (Stringify obligation), \
         not exit 0 and defer to cargo",
    );
}
