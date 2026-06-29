//! M2-lex `++` string-concat parity gate: the append operator `++` lexes to a
//! single [`sky_parse`] token (never two `+`), parses as a binary operator at the
//! reference precedence (level 5, right-associative — see
//! `/home/arthur/Documentos/comp/sky/src/Sky/Parse/Symbol.hs`), canonicalises to
//! the `append` kernel, types as `String -> String -> String`, and emits a Rust
//! `format!` concatenation that yields a fresh `String`.
//!
//! Two programs exercise the surface end to end:
//!
//! * `m2lex_append` — `greet name = "hi, " ++ name ++ "!"`, a mixed
//!   literal/variable chain, prints `hi, world!` at `greet "world"`.
//! * `m2lex_append_chain` — `"a" ++ "b" ++ "c" ++ "d"`, an all-literal chain
//!   that confirms right-associative nesting, prints `abcd`.
//!
//! Each emitted `main.rs` must be byte-identical to the checked-in golden, and
//! (behind `SKY_E2E=1`) the emitted project must build and print the value the
//! Go reference compiler produces.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` files to the same stdout:
//!
//! ```text
//! $ sky run Main.sky   # Go backend
//! hi, world!   # m2lex_append
//! abcd         # m2lex_append_chain
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Compile `tests/golden/<name>/Main.sky` and assert the emitted `src/main.rs`
/// equals the checked-in `tests/golden/<name>/main.rs` byte-for-byte.
fn assert_byte_identical(name: &str) {
    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.sky");
    let golden = dir.join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_emit"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"));
    let want = std::fs::read_to_string(&golden);
    assert!(emitted.is_ok() && want.is_ok(), "both files must read");
    assert_eq!(
        emitted.ok(),
        want.ok(),
        "emitted main.rs for {name} must equal the golden byte-for-byte"
    );
}

/// Full spine: compile, build the emitted Cargo project, run it, and assert it
/// prints `want` — the value the Go backend produces. Gated on `SKY_E2E=1` so
/// the default `cargo test` stays fast.
fn assert_runs_and_prints(name: &str, want: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = golden_dir(&root, name).join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&out)
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "emitted {name} project must build: {status:?}"
    );

    let bin = out.join("target").join("debug").join("sky-app");
    let output = Command::new(&bin).output();
    assert!(
        output.is_ok(),
        "emitted {name} binary must run: {:?}",
        output.as_ref().err()
    );
    let Ok(output) = output else { return };
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        want,
        "{name} prints the Go-backend value"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
}

#[test]
fn append_chain_emits_byte_identical_main_rs() {
    assert_byte_identical("m2lex_append");
}

#[test]
fn append_literal_chain_emits_byte_identical_main_rs() {
    assert_byte_identical("m2lex_append_chain");
}

#[test]
fn append_chain_builds_and_prints_greeting() {
    assert_runs_and_prints("m2lex_append", "hi, world!\n");
}

#[test]
fn append_literal_chain_builds_and_prints_abcd() {
    assert_runs_and_prints("m2lex_append_chain", "abcd\n");
}
