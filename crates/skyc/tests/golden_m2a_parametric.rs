//! Milestone-2a parametric-polymorphism gate: `skyc` must emit `main.rs`
//! byte-identical to the checked-in golden for fully-parametric top-level
//! functions (type variables used *structurally* — pure pass-through), and
//! (behind `SKY_E2E=1`) the emitted project must build and print `42`.
//!
//! The program exercises the three M2a shapes:
//!
//! ```text
//! identity : a -> a            -- one var, returned          → fn ..<T1>(x: T1) -> T1
//! const    : a -> b -> a       -- two vars, first returned    → fn ..<T1, T2>(x: T1, y: T2) -> T1
//! apply    : (a -> b) -> a -> b-- higher-order parametric     → fn ..<T1, T2>(f: Box<dyn Fn(T1) -> T2>, x: T1) -> T2
//! ```
//!
//! Each `a` lowers to `Generic(a)` (Rust `T1`) by quantification position, and
//! `identity` / `const` are each used at two distinct concrete types in the same
//! `main` — the ONE generic function, monomorphised by Rust at every call site.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `42\n`, exit 0 — hand-verified in a temp dir, where the
//! Go backend emits the matching monomorphisation
//! `func identity[T1 any](x T1) T1` / `func const_[T1 any, T2 any](x T1, y T2) T1`
//! / `func apply[T1 any, T2 any](f func(T1) T2, x T1) T2`, confirming the
//! `a` → `T1` naming convention and the `func(T1) T2` ↔ `Box<dyn Fn(T1) -> T2>`
//! correspondence. Running the Go toolchain inside `cargo test` is impractical
//! (it needs the Haskell `sky` binary plus a Go toolchain), so the hand-computed
//! `42` is the in-test oracle, documented here against the Go-equivalent command.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The `sky-rust` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m2a_parametric")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m2a_parametric")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m2a_parametric_emit");
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
/// parametric program prints `42` — the same value the Go backend produces.
/// Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_forty_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m2a_parametric_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&out)
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "emitted project must build: {status:?}"
    );

    let bin = out.join("target").join("debug").join("sky-app");
    let output = Command::new(&bin).output();
    assert!(
        output.is_ok(),
        "emitted binary must run: {:?}",
        output.as_ref().err()
    );
    let Ok(output) = output else { return };
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "42\n",
        "program prints 42 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
}
