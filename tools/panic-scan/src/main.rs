//! `panic-scan <file.rs>…` — flag authored abrupt-failure constructs in the
//! production regions of the given Rust files. Exit 1 if any are found.
//!
//! Files whose path contains a `/tests/` segment are skipped (integration-test
//! crates); inline `#[cfg(test)]` bodies are skipped by the scanner itself.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut found = false;
    for path in std::env::args().skip(1) {
        if path.split('/').any(|seg| seg == "tests") {
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
