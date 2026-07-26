#![forbid(unsafe_code)]
//! `ipe` binary entry point — a thin wrapper over the [`ipe`] driver library.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match ipe::run_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        // An unknown command, or a per-command misuse, shows a full help page on
        // its own; the `ipe: ` prefix belongs to short diagnostics, not a help
        // screen. A command misuse leads with its own reason line before the page.
        Err(err @ (ipe::CliError::UnknownCommand { .. } | ipe::CliError::CommandUsage { .. })) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprint!(
                "{}",
                ipe::style::frame(&ipe::style::gutter(&format!("ipe: {err}")))
            );
            ExitCode::FAILURE
        }
    }
}
