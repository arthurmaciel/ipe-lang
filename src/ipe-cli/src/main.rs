#![forbid(unsafe_code)]
//! `ipe` binary entry point — a thin wrapper over the [`ipe`] driver library.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match ipe::run_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        // An unknown command shows the full help screen on its own; the `ipe: `
        // prefix belongs to short diagnostics, not the sectioned help.
        Err(err @ ipe::CliError::UnknownCommand) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("ipe: {err}");
            ExitCode::FAILURE
        }
    }
}
