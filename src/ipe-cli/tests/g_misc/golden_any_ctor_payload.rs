//! IPE-L0102 regression — `any` as a union-constructor payload field.
//!
//! ## Root cause
//!
//! `type Msg = … | CartTopicReceived any` has `any` as a **union-ctor payload
//! field**.  If `lower_enum` Gate 1 collected the `any` type variable via
//! `collect_type_vars`, found it absent from `union.vars` (monomorphic `Msg`
//! has no type params), and raised `Feature::Polymorphism` (IPE-L0102), it
//! would block examples 27 (`MessageReceived any`) and 37
//! (`CartTopicReceived any`).
//!
//! ## Fix
//!
//! * `constrain.rs` — `pin_any_in_ty` substitutes `any` `Ty::Var`s in ctor
//!   schemes with `Dict String String` (the pub/sub wire carrier) so every
//!   instantiation site sees a concrete type.
//! * `lower.rs` `lower_enum` Gate 1 — excludes `any` from the unbound-var
//!   check (mirrors reference `(/= "any") freeVars` filter).
//! * `lower.rs` `ir_type_from_canon` — maps an `any` Var to
//!   `IrType::Dict(Str, Str)` before the generic-var check.
//! * `ipe/src/lib.rs` `source_for_span` — adds union ctor spans to the
//!   attribution heuristic so ctor-based lower errors (IPE-L0114, etc.) render
//!   against the owning dep file, not the entry file.
//!
//! ## Tests
//!
//! 1. `any_ctor_payload_ipe_and_cargo_zero` — build-only seal: `BroadcastMsg
//!    any` compiles (ipe exit 0) and the emitted Rust builds (cargo exit 0).
//!    Without the fix, IPE-L0102.
//!
//! 2. `any_ctor_payload_fail_closed` — using the `any` payload as a `String`
//!    must surface `IPE-T0001` at ipe, never silently cargo-fail.
//!
//! 3. `ctor_span_attr_dep_module` — a function-bearing ctor in a dep module
//!    must surface `IPE-L0114` attributed to `Dep.ipe`, not `Main.ipe`.

use std::path::{Path, PathBuf};

use ipe::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn fixture_entry(root: &Path, name: &str) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(name)
        .join("Main.ipe")
}

fn fixture_src_entry(root: &Path, name: &str) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(name)
        .join("src")
        .join("Main.ipe")
}

/// Build-only seal: `BroadcastMsg any` — ipe exit 0 AND cargo build green.
/// Without the fix, IPE-L0102 (`Feature::Polymorphism` in `lower_enum` Gate 1).
#[test]
fn any_ctor_payload_ipec_and_cargo_zero() {
    let root = repo_root();
    let entry = fixture_entry(&root, "any_ctor_payload");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("l0102_any_ctor_payload_emit");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };

    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipec must accept `BroadcastMsg any` (was IPE-L0102 pre-fix): {:?}",
        built.err()
    );

    // Cargo build seal: the emitted Rust must compile.
    // Gated on IPE_E2E so the default test run stays fast.
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let outcome = crate::support::build_and_run_emitted("any_ctor_payload", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "binary must exit 0; stdout:\n{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("ignored"),
        "handleMsg Ignored must print 'ignored'; got:\n{}",
        outcome.stdout
    );
}

/// Fail-closed guard: using the `any`-ctor payload as a String must produce
/// IPE-T0001 at ipe time, never silently cargo-fail.
#[test]
fn any_ctor_payload_fail_closed() {
    let root = repo_root();
    let entry = fixture_entry(&root, "any_ctor_fail_closed");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("l0102_any_ctor_fail_closed_emit");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };

    let built = ipe::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(ipe_diagnostics::IPE_T0001),
        "using an `any`-ctor payload as String must surface IPE-T0001, got: {built:?}"
    );
}

/// Misattribution seal: a function-bearing ctor in a dep module must surface
/// IPE-L0114 attributed to `Dep.ipe`, not the entry `Main.ipe`.
#[test]
fn ctor_span_attr_dep_module() {
    let root = repo_root();
    let entry = fixture_src_entry(&root, "ctor_span_attr");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("l0102_ctor_span_attr_emit");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };

    let result = ipe::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        result.is_err(),
        "expected IPE-L0114 for function-bearing ctor in dep — success means the seal is broken"
    );
    let err_str = result.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        err_str.contains("IPE-L0114"),
        "expected IPE-L0114 for function-bearing ctor, got:\n{err_str}"
    );
    assert!(
        err_str.contains("Dep.ipe"),
        "IPE-L0114 must attribute to Dep.ipe (ctor-span attribution fix), got:\n{err_str}"
    );
    assert!(
        !err_str.contains("Main.ipe"),
        "IPE-L0114 must NOT mis-attribute to Main.ipe, got:\n{err_str}"
    );
}
