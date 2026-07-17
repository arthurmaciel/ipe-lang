//! `toString` on a wildcard `any` param.
//!
//! Without the fix, `skyc build` exits 0, but the emitted Rust fails `cargo build`
//! with E0277 (`the trait bound T1: std::fmt::Display is not satisfied`) at the
//! `basics_to_string(x)` call inside a function whose only generic is the
//! wildcard `any` PARAM (`render : any -> String; render x = toString x`).
//!
//! Root cause: the runtime's stringifier is generic
//! `basics_to_string<T: std::fmt::Display>(v: T)`, but the emitted enclosing
//! function carried an unbounded `<T1: Clone>` param, so its body's
//! `basics_to_string(x)` could not prove `x: Display`.
//!
//! Fix (`crates/ipe_ir/src/ir.rs` + `crates/ipe_lower/src/lower.rs`): a new
//! `BoundSet::DISPLAY` flag, rendered by `render_bounds` as `std::fmt::Display`.
//! The bound is decided STRUCTURALLY, at IR level, by the GENERAL kernel->bound
//! map (`apply_kernel_type_param_bounds` / `body_calls_kernel_on_param`) that
//! generalises the `Db.get*`->`SkyRow` machinery: it fires ONLY when the fn
//! body contains an actual `Basics.toString` KERNEL application whose sole
//! argument (arg 0) is a `Var`/`CloneVar` reference to the param. Unlike the
//! wildcard-only `SkyRow`, `Display` applies to wildcard `any` AND named tvars
//! alike — `toString` is legitimate on any polymorphic value, and `T: Display`
//! is satisfiable by every scalar caller (Int/Float/Bool/String).
//!
//! Run:
//! ```text
//! IPE_E2E=1 cargo test -p skyc --test golden_i186_display_bound
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
        .join("display_bound")
        .join("Main.sky")
}

/// skyc-0: the compiler must accept the program AND emit `+ std::fmt::Display`
/// on the wildcard-`any` renderer function's own generic — checked
/// unconditionally (cheap, no `cargo`), independent of the `IPE_E2E` gate.
#[test]
fn i186_skyc_accepts_and_bounds_fn_display() {
    let root = repo_root();
    let entry = entry_path(&root);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i186_display_bound_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP display_bound: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for display_bound: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    // The renderer function's wildcard-`any` generic gains the `Display` bound so
    // its `basics_to_string(x)` body type-checks (the E0277 half).
    assert!(
        emitted.contains("std::fmt::Display"),
        "the wildcard-`any` renderer function must carry the `std::fmt::Display` \
         bound so its `basics_to_string` body type-checks (#186); got main.rs:\n{emitted}"
    );
    assert!(
        emitted.contains("pub fn main_render<T1: Clone + std::fmt::Display>"),
        "the bound belongs on the renderer FUNCTION's generic param (#186); got \
         main.rs:\n{emitted}"
    );
}

/// cargo-0 ∧ run-0: the emitted project actually compiles with `rustc` and
/// prints the rendered value. Gated on `IPE_E2E=1` — the only check that would
/// have caught the original SEAL violation (E0277, `skyc build` clean).
#[test]
fn i186_cargo_builds_and_runs() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = entry_path(&root);
    let out = std::env::temp_dir().join("skyc_i186_display_bound_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for display_bound: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("display_bound", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "display_bound binary must exit 0 (no E0277); got {:?} (stdout: {:?})",
        outcome.exit_code,
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim(),
        "value=42",
        "must print the `toString`-rendered value through the wildcard-`any` \
         renderer; got: {:?}",
        outcome.stdout
    );
}
