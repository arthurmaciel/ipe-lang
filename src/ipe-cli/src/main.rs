#![forbid(unsafe_code)]
//! `ipe` binary entry point — a thin wrapper over the [`ipe`] driver library.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match ipe::run_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        // `--check --exit-code` carries a git-style numeric code (10/0/2); the
        // status line was already printed by `run_upgrade`. Deliver it as the
        // process exit code — `ExitCode` renders any byte, so no abrupt exit.
        Err(ipe::CliError::UpgradeCheckExit { code }) => {
            ExitCode::from(u8::try_from(code).unwrap_or(2))
        }
        // These variants render their own complete screen — a full help page, a
        // gate report, or a self-guttered environment message — so the `ipe: `
        // prefix (which belongs to short one-line diagnostics) and the extra
        // gutter are not applied; they print as-is. A command misuse leads with
        // its own reason line before its help page.
        Err(
            err @ (ipe::CliError::UnknownCommand { .. }
            | ipe::CliError::CommandUsage { .. }
            | ipe::CliError::DocCoverage(_)
            | ipe::CliError::DocExamplesFailed(_)
            | ipe::CliError::VerifyFailed { .. }
            | ipe::CliError::TestFailed { .. }
            | ipe::CliError::UpgradeNoPrebuilt { .. }
            | ipe::CliError::ToolchainMissing(_)
            | ipe::CliError::EmittedBuildFailed { .. }
            | ipe::CliError::HealthCritical
            | ipe::CliError::LintGateFailed
            | ipe::CliError::EjectUnsupported { .. }
            | ipe::CliError::UpgradeFeedUnreachable
            // The JSON was already written to stderr; nothing more to print.
            | ipe::CliError::DiagnosticJsonEmitted),
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
