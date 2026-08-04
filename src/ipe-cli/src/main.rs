#![forbid(unsafe_code)]
//! `ipe` binary entry point — a thin wrapper over the [`ipe`] driver library.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match ipe::run_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        // These variants render their own complete screen — a full help page, a
        // gate report, or a self-guttered environment message — so the `ipe: `
        // prefix (which belongs to short one-line diagnostics) and the extra
        // gutter are not applied; they print as-is. A command misuse leads with
        // its own reason line before its help page.
        Err(
            err @ (ipe::CliError::UnknownCommand { .. }
            | ipe::CliError::CommandUsage { .. }
            | ipe::CliError::DocCoverage(_)
            | ipe::CliError::VerifyFailed { .. }
            | ipe::CliError::UpgradeNoPrebuilt { .. }
            | ipe::CliError::ToolchainMissing(_)
            | ipe::CliError::EmittedBuildFailed { .. }
            | ipe::CliError::DoctorCritical
            | ipe::CliError::EjectUnsupported { .. }),
        ) => {
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
