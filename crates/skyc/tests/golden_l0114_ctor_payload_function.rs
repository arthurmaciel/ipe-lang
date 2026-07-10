//! #90 (SKY-L0114 narrowing): `Ok f` / `Just f` holding a function used to
//! trip the ctor-payload-function gate unconditionally, making
//! `Result.andMap` / `Maybe.andMap` unusable. Stage 1 lifts the blanket
//! rejection for enum-like heads (`Maybe`/`Result`/user unions) — the
//! runtime/derive machinery already tolerates a function payload there — while
//! keeping two narrow fail-closed residuals: a CURRIED (arity >= 2) `andMap`
//! payload (needs Stage 2, nested-closure lowering), and REUSE of a
//! fn-carrying binding in more than one consuming position (SKY-L0127,
//! `Box<dyn Fn>` is not `Clone`). See
//! `docs/architecture/ctor-payload-function-design.md`.
//!
//! Green fixtures assert `skyc::build` succeeds by default (fast, no cargo);
//! under `SKY_E2E=1` they additionally cargo-build + run the emitted crate
//! and check stdout against the cached Go-oracle value. The two residual-gate
//! fixtures assert the exact diagnostic code always (no program output to
//! run).
//!
//! **Revert-incident regressions (BACKLOG #90, `f80f05a` reverted at
//! `dbd876b`).** The first #90 landing shipped both residual gates with two
//! real gaps, each independently reproduced via a built `skyc` binary + a
//! real `cargo build` of the emitted crate — not just inspection:
//!
//! 1. `reject_fn_value_reuse` (the SKY-L0127 reuse gate) was wired at the 4
//!    Def-param / let-binding / match-arm call sites but NOT at
//!    `lower_lambda`'s own `ir_params` — a fn-carrying LAMBDA parameter
//!    reused twice (`\mf -> consume mf + consume mf`) bypassed the gate and
//!    reached `cargo build` as E0382 (use of moved value). Closed by wiring
//!    `reject_fn_value_reuse` into `lower_lambda` too; see
//!    `l0127_lambda_param_reuse_gated` (must stay rejected) and its sound
//!    companion `l0127_lambda_param_call_twice_accepted` (proves the fix
//!    does not over-reject calling the same lambda param twice).
//! 2. `reject_curried_andmap_payload` (the SKY-L0114 curried-`andMap` gate)
//!    only recognised two syntactic callee shapes (a direct kernel var, or
//!    the pipe-desugared nested-call form) — an `andMap` partial application
//!    first bound to a `let` (`let g = Result.andMap (Ok 1) in g (Ok add3)`)
//!    matched neither shape and reached `cargo build` as E0277 (expected an
//!    `FnOnce` closure, found a multi-arg `dyn Fn`). Closed by moving the
//!    check to the KERNEL-CALL RESOLUTION boundary
//!    (`lower_call_uniform`'s `VarKernel | VarTopLevel` arm, gated on the
//!    resolved `Callee` + the callee's OWN solved type) instead of matching
//!    the call's AST shape, so no alias of the partial application can
//!    bypass it structurally; see `l0114_and_map_let_bound_alias_stays_gated`.

use std::path::{Path, PathBuf};

mod support;

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(name: &str) -> PathBuf {
    repo_root().join("tests").join("golden").join(name)
}

/// Build `name`; assert the pipeline ACCEPTS the program. Under `SKY_E2E=1`,
/// also cargo-build + run the emitted crate and check its stdout against the
/// cached Go-oracle value (`support::assert_go_parity`).
fn assert_green(name: &str) {
    let dir = golden_dir(name);
    let entry = dir.join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_emit"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently rather than fail
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "{name}: expected skyc to ACCEPT this program (#90 Stage 1 lift), got: {:?}",
        built.err()
    );

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted(name, &out);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "{name}: must exit 0");
}

/// Build `name`; assert the pipeline REJECTS it with exactly `expected`.
fn assert_gated(name: &str, expected: sky_diagnostics::Code) {
    let dir = golden_dir(name);
    let entry = dir.join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_emit"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "{name}: expected a clean {expected:?}, got build result {built:?}"
    );
}

/// `Ok (\x -> x + 1) |> Result.andMap (Ok 2)` — the runtime `SkyResult`'s
/// derives are generic-bounded, so `SkyResult<E, Box<dyn Fn>>` compiles.
#[test]
fn result_and_map_fn_payload_accepted() {
    assert_green("l0114_result_and_map_fn_payload");
}

/// `Just (\x -> x * 2) |> Maybe.andMap (Just 21)` — same shape for `Maybe`.
#[test]
fn maybe_and_map_fn_payload_accepted() {
    assert_green("l0114_maybe_and_map_fn_payload");
}

/// `type Retryish e = RetryAlways | RetryWhen (e -> Bool)` — a DECLARED
/// function-typed payload; #87's derive-demotion keeps the enum sound.
#[test]
fn ctor_decl_fn_payload_accepted() {
    assert_green("l0114_ctor_decl_fn_payload");
}

/// `case Just (\x -> x + 1) of Just f -> f 1 + f 2` — callee-position calls
/// never consume (`Fn::call` borrows), so calling twice is unlimited.
#[test]
fn fn_extracted_called_twice_accepted() {
    assert_green("l0114_fn_extracted_called_twice");
}

/// `Just (\a b -> a + b) |> Maybe.andMap (Just 1) |> Maybe.andMap (Just 2)` —
/// curried (arity >= 2) payload through `andMap` stays gated (Stage 2 territory).
#[test]
fn and_map_curried_payload_stays_gated() {
    assert_gated("l0114_and_map_curried_stays_gated", sky_diagnostics::SKY_L0114);
}

/// `let mf = Just (\x -> x + 1) in consume mf + consume mf` — a fn-carrying
/// binding used in two consuming (argument) positions has no sound rewrite.
#[test]
fn fn_carrier_reuse_stays_gated() {
    assert_gated("l0127_fn_carrier_reuse_gated", sky_diagnostics::SKY_L0127);
}

/// Revert-incident Bug 1 regression: `\mf -> consume mf + consume mf` — a
/// fn-carrying LAMBDA parameter (not a Def param / let-binding / match-arm
/// binding) reused in two consuming positions. Before the fix,
/// `lower_lambda` never ran its own `ir_params` through
/// `reject_fn_value_reuse`, so this reached `cargo build` as E0382 instead
/// of a clean SKY-L0127.
#[test]
fn lambda_param_reuse_stays_gated() {
    assert_gated("l0127_lambda_param_reuse_gated", sky_diagnostics::SKY_L0127);
}

/// Sound companion of `lambda_param_reuse_stays_gated`: `\f -> f 1 + f 2` —
/// a fn-carrying lambda parameter CALLED twice (callee position, never
/// consuming) must stay accepted. Proves the Bug 1 fix does not
/// over-reject.
#[test]
fn lambda_param_call_twice_accepted() {
    assert_green("l0127_lambda_param_call_twice_accepted");
}

/// Revert-incident Bug 2 regression:
/// `let g = Result.andMap (Ok 1) in g (Ok add3)` — `andMap`'s partial
/// application is bound to a `let` before being finished, splitting the
/// 2-arg call across a `let`-binding and a later plain-`Var` application.
/// The original AST-shape match recognised neither this call site's shape
/// nor any other alias of it; this reached `cargo build` as E0277 instead
/// of a clean SKY-L0114. The fix (checking at the kernel-call resolution
/// boundary, not the AST shape) closes this and every other alias
/// uniformly.
#[test]
fn and_map_let_bound_alias_stays_gated() {
    assert_gated(
        "l0114_and_map_let_bound_alias_stays_gated",
        sky_diagnostics::SKY_L0114,
    );
}
