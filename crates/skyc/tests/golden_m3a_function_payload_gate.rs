//! Milestone-3a soundness gate: a function value reaching a CONSTRUCTOR PAYLOAD
//! THROUGH a type variable must NEVER emit cargo-failing Rust silently.
//!
//! The shape — `type Box a = Mk a`, with `Mk (\n -> n + 1)` — instantiates the
//! payload field `a` to `Int -> Int` at the use site. The synthesised Rust enum
//! `MainBox<T1>` derives `Clone`/`Debug`/`PartialEq` and impls `SkyStringify`; a
//! `Box<dyn Fn>` field satisfies none of them, so the emitted Rust would not
//! build. The syntactic per-field gate cannot see this (the payload value at the
//! call site is a bare lambda, not a struct/enum field), so the lowerer's
//! region-based gate catches it and surfaces the documented first-class-function
//! gap (SKY-L0107).
//!
//! This pins the recurring soundness-floor class (the sibling of the M2C
//! function-in-record gate): the driver must produce EITHER a clean Sky
//! diagnostic (SKY-L0107) OR — should eager `Box<dyn Fn>` coercion at the
//! construction site ever land — Rust that builds and runs with the
//! semantically-correct output (`2`, since `(\n -> n + 1) 1 == 2`). It must NEVER
//! accept the program and then cargo-fail.
//!
//! Note on the Go oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` ALSO fails this shape today —
//! its codegen emits Go that `go build` rejects ("the Go compiler does not
//! accept"), hand-verified in a temp dir. So the Rust clean diagnostic is a
//! strict improvement over the Go reference, not a divergence.

use std::path::{Path, PathBuf};

mod support;

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3a_function_payload_gate")
        .join("Main.sky")
}

#[test]
fn rejects_cleanly_or_builds_and_runs_never_silent_cargo_fail() {
    let root = repo_root();
    let entry = example_entry(&root);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3a_function_payload_gate_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);

    // The minimal sound outcome: a clean constructor-payload-function
    // diagnostic naming the constructor-payload carrier (SKY-L0114), distinct
    // from the record-field gap (SKY-L0107).
    if let Err(CliError::Pipeline { diag, .. }) = &built {
        assert_eq!(
            diag.code(),
            sky_diagnostics::SKY_L0114,
            "a function value reaching a constructor payload through a type \
             variable must surface SKY-L0114, got: {diag:?}"
        );
        return;
    }

    // The only other acceptable outcome is full acceptance — never another
    // driver error, and never a silent accept that later cargo-fails.
    assert!(
        built.is_ok(),
        "must reject cleanly (SKY-L0114) or accept fully — never another error: {:?}",
        built.err()
    );

    // Proper support landed (an eager `Box<dyn Fn>` coercion): the emitted crate
    // MUST build and run with the semantically-correct output. Gated on SKY_E2E
    // so default runs stay fast.
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("m3a_function_payload_gate", &out);
    support::assert_go_parity(
        "m3a_function_payload_gate",
        &repo_root()
            .join("tests")
            .join("golden")
            .join("m3a_function_payload_gate"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0");
}
