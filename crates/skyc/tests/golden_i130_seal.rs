//! #130 seal — four residual `CopyLeaf` / depth / T4-hoist / T7-close holes from
//! the post-#121 gate (ipe-121-postmerge-seal-round.md).
//!
//! Fixes:
//!
//! * **Fix 1** — `clone_class_named_composite`: floors `CopyLeaf` → `CloneOk`
//!   for named composite types (`IrType::Record` / `IrType::Enum`).  Emitted
//!   Rust structs and enums derive `Clone` but NOT `Copy`, so `CopyLeaf` was
//!   an active wrong claim that produced E0525.
//! * **Fix 2** — `rewrite_captured_clones` depth guard: the `NonClone`
//!   callee-position exemption fires only at `depth == 0`.  At depth > 0 the
//!   symbol is consumed by an inner `move` closure → outer `FnOnce` → E0525.
//! * **Fix 3 T4** — `eta_expand_partial` complex-arg hoist: non-Var supplied
//!   args are lifted to `let __sky_cap_i = <expr>` OUTSIDE the lambda so the
//!   lambda captures the named binding (Clone-wrapped) rather than inlining
//!   the expression with its free vars captured bare.
//! * **Fix 4 T7** — `eta_expand_partial` fail-close: `ir_type_from_ty` → None
//!   now emits SKY-L0126 instead of silently passing a bare Var.
//!
//! Green fixtures (c01, c02, c13) require `SKY_E2E=1` for cargo build+run.
//! Gate fixture (c14) runs the diagnostic check always.
//!
//! ```text
//! # green suite:
//! SKY_E2E=1 cargo test -p skyc --test golden_i130_seal
//!
//! # gate check only (fast):
//! cargo test -p skyc --test golden_i130_seal
//! ```

use std::path::{Path, PathBuf};

use skyc::CliError;

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Assert that `skyc::build(fixture)` surfaces `expected` as a
/// `CliError::Pipeline` diagnostic.  Runs WITHOUT `SKY_E2E` so the gate
/// checks remain fast in the default CI pass.
fn assert_skyc_gate(fixture: &str, out_suffix: &str, expected: sky_diagnostics::Code) {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently rather than fail
    };
    let built = skyc::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "fixture {fixture}: expected skyc-fail {expected:?}, got build result {built:?}"
    );
}

// ── c01 — enum capture (Fix 1: CopyLeaf misclass for named types) ────────────

/// `List.map (\_ -> colorName color) [1,2,3]` — `color : Color` is an enum
/// with no payload fields.  Pre-fix: `clone_class(Enum{args:[]})` returned
/// `CopyLeaf` (composite over empty iterator); the bare capture made the
/// lambda `FnOnce` → E0525 on the second element.
/// Fix 1: `clone_class_named_composite` floors `CopyLeaf` → `CloneOk`;
/// the lambda emits `CloneVar(color)` → re-callable.
/// Expected output: "green,green,green".
#[test]
fn c01_enum_capture_fix1() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i130_enum_capture")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i130_enum_capture_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i130_enum_capture: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i130_enum_capture", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "must exit 0 (was E0525: enum CopyLeaf misclass)"
    );
    assert!(
        outcome.stdout.contains("green,green,green"),
        "List.map over enum capture must produce 'green,green,green'; got:\n{}",
        outcome.stdout
    );
}

// ── c02 — all-Int record capture (Fix 1: CopyLeaf misclass for named types) ──

/// `List.map (\dx -> translate origin dx) [1,2,3]` — `origin : Point` is an
/// all-Int record alias.  Pre-fix: `clone_class(Record{fields:[Int,Int]})`
/// returned `CopyLeaf` (all fields `CopyLeaf`); emitted Rust struct derives
/// `Clone` but NOT `Copy` → bare capture → `FnOnce` → `E0525` on second element.
/// Fix 1: `clone_class_named_composite` floors to `CloneOk` for named types.
/// Expected output: "1,5 2,5 3,5".
#[test]
fn c02_record_capture_fix1() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i130_record_capture")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i130_record_capture_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i130_record_capture: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i130_record_capture", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "must exit 0 (was E0525: all-Int record CopyLeaf misclass)"
    );
    assert!(
        outcome.stdout.contains("1,5 2,5 3,5"),
        "List.map over all-Int record capture must produce '1,5 2,5 3,5'; got:\n{}",
        outcome.stdout
    );
}

// ── c13 — complex-expr partial hoist (Fix 3 T4) ──────────────────────────────

/// `let f = mk (String.append base suffix) in f "!" ; f "?"` — the supplied
/// arg `String.append base suffix` is a complex expression (not a bare Var).
/// Pre-fix: the expr was inlined into the eta-lambda body; `base` and `suffix`
/// (both String, `CloneOk`) were captured bare → `FnOnce` → `E0525` on second call.
/// Fix 3 T4: hoist to `let __sky_cap_0 = <expr>` outside the lambda; lambda
/// captures `CloneVar(__sky_cap_0)` → re-callable.
/// Expected output: "hello! hello?".
#[test]
fn c13_complex_arg_hoist_t4() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i130_complex_arg_hoist")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i130_complex_arg_hoist_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i130_complex_arg_hoist: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i130_complex_arg_hoist", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "must exit 0 (was E0525: complex-expr arg inlined bare → FnOnce)"
    );
    assert!(
        outcome.stdout.contains("hello!"),
        "first call of partial must produce 'hello!'; got:\n{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("hello?"),
        "second call of partial must produce 'hello?' (re-callable); got:\n{}",
        outcome.stdout
    );
}

// ── c14 — nested-lambda NonClone callee gate (Fix 2) ─────────────────────────

/// `composed f = \p -> (\x -> f x) p` — `f : Int -> Int` (`NonClone`) is used
/// as the direct callee of `Apply` INSIDE a nested lambda (`\x -> f x`),
/// which is at depth 1 relative to the outer lambda `\p -> ...`.
///
/// Pre-fix: the callee-position exemption fired at any depth, so `Var(f)` was
/// allowed bare inside the inner lambda → inner `move` closure consumed `f`
/// from the outer env → outer `FnOnce` → E0525 at cargo.
///
/// Fix 2: exemption fires only at depth == 0; at depth > 0 → SKY-L0126.
#[test]
fn c14_nested_lambda_noncopy_gate() {
    assert_skyc_gate(
        "i130_nested_lambda_noncopy",
        "i130_nested_lambda_noncopy_gate",
        sky_diagnostics::SKY_L0126,
    );
}

// ── c05 — StreamWriter capture-forward (clone_class opaque audit) ────────────

/// `Stream.stream (\writer -> emitTick writer 1 |> Task.andThen (\_ ->
/// emitTick writer 2))` — the exact shape of examples/30-sse-server-demo.
///
/// Pre-fix: `clone_class(IrType::StreamWriter)` said `NonClone`, so
/// forwarding the captured handle as an argument (not calling it) tripped
/// SKY-L0126.  The runtime type is `#[derive(Clone, Copy)]`
/// (`server_stream.rs:38`) — the classification was an active wrong claim.
/// Same audit flipped ServerRequest/ServerResponse/ServerRoute/ServerCookie/
/// `HttpRequest` to `CloneOk` (all derive `Clone`).
///
/// skyc must exit 0.  The cargo/runtime layer is covered by the
/// 30-sse-server-demo sweep entry (a server fixture can't run-to-exit here).
#[test]
fn c05_streamwriter_capture_forward() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i130_streamwriter_capture")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i130_streamwriter_capture");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently rather than fail
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "StreamWriter capture-forward must pass skyc (was SKY-L0126): {:?}",
        built.err()
    );
}
