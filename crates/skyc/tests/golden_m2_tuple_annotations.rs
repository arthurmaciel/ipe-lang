//! Milestone-2 tuple-type-annotation gate: `skyc` must emit `main.rs`
//! byte-identical to the checked-in golden for a program whose top-level
//! bindings carry **tuple-type annotations** (`firstOf : (Int, Int) -> Int`),
//! and (behind `SKY_E2E=1`) the emitted project must build and print `48`.
//!
//! This is the parity artefact for the M2B tuple-annotation feature: before it,
//! a `(T1, T2)` in a type annotation failed at parse with SKY-P0050 ("unclosed
//! delimiter"). The annotation now parses to `TypeAnnotation::TTuple`,
//! canonicalises to `canon::Type::Tuple`, flows through the solver's existing
//! `Ty::Tuple` structure, and lowers to `IrType::Tuple` — so a tuple-typed
//! parameter emits as a Rust `(i64, i64)` slot. This is what unblocks the
//! `fst : (a, b) -> a` / `snd : (a, b) -> b` signatures.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `48\n`, exit 0 — verified by hand in a temp dir (so the
//! Go build artifacts never touch the reference tree):
//!
//! ```text
//! $ cd "$(mktemp -d)" && sky run Main.sky   # Go backend
//! 48
//! ```
//!
//! `firstOf (41, 7) = 41` and `secondOf (41, 7) = 7` (each returns the matching
//! element when the tuple equals `(41, 7)`), so the entry prints `41 + 7 = 48`.
//! The `end_to_end_*` test below asserts the Rust backend reaches the identical
//! `48`. Running the Go toolchain inside `cargo test` is impractical (it needs
//! the Haskell `sky` binary plus a Go toolchain), so the hand-verified value is
//! the in-test oracle, documented here against the Go-equivalent command.
//!
//! (Bool literals `True`/`False` are not yet in the Rust frontend's prelude, so
//! the example uses `(Int, Int)` tuples rather than the `(41, True)` shape — the
//! observable element-projection behaviour, and the tuple-type annotation under
//! test, are identical.)

use std::path::{Path, PathBuf};

mod support;

/// The `sky-rust` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m2_tuple_annotations")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m2_tuple_annotations")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m2_tuple_annotations_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"));
    let want = std::fs::read_to_string(&golden);
    assert!(emitted.is_ok() && want.is_ok(), "both files must read");
    assert_eq!(
        emitted.ok(),
        want.ok(),
        "emitted main.rs must equal the golden byte-for-byte"
    );
}

/// Full spine: compile, build the emitted Cargo project, run it, and assert the
/// tuple-annotation program prints `48` — the same value the Go backend
/// produces. Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_forty_eight() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m2_tuple_annotations_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m2_tuple_annotations", &out);
    support::assert_go_parity(
        "m2_tuple_annotations",
        &repo_root()
            .join("tests")
            .join("golden")
            .join("m2_tuple_annotations"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}
