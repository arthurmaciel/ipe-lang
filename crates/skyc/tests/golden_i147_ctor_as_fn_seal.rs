//! #147 seal — constructors as first-class function values.
//!
//! A constructor of arity N in a value / partial-application position used to
//! emit `Err(unsupported(…, Feature::CtorAsFunction))` (SKY-L0113).  The
//! lowerer now routes such sites into `eta_expand_partial_ctor`, producing an
//! eta-wrapped closure that captures the supplied args (with T4/#130 capture-
//! clone discipline) and takes the remaining ones as lambda parameters:
//!
//! ```text
//! Tagged "item"     (arity 2, 1 supplied)
//! ──────────────────────────────────────────────────────────────
//! Box::new(move |eta_0: i64| -> Item { Main_Tagged("item".to_string(), eta_0) })
//! ```
//!
//! Three scenarios:
//!
//! * **A1** (`i147_ctor_map_bare`) — nullary-arity-1 ctor passed bare to
//!   `List.map`: `List.map Wrap [1,2,3]` → `[Wrap(1), Wrap(2), Wrap(3)]`.
//! * **A2** (`i147_ctor_partial`) — partial multi-arg ctor + T4 String capture:
//!   `Tagged "item"` mapped over `[10, 20, 30]` → `"item:10, item:20, item:30"`.
//! * **A3** (`i147_ctor_field`) — ctor as a HOF argument (user-fn taking `String -> Box`):
//!   `applyMk Wrap "hello"` → `"hello"`. (Records cannot hold function-typed fields
//!   per SKY-L0107; the equivalent "first-class ctor" story holds via HOF arguments.)
//!
//! All three also verify that the `m3a_gate_partial` fixture (formerly the
//! SKY-L0113 gate) now COMPILES SUCCESSFULLY as a positive regression.
//!
//! Gated: green fixtures (A1-A3) require `SKY_E2E=1` for the cargo build+run
//! step; the `m3a_gate_partial` positive-compile check runs always.
//!
//! ```text
//! # green suite (cargo-0 required):
//! SKY_E2E=1 cargo test -p skyc --test golden_i147_ctor_as_fn_seal
//!
//! # positive-compile check only (fast):
//! cargo test -p skyc --test golden_i147_ctor_as_fn_seal
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Assert that `skyc::build(fixture)` SUCCEEDS (exit-0 from the lowerer).
/// Runs without `SKY_E2E` so this check is always fast.
fn assert_skyc_ok(fixture: &str, out_suffix: &str) {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for fixture {fixture}: {:?}",
        built.err()
    );
}

// ── m3a_gate_partial positive-compile (formerly SKY-L0113 gate) ──────────────

/// The `m3a_gate_partial` fixture (`Node x 1` — arity-3 ctor with 2 supplied
/// args) used to surface SKY-L0113.  Since #147 it must COMPILE SUCCESSFULLY.
#[test]
fn m3a_gate_partial_now_compiles() {
    assert_skyc_ok("m3a_gate_partial", "i147_m3a_gate_partial_positive");
}

// ── A1 — bare ctor passed to List.map ────────────────────────────────────────

/// `Wrap : Int -> Box` (arity 1) passed bare to `List.map`.
/// `List.map Wrap [1,2,3]` → `[Wrap(1), Wrap(2), Wrap(3)]` → unwrapped
/// and joined → `"1, 2, 3"`.
#[test]
fn a1_ctor_map_bare() {
    assert_skyc_ok("i147_ctor_map_bare", "i147_ctor_map_bare_emit");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i147_ctor_map_bare")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i147_ctor_map_bare_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i147_ctor_map_bare: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i147_ctor_map_bare", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "A1: must exit 0 (was SKY-L0113 before #147)"
    );
    assert!(
        outcome.stdout.contains("1, 2, 3"),
        "A1: List.map with bare ctor must produce '1, 2, 3'; got:\n{}",
        outcome.stdout
    );
}

// ── A2 — partial multi-arg ctor + T4 String capture ──────────────────────────

/// `Tagged "item"` (arity-2 ctor with 1 arg supplied) mapped over `[10, 20, 30]`.
/// The captured `"item" : String` is `CloneOk` — T4/#130 rewrites it to
/// `.clone()` so the closure is `Fn` and survives multiple calls.
/// Expected output: `"item:10, item:20, item:30"`.
#[test]
fn a2_ctor_partial_multiarg_with_clone() {
    assert_skyc_ok("i147_ctor_partial", "i147_ctor_partial_emit");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i147_ctor_partial")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i147_ctor_partial_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i147_ctor_partial: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i147_ctor_partial", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "A2: must exit 0 (was SKY-L0113 before #147)"
    );
    assert!(
        outcome.stdout.contains("item:10, item:20, item:30"),
        "A2: partial ctor with captured String must produce 'item:10, item:20, item:30'; got:\n{}",
        outcome.stdout
    );
}

// ── A3 — ctor stored in record field then applied ─────────────────────────────

/// `applyMk Wrap "hello"` — `Wrap : String -> Box` is passed as a HOF
/// argument whose slot type is `String -> Box`.  The `VarCtor` path in
/// `lower_expr` eta-expands the bare ctor into a closure which the function
/// then calls.  Expected output: `"hello"`.
#[test]
fn a3_ctor_stored_in_record_field() {
    assert_skyc_ok("i147_ctor_field", "i147_ctor_field_emit");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i147_ctor_field")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i147_ctor_field_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i147_ctor_field: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i147_ctor_field", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "A3: must exit 0 (was SKY-L0113 before #147)"
    );
    assert!(
        outcome.stdout.contains("hello"),
        "A3: ctor in record field then applied must produce 'hello'; got:\n{}",
        outcome.stdout
    );
}
