//! Soundness gate: a function value reaching a record field
//! THROUGH a type variable must NEVER emit cargo-failing Rust silently.
//!
//! The shape — `wrap : a -> { value : a }` applied as `wrap (\n -> n + 1)` —
//! instantiates the field `value : a` to `Int -> Int` at the use site. The
//! synthesised struct `RecValue<T1>` derives `Clone`/`Debug`/`PartialEq` and
//! impls `IpeStringify`; a `Box<dyn Fn>` field satisfies none of them, so the
//! emitted Rust does not build. The syntactic per-field gate
//! (`reject_function_valued_field`) cannot see this — the field value at the call
//! site is not syntactically a function — so a region-based gate in the lowerer
//! catches it and surfaces the documented first-class-function gap (IPE-L0107).
//!
//! This test pins the regression: the driver must produce EITHER a clean Ipê
//! diagnostic (IPE-L0107) OR — should proper support land later (an eager
//! `Box<dyn Fn>` coercion at the construction site) — Rust that builds and runs
//! with the semantically-correct output (`42`, since
//! `unwrap (wrap (\n -> n + 1))` is `\n -> n + 1` and `f 41 == 42`). It must
//! NEVER accept the program and then cargo-fail.
//!
//! Note on the golden oracle: the the reference compiler at
//! `/home/arthur/Documentos/comp/ipe/out/ipe` ALSO fails this shape today —
//! its codegen emitted code that `go build` rejects (E5001 "Ipê compiler bug",
//! `cannot call f ... any is not a function`), hand-verified in a temp dir. So
//! the Rust clean diagnostic is a strict improvement over the golden reference, not
//! a divergence: `42` is the value the language *semantics* specify, which the
//! `Ok` branch below asserts should proper support ever land.

use std::path::{Path, PathBuf};

use ipe::CliError;

/// The `ipe-lang` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("function_field_gate")
        .join("Main.ipe")
}

#[test]
fn rejects_cleanly_or_builds_and_runs_never_silent_cargo_fail() {
    let root = repo_root();
    let entry = example_entry(&root);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m2c_function_field_gate_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = ipe::build(&entry, &out, &runtime);

    // The minimal sound outcome: a clean first-class-function diagnostic.
    if let Err(CliError::Pipeline { diag, .. }) = &built {
        assert_eq!(
            diag.code(),
            ipe_diagnostics::IPE_L0107,
            "a function value reaching a record field through a type variable \
             must surface IPE-L0107, got: {diag:?}"
        );
        return;
    }

    // The only other acceptable outcome is full acceptance — never another
    // driver error, and never a silent accept that later cargo-fails.
    assert!(
        built.is_ok(),
        "must reject cleanly (IPE-L0107) or accept fully — never another error: {:?}",
        built.err()
    );

    // With proper support (an eager `Box<dyn Fn>` coercion), the emitted crate
    // MUST build and run with the semantically-correct output. Gated on IPE_E2E
    // so default runs stay fast.
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let outcome = crate::support::build_and_run_emitted("function_field_gate", &out);
    crate::support::assert_go_parity(
        "function_field_gate",
        &repo_root()
            .join("tests")
            .join("golden")
            .join("function_field_gate"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0");
}
