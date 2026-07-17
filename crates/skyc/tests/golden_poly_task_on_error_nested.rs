//! Seal — E0308 pair. `examples/18-job-queue`'s `withErrorReporting :
//! String -> Task Error a -> Task Error a` defines internal error-handling
//! closures (`logAndFail`, `report`) whose bodies are MULTI-STEP pipelines
//! (`Crypto.randomToken 4 |> Task.andThen (\errId -> logAndFail e errId)`),
//! not a bare partial-application forward (that shape is the SEPARATE
//! `poly_task_on_error` c02 fixture, which goes through
//! `eta_expand_partial`'s slot-class path).  This fixture instead exercises
//! `lower_lambda`'s own return-type inference (`ir_type_from_ty_json`,
//! `lower.rs` around line 5537).
//!
//! Root cause: `ir_type_from_ty_json`'s `Ty::Var` arm mapped EVERY free
//! solver variable straight to `IrType::Json`, with no check against
//! `current_poly_tvars` — unlike the sibling helpers `ir_type_from_ty` and
//! `ir_type_from_ty_ui_msg`, which both check `current_poly_tvars` FIRST.  A
//! nested lambda's return-type slot inside a polymorphic `Def::Typed` body
//! solves to the SAME free var as the enclosing function's own quantified
//! `a` — so the unconditional Json fallback emitted a closure typed
//! `Fn(..) -> SkyTask<JsonVal>` at a call site expecting `SkyTask<T1>`: a
//! clean exit-0-then-cargo-fail (2x E0308).
//!
//! A second, independent layer of the SAME bug was found while chasing this:
//! `SolvedTypes::poly_var_map`'s "typed-rigids" entries are keyed by the BARE
//! union-find representative (a typed binding's own `params`/`ret` are read
//! from its annotation, never zonked) — but a `Ty::Var` read back from a
//! ZONKED region (`region_ty`, used by every nested-lambda return-type
//! lookup) is ALWAYS tagged via `sky_types::tag_solver_var`.  So even after
//! adding the `current_poly_tvars` check to `ir_type_from_ty_json`, the
//! lookup still silently missed for every TYPED enclosing binding (it only
//! matched for boundary-scheme-promoted UNTYPED bindings, whose
//! `poly_var_map` entries ARE tagged).  Fixed by a shared
//! `Lowerer::poly_tvar_symbol` helper that probes both the tagged and
//! untagged form before giving up — used by all three `Ty::Var`-vs-
//! `current_poly_tvars` call sites (`ir_type_from_ty`, `ir_type_from_ty_ui_msg`,
//! `ir_type_from_ty_json`) for consistency.
//!
//! Post-fix: skyc build succeeds; the emitted `main_with_error_reporting`
//! is generic over `T1` throughout (no `JsonVal`); cargo build + run confirm
//! BOTH the success path (untouched result) and Task.onError's fallback path
//! (original error is replaced by the "ref <token>" wrapper) work correctly
//! at runtime.
//!
//! ```text
//! # gate check always (no SKY_E2E needed):
//! cargo test -p skyc --test golden_i164_poly_task_on_error_nested
//!
//! # full E2E (skyc build + cargo build + run):
//! SKY_E2E=1 cargo test -p skyc --test golden_i164_poly_task_on_error_nested
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn poly_task_on_error_nested_green() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("poly_task_on_error_nested")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("poly_task_on_error_nested");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return; // runtime unavailable — skip silently
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for poly_task_on_error_nested: {:?}",
        built.err()
    );

    // Structural check independent of SKY_E2E: the emitted Rust must be
    // generic over the helper's own type param (T1), never `JsonVal`. This
    // is exactly the SEAL-violating shape this closes — assert it here so a
    // future regression fails even when SKY_E2E is not set (CI-cheap gate).
    let main_rs = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist after a successful skyc build");
    assert!(
        main_rs.contains("fn main_with_error_reporting<T1"),
        "withErrorReporting must lower to a generic Rust fn over T1; got:\n{main_rs}"
    );
    assert!(
        !main_rs.contains("SkyTask<JsonVal>"),
        "withErrorReporting's internal closures must stay typed SkyTask<T1>, \
         never fall back to SkyTask<JsonVal> (the #164 E0308 exit-0-then-cargo-fail \
         shape); got:\n{main_rs}"
    );

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("poly_task_on_error_nested", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "must exit 0; got:\n{}",
        outcome.stdout
    );
    // The success path passes "hello" through untouched; the failure path
    // replaces the original "boom" error with the "<opName> failed (ref
    // <4-char token>)" wrapper — proving Task.onError's fallback actually
    // fires with the right (generic, not JsonVal-erased) error type at
    // runtime. `Error.toString` on an `Error.unexpected` payload prefixes
    // "Unexpected: " (see `sky_runtime::error::SkyError::to_sky_string`) —
    // that prefix is genuine runtime behaviour, not part of this fixture's
    // own message text.
    assert!(
        outcome.stdout.contains("hello | Unexpected: op.fail failed (ref "),
        "expected the ok path to print 'hello' and the fail path to print \
         the wrapped 'Unexpected: op.fail failed (ref ...)' message; got:\n{}",
        outcome.stdout
    );
    assert!(
        !outcome.stdout.contains("boom"),
        "the original error message must be replaced by withErrorReporting's \
         wrapper, not leak through verbatim; got:\n{}",
        outcome.stdout
    );
}
