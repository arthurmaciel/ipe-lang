//! Seal — let-fn captured as a callback lambda (callee position) and
//! let-fn forwarded to a kernel in a polymorphic context.
//!
//! **c01**: A let-fn `process : Int -> Int -> Int` is `NonClone` in the
//! lowerer.  A sibling let-fn `applyInner` captures `process` from the outer
//! scope and passes it as a callback `\m -> process n m` to `List.map`
//! (Apply.args position).
//!
//! c01: propagating `noncl_set = {process}` into the lambda argument at depth+1
//! would make the callee-position exemption (`depth == 0`) miss at depth 1 →
//! spurious SKY-L0126.
//!
//! So the `Apply` arm of `rewrite_captured_clones` clears `noncl_set`
//! for any `Expr::Lambda` in argument position before recursing.  Lambdas in
//! argument position are callbacks already fully validated by their own
//! `lower_lambda` pass at depth 0.  Lambdas in func position (immediately-
//! invoked: `(\x -> f x) p`, i130 c14 gate) still propagate `noncl_set`
//! normally so the outer-FnOnce-vs-Box<dyn Fn> check is preserved.
//!
//! **c02**: A let-fn `report` is forwarded to `Task.onError` inside a
//! polymorphic helper `wrap : String -> Task Error a -> Task Error a`.
//!
//! c02: classifying `Var(report)` in the partial application `Task.onError
//! report` as slot-class `None` (because `ir_type_from_ty` fails resolving
//! `Error -> Task Error a` while `a` is still a free `Ty::Var`) would let T7
//! conservatism turn `None` into SKY-L0126.
//!
//! So a `Ty::Fun` slot whose type resolution fails only due to a nested
//! type variable maps to `Some(NonClone)` instead of `None`.  Forwarding the
//! Var into an `impl FnOnce` slot is a plain ownership transfer — correct.
//!
//! ```text
//! # gate check always (no SKY_E2E needed):
//! cargo test -p skyc --test golden_i151_nested_let_fn
//!
//! # full E2E (skyc build + cargo build + run):
//! SKY_E2E=1 cargo test -p skyc --test golden_i151_nested_let_fn
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

// ── c01 — let-fn in callee position inside nested lambda (green after fix) ───

/// `applyInner n = List.map (\m -> process n m) [1,2,3]` — `process` is
/// `NonClone` in callee position inside the inner lambda.  Without the fix, `SKY-L0126`.
/// Post-fix: skyc build succeeds; cargo build + run produce "11, 12, 13".
#[test]
fn c01_nested_let_fn_callee_green() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i151_nested_let_fn_callee")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i151_nested_let_fn_callee");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i151_nested_let_fn_callee (was SKY-L0126 pre-fix): {:?}",
        built.err()
    );

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("i151_nested_let_fn_callee", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "must exit 0; got:\n{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("11, 12, 13"),
        "applyInner 10 over [1,2,3] must print '11, 12, 13'; got:\n{}",
        outcome.stdout
    );
}

// ── c02 — let-fn forwarded to Task.onError inside a polymorphic function ─────

/// `wrap : String -> Task Error a -> Task Error a` defines a let-fn `report`
/// and forwards it to `Task.onError`.  The polymorphic `a` caused two failures:
///
/// 1. T7b (`slot_classes`): `ir_type_from_ty(Error -> Task Error a)` failed on the
///    nested `Ty::Var("a")` → `slot_class = None` → SKY-L0126.  Fixed by mapping
///    `Ty::Fun` failed slots to `Some(NonClone)`.
///
/// 2. T8 (ret type): after T7b cleared the slot gate, `ir_type_from_ty(Task a)` on
///    the eta-lambda return type (line 6048 of `eta_expand_partial`) still failed
///    because `Task a` contains the free `Ty::Var("a")` →
///    `Unsupported(Polymorphism)`.  Fixed by switching line 6048 to
///    `ir_type_from_ty_json` — consistent with the eta-params at line 6044 which
///    already used `ir_type_from_ty_json`.  Free `Ty::Var` in the return-type slot
///    maps to `IrType::Json` (a sound stand-in, since the kernel signature at the
///    call site unifies the concrete type).
///
/// Post-fix: skyc build succeeds.
#[test]
fn c02_poly_fn_on_error_green() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i151_poly_task_on_error")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i151_poly_task_on_error");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i151_poly_task_on_error (was SKY-L0126 pre-fix): {:?}",
        built.err()
    );
}
