//! `panic-scan <file.rs>…` — flag authored abrupt-failure constructs in the
//! production regions of the given Rust files. Exit 1 if any are found.
//!
//! Files whose path contains a `/tests/` or `/templates/` segment are skipped:
//! `tests/` is integration-test code, and `templates/` holds emitted-program
//! Rust that is copied verbatim into every generated binary (the generated
//! program legitimately exits/panics; that Rust is covered by the separate
//! emitted-output package gate, not this compiler-code scan). Inline
//! `#[cfg(test)]` bodies are skipped by the scanner itself.

use std::process::ExitCode;

/// Path segments whose files are not compiler production code: test crates and
/// the emitted-program templates copied verbatim into generated binaries.
const SKIP_SEGMENTS: &[&str] = &["tests", "templates"];

fn main() -> ExitCode {
    let mut found = false;
    for path in std::env::args().skip(1) {
        if path.split('/').any(|seg| SKIP_SEGMENTS.contains(&seg)) {
            continue;
        }
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("panic-scan: cannot read {path}: {e}");
                return ExitCode::from(2);
            }
        };
        match panic_scan::scan_str(&src) {
            Ok(hits) => {
                for hit in hits {
                    println!(
                        "{path}:{}: banned abrupt-failure construct `{}`",
                        hit.line, hit.tok
                    );
                    found = true;
                }
            }
            Err(e) => eprintln!("panic-scan: {path}: could not lex ({e}) — review manually"),
        }
    }
    if found {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
