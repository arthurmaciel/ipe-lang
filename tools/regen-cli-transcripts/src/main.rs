//! Regenerate the byte-exact CLI transcript goldens under
//! `src/ipe-cli/tests/golden/cli/`.
//!
//! For every advertised (non-hidden) command it records the `--help` page, and
//! for every extra hermetic invocation in the shared catalog it records the
//! transcript — each redacted through [`ipe::cli_transcript::redact`] and stored
//! in the [`ipe::cli_transcript::golden_envelope`] shape the integration test
//! reads back. The command list, classification, redaction, and envelope all
//! come from `ipe::cli_transcript`, so the tool and the test cannot drift.
//!
//! Transcripts are recorded from one `ipe` binary: the one given by `--ipe-bin`,
//! or else the one `cargo build -p ipe --bin ipe` reports building — the SAME
//! `ipe` the test spawns — so a regenerated golden is faithful by construction:
//! on an unchanged CLI surface it is a no-op (`git status` stays clean), which is
//! exactly what the CI drift gate asserts.
//!
//! Only a transcript `ipe` actually produced is ever written. When `ipe` cannot
//! be built or spawned, or is killed by a signal, the tool exits non-zero before
//! writing any golden, so a tool failure never surfaces as golden drift.
//!
//! Usage:
//!   regen-cli-transcripts                   # regenerate every transcript golden
//!   regen-cli-transcripts --repo-root DIR   # anchor at DIR instead of walking up
//!   regen-cli-transcripts --ipe-bin PATH    # record from a prebuilt `ipe`

#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use ipe::cli_transcript;

fn main() -> ExitCode {
    match run() {
        Ok(count) => {
            println!("regen-cli-transcripts: {count} transcript golden(s) written");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("regen-cli-transcripts: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Why the tool could not record the goldens. Every variant is a failure of the
/// tool or its environment — never a transcript, which is only a [`Transcript`].
#[derive(Debug)]
enum RegenError {
    /// A command-line flag is malformed.
    Usage(String),
    /// The workspace root could not be located or read.
    RepoRoot(String),
    /// `cargo build -p ipe --bin ipe` failed or reported no `ipe` executable.
    BuildIpe(String),
    /// The `ipe` binary could not be spawned.
    Spawn {
        args: Vec<String>,
        error: std::io::Error,
    },
    /// `ipe` was killed by a signal instead of exiting with a code.
    Signal { args: Vec<String> },
    /// A golden (or its directory) could not be written.
    Write {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl fmt::Display for RegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(msg) | Self::RepoRoot(msg) => f.write_str(msg),
            Self::BuildIpe(msg) => write!(f, "cannot build `ipe`: {msg}"),
            Self::Spawn { args, error } => {
                write!(f, "cannot spawn `ipe {}`: {error}", args.join(" "))
            }
            Self::Signal { args } => write!(
                f,
                "`ipe {}` was killed by a signal; no golden written",
                args.join(" ")
            ),
            Self::Write { path, error } => write!(f, "cannot write {}: {error}", path.display()),
        }
    }
}

/// What one `ipe` invocation produced: its exit code and its raw stdout.
struct Transcript {
    exit_code: i32,
    stdout: String,
}

struct Options {
    repo_root: PathBuf,
    ipe_bin: Option<PathBuf>,
}

fn run() -> Result<usize, RegenError> {
    let opts = parse_args()?;
    let repo_root = opts.repo_root;
    // Absolute, so the spawn resolves the same file whatever `current_dir` is.
    let ipe_bin = match opts.ipe_bin {
        Some(path) => std::fs::canonicalize(&path)
            .map_err(|e| RegenError::Usage(format!("--ipe-bin {}: {e}", path.display())))?,
        None => build_ipe(&repo_root)?,
    };

    // Record every golden before writing any, so a failure leaves the committed
    // goldens untouched.
    let mut goldens: Vec<(String, String)> = Vec::new();

    // One golden per advertised command's `--help` page.
    for spec in ipe::help::all_command_specs() {
        if spec.hidden {
            continue;
        }
        let args = [spec.name.to_owned(), "--help".to_owned()];
        let transcript = capture(&ipe_bin, &repo_root, &args)?;
        goldens.push((
            cli_transcript::help_golden_name(spec.name),
            envelope(&transcript, &repo_root),
        ));
    }

    // One golden per extra hermetic invocation.
    for inv in cli_transcript::INVOCATIONS {
        let Some(args) = (inv.args)(&repo_root) else {
            eprintln!(
                "regen-cli-transcripts: skipping `{}` — fixture absent",
                inv.golden
            );
            continue;
        };
        let transcript = capture(&ipe_bin, &repo_root, &args)?;
        goldens.push((inv.golden.to_owned(), envelope(&transcript, &repo_root)));
    }

    let dir = cli_transcript::golden_dir(&repo_root);
    std::fs::create_dir_all(&dir).map_err(|error| RegenError::Write {
        path: dir.clone(),
        error,
    })?;
    for (name, content) in &goldens {
        write_golden(&dir, name, content)?;
    }
    Ok(goldens.len())
}

/// Run `ipe <args>` under `NO_COLOR=1` from the repository root.
fn capture(ipe_bin: &Path, repo_root: &Path, args: &[String]) -> Result<Transcript, RegenError> {
    let output = Command::new(ipe_bin)
        .args(args)
        .current_dir(repo_root)
        .env("NO_COLOR", "1")
        .output()
        .map_err(|error| RegenError::Spawn {
            args: args.to_vec(),
            error,
        })?;
    let exit_code = output.status.code().ok_or_else(|| RegenError::Signal {
        args: args.to_vec(),
    })?;
    Ok(Transcript {
        exit_code,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

/// The redacted transcript in the golden envelope the test reads back.
fn envelope(transcript: &Transcript, repo_root: &Path) -> String {
    let redacted = cli_transcript::redact(&transcript.stdout, repo_root);
    cli_transcript::golden_envelope(Some(transcript.exit_code), &redacted)
}

/// Build `ipe` and return the executable cargo reports, so the transcripts come
/// from exactly the binary just built wherever the target directory lives.
fn build_ipe(repo_root: &Path) -> Result<PathBuf, RegenError> {
    let output = Command::new("cargo")
        .args(["build", "--quiet", "-p", "ipe", "--bin", "ipe"])
        .arg("--message-format=json-render-diagnostics")
        .current_dir(repo_root)
        .output()
        .map_err(|e| RegenError::BuildIpe(format!("cannot spawn `cargo`: {e}")))?;
    if !output.status.success() {
        return Err(RegenError::BuildIpe(format!(
            "`cargo build -p ipe --bin ipe` failed ({})",
            output.status
        )));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|msg| ipe_executable(&msg))
        .ok_or_else(|| {
            RegenError::BuildIpe("cargo reported no executable for the `ipe` binary".to_owned())
        })
}

/// The `executable` of a cargo `compiler-artifact` message for the `ipe` bin.
fn ipe_executable(msg: &serde_json::Value) -> Option<PathBuf> {
    let target = msg.get("target")?;
    let is_ipe_bin = msg.get("reason")?.as_str()? == "compiler-artifact"
        && target.get("name")?.as_str()? == "ipe"
        && target
            .get("kind")?
            .as_array()?
            .iter()
            .any(|k| k.as_str() == Some("bin"));
    if !is_ipe_bin {
        return None;
    }
    msg.get("executable")?.as_str().map(PathBuf::from)
}

/// Write a golden file, creating or overwriting it.
fn write_golden(dir: &Path, basename: &str, content: &str) -> Result<(), RegenError> {
    let path = dir.join(format!("{basename}.txt"));
    std::fs::write(&path, content).map_err(|error| RegenError::Write { path, error })
}

fn parse_args() -> Result<Options, RegenError> {
    let mut repo_root = None;
    let mut ipe_bin = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "--repo-root" => &mut repo_root,
            "--ipe-bin" => &mut ipe_bin,
            other => return Err(RegenError::Usage(format!("unknown argument `{other}`"))),
        };
        let value = args
            .next()
            .ok_or_else(|| RegenError::Usage(format!("{arg} requires a path argument")))?;
        *slot = Some(PathBuf::from(value));
    }
    let repo_root = match repo_root {
        Some(path) => path,
        None => find_workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")))?,
    };
    Ok(Options { repo_root, ipe_bin })
}

fn find_workspace_root(start: &Path) -> Result<PathBuf, RegenError> {
    let mut current = start;
    loop {
        let candidate = current.join("Cargo.toml");
        if candidate.exists() {
            let content = std::fs::read_to_string(&candidate).map_err(|e| {
                RegenError::RepoRoot(format!("cannot read {}: {e}", candidate.display()))
            })?;
            if content.contains("[workspace]") {
                return Ok(current.to_owned());
            }
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => {
                return Err(RegenError::RepoRoot(
                    "workspace root not found — pass --repo-root".to_owned(),
                ));
            }
        }
    }
}
