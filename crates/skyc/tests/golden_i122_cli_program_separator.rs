//! #122 regression — `Cli.program`'s view printer must separate consecutive
//! renders with a newline. Pre-fix, piping 2 lines through stdin produced
//! "lines: 0lines: 1lines: 2" (renders glued together); post-fix each
//! render lands on its own line.
//!
//! Gated on `SKY_E2E=1`. Run:
//!
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_i122_cli_program_separator
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn cli_program_separates_consecutive_renders() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let dir = root.join("tests").join("golden").join("i122_cli_program_view_separator");
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i122_cli_program_view_separator_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "skyc build must succeed: {:?}", built.err());

    // Two stdin lines → two loop-body renders (count 0 → 1 → 2), then EOF.
    let outcome =
        support::build_and_run_emitted_with_stdin("i122_cli_program_view_separator", &out, b"a\nb\n");

    assert_eq!(outcome.exit_code, Some(0));
    let expected = "lines: 0\nlines: 1\nlines: 2\n";
    assert_eq!(
        outcome.stdout, expected,
        "consecutive Cli.program renders must be newline-separated, got: {:?}",
        outcome.stdout
    );
}
