#![forbid(unsafe_code)]
//! `skyc` binary entry point — a thin wrapper over the [`skyc`] driver library.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match skyc::run_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("skyc: {err}");
            ExitCode::FAILURE
        }
    }
}
