//! Go-oracle parity check for `Cli.program`'s view printer. A
//! `view` that doesn't append its own trailing newline gets renders glued
//! together with NOTHING in between, and exactly ONE trailing newline after
//! the event loop exits. This matches `runtime-go/rt/cli.go`'s
//! `cliPrintView` contract byte-for-byte: it "writes the result to stdout
//! WITHOUT a trailing newline (the user's prompt formatting decides whether
//! to add one)".
//!
//! Forcing a newline after every render would break `examples/sky/ipe/20-cli-counter`'s
//! REPL-prompt UX (`view` returns `"count=... > "` with no trailing newline so
//! the cursor stays on the prompt line) and diverge from the Go reference.
//! This test asserts the CORRECT (Go-parity) glued-together behavior so
//! a future "fix" doesn't reintroduce that divergence.
//!
//! Gated on `IPE_E2E=1`. Run:
//!
//! ```text
//! IPE_E2E=1 cargo test -p ipe --test golden_i122_cli_program_separator
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn cli_program_glues_consecutive_renders_matching_go_oracle() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let dir = root
        .join("tests")
        .join("golden")
        .join("cli_program_view_separator");
    let entry = dir.join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_i122_cli_program_view_separator_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "ipe build must succeed: {:?}", built.err());

    // Two stdin lines → two loop-body renders (count 0 → 1 → 2), then EOF.
    let outcome =
        support::build_and_run_emitted_with_stdin("cli_program_view_separator", &out, b"a\nb\n");

    assert_eq!(outcome.exit_code, Some(0));
    // Go-parity: renders glue together (view supplies no separator of its
    // own), with exactly ONE trailing newline after the loop exits.
    let expected = "lines: 0lines: 1lines: 2\n";
    assert_eq!(
        outcome.stdout, expected,
        "Cli.program must match Go's cliPrintView contract (no per-render \
         newline, one trailing newline at exit), got: {:?}",
        outcome.stdout
    );
}
