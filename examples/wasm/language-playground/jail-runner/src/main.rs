//! `jail-runner` — jailed build+run harness for the Ipê playground `/run` surface.
//!
//! Process boundary: argv in, JSON out. The Ipê server stages the client's
//! emitted crate (`Cargo.toml` + `src/`, split from the banner-delimited
//! emitted Rust) under a scratch dir and execs this binary with the project
//! dir as the single positional argument. The harness refuses any other entry
//! in that dir, then adds the trusted rest itself: the runtime crate, the
//! lockfile, the read-only vendored crate sources and a private copy of the
//! warm target.
//! Every outcome is printed as one
//! JSON document on stdout; the exit code is `0` whenever JSON was printed,
//! `1` only when JSON could not be printed (crash), and `2` on usage errors
//! or harness wall-clock expiry.
//!
//! Security posture (IPE-F4410 fail-closed): the build and run phases run in
//! a bubblewrap jail (network denied, filesystem jailed, rlimits, wall-clock)
//! via `ipe_sandbox`. If the jail cannot be assembled, the harness refuses —
//! it never runs unjailed unless `IPE_FFI_ALLOW_UNSANDBOXED=1` is set, which
//! the runtime only honours on the driver's loud trust warning.
#![allow(clippy::module_name_repetitions)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde::Serialize;

use ipe_sandbox::unsandboxed_override_set;
use playground_jail_runner::run_jailed::{
    self, VendorSource, app_binary_path, jailed_build, jailed_run, probe_or_refuse, seed_target_dir,
};
use playground_jail_runner::staging::{
    PREWARM_PROGRAM, RuntimeFiles, check_project_layout, write_emitted, write_runtime,
};

const HARNESS_WALL_DEFAULT_SECS: u64 = 60;
const UNSANDBOXED_OUTPUT_CAP_BYTES: u64 = 64 * 1024;
/// Output cap for one trusted prewarm cargo step, run `--quiet`.
const PREWARM_OUTPUT_CAP_BYTES: u64 = 1024 * 1024;
const WARM_DIR_ENV: &str = "IPE_PLAYGROUND_WARM_DIR";
const DEFAULT_WARM_DIR: &str = ".cache/ipe/playground-warm";

/// Serializable mirror of `run_jailed::PhaseOutcome`.
#[derive(Serialize)]
struct PhaseJson {
    status: Option<i32>,
    stdout: String,
    stderr: String,
    killed: bool,
}

impl From<run_jailed::PhaseOutcome> for PhaseJson {
    fn from(phase: run_jailed::PhaseOutcome) -> Self {
        Self {
            status: phase.status,
            stdout: phase.stdout,
            stderr: phase.stderr,
            killed: phase.killed,
        }
    }
}

/// The single wire shape the server understands.
#[derive(Serialize)]
struct Outcome {
    ok: bool,
    unsandboxed: bool,
    build: Option<PhaseJson>,
    run: Option<PhaseJson>,
    exit: Option<i32>,
    error: Option<String>,
}

impl Outcome {
    fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            unsandboxed: false,
            build: None,
            run: None,
            exit: None,
            error: Some(error.into()),
        }
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let code: u8 = match args.first().map(String::as_str) {
        Some("run") => cmd_run(rest),
        Some("prewarm") => cmd_prewarm(rest),
        Some("help" | "--help" | "-h") => {
            usage();
            0
        }
        _ => {
            usage();
            2
        }
    };
    std::process::ExitCode::from(code)
}

fn usage() {
    eprintln!(
        "jail-runner: jailed build+run harness for the Ipê playground /run surface\n\
         \n\
         USAGE:\n\
         \x20   jail-runner run <project-dir> [--wall N] [--warm <dir>]\n\
         \x20   jail-runner prewarm [--warm <dir>]\n\
         \n\
         \x20   run      Build (cargo build --offline) and run the staged Rust project\n\
         \x20            inside a bubblewrap jail; prints one JSON document to stdout.\n\
         \x20   prewarm  Fetch the emitted crate's dependencies and build a hello program\n\
         \x20            into the warm cache so jailed builds run --offline.\n\
         \n\
         The warm cache defaults to $IPE_PLAYGROUND_WARM_DIR or ~/.cache/ipe/playground-warm."
    );
}

fn cmd_run(args: &[String]) -> u8 {
    let parsed = match parse_run_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            usage();
            return 2;
        }
    };
    start_watchdog(parsed.wall_secs, &parsed.project_dir);
    let outcome = run_project(&parsed.project_dir, &parsed.warm_dir);
    let code = print_json(&outcome);
    cleanup_project(&parsed.project_dir);
    code
}

struct RunArgs {
    project_dir: PathBuf,
    wall_secs: u64,
    warm_dir: PathBuf,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut project_dir: Option<PathBuf> = None;
    let mut wall_secs = HARNESS_WALL_DEFAULT_SECS;
    let mut warm_dir: Option<PathBuf> = None;
    let mut positionals = 0;
    let mut index = 0;
    while index < args.len() {
        let arg = args.get(index).map(String::as_str);
        match arg {
            Some("--wall") => {
                index += 1;
                let raw = args.get(index).ok_or("--wall requires a value")?;
                wall_secs = raw
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --wall value: {raw}"))?;
            }
            Some("--warm") => {
                index += 1;
                warm_dir = Some(PathBuf::from(
                    args.get(index).ok_or("--warm requires a value")?,
                ));
            }
            Some(flag) if flag.starts_with('-') => return Err(format!("unknown flag: {flag}")),
            Some(value) => {
                positionals += 1;
                if positionals > 1 {
                    return Err(format!("unexpected extra argument: {value}"));
                }
                project_dir = Some(PathBuf::from(value));
            }
            None => break,
        }
        index += 1;
    }
    let project_dir = project_dir.ok_or_else(|| "missing <project-dir> argument".to_owned())?;
    let warm_dir = match warm_dir {
        Some(dir) => dir,
        None => resolve_warm_dir().map_err(|e| e.to_string())?,
    };
    Ok(RunArgs {
        project_dir,
        wall_secs,
        warm_dir,
    })
}

fn cmd_prewarm(args: &[String]) -> u8 {
    let mut warm_dir: Option<PathBuf> = None;
    let mut index = 0;
    while index < args.len() {
        match args.get(index).map(String::as_str) {
            Some("--warm") => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("--warm requires a value");
                    usage();
                    return 2;
                };
                warm_dir = Some(PathBuf::from(value));
            }
            Some(flag) if flag.starts_with('-') => {
                eprintln!("unknown flag: {flag}");
                usage();
                return 2;
            }
            Some(value) => {
                eprintln!("unexpected argument: {value}");
                usage();
                return 2;
            }
            None => break,
        }
        index += 1;
    }
    let warm_dir = match warm_dir.map_or_else(resolve_warm_dir, Ok) {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    print_json(&prewarm(&warm_dir))
}

/// Harness-level wall-clock: after `wall_secs` the watchdog prints a timeout
/// JSON document and exits hard. The jail wrapper runs with
/// `--die-with-parent`, so the whole bwrap tree dies with the harness.
fn start_watchdog(wall_secs: u64, project_dir: &Path) {
    let project_dir = project_dir.to_path_buf();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(wall_secs));
        let outcome = Outcome {
            ok: false,
            unsandboxed: false,
            build: None,
            run: None,
            exit: None,
            error: Some(format!("timed out after {wall_secs}s (harness wall-clock)")),
        };
        let _ = print_json(&outcome);
        // Best-effort: remove the staged project (compiled artifacts can be
        // large). Children may still hold cwd entries; leftover files in that
        // race are bounded by the wall budget and harmless.
        cleanup_project(&project_dir);
        // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — watchdog expiry: process
        // exit kills all threads; --die-with-parent reaps the bwrap tree.
        std::process::exit(2);
    });
}

/// Best-effort removal of a staged project tree. The harness owns the
/// project-dir lifecycle: the server stages it, this binary runs it, and
/// nothing else touches it afterwards (the server never reuses project
/// dirs). Failures are ignored — a leftover tree is a bounded disk cost,
/// never a correctness issue.
fn cleanup_project(project_dir: &Path) {
    let _ = std::fs::remove_dir_all(project_dir);
}

/// Why no absolute warm-cache directory could be derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WarmDirError {
    /// `IPE_PLAYGROUND_WARM_DIR` is set to a relative path.
    RelativeOverride,
    /// No override is set and the invoking user's home is unset or relative.
    HomeUnresolved,
}

impl std::fmt::Display for WarmDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RelativeOverride => write!(
                f,
                "{WARM_DIR_ENV} must be an absolute path; pass --warm <dir> or set it absolute"
            ),
            Self::HomeUnresolved => write!(
                f,
                "cannot locate the warm cache: HOME is unset or relative and {WARM_DIR_ENV} \
                 is unset; pass --warm <dir> or set {WARM_DIR_ENV}"
            ),
        }
    }
}

fn resolve_warm_dir() -> Result<PathBuf, WarmDirError> {
    resolve_warm_dir_from(ipe_env::var_os(WARM_DIR_ENV), ipe_sandbox::home::home_dir())
}

/// Resolve the warm-cache directory, refusing any cwd-relative spelling.
///
/// An empty override counts as unset.
fn resolve_warm_dir_from(
    raw: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Result<PathBuf, WarmDirError> {
    if let Some(value) = raw.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err(WarmDirError::RelativeOverride)
        };
    }
    home.filter(|home| home.is_absolute())
        .map(|home| home.join(DEFAULT_WARM_DIR))
        .ok_or(WarmDirError::HomeUnresolved)
}

/// The embedded runtime crate source, the same text `ipe build` materialises.
fn runtime_files() -> Result<&'static RuntimeFiles, String> {
    ipe::runtime_embed::collect_embedded_crate_text()
        .map_err(|error| format!("embedded runtime unavailable: {error}"))
}

/// Add the trusted files the emitted crate builds against: the runtime crate
/// and the warm lockfile.
///
/// The lock pins the build to exactly the crates prewarm fetched, so the
/// offline build never re-resolves. Cargo's lockfile is independent of the
/// features a program selects, so the one lock covers every emitted crate.
fn stage_harness_files(project_dir: &Path, warm_lock: &Path) -> Result<(), String> {
    write_runtime(project_dir, runtime_files()?)
        .map_err(|error| format!("failed to stage the runtime: {error}"))?;
    let lock = project_dir.join("Cargo.lock");
    let copied = std::fs::File::open(warm_lock).and_then(|mut input| {
        let mut output = std::fs::File::create_new(&lock)?;
        std::io::copy(&mut input, &mut output)
    });
    copied
        .map(drop)
        .map_err(|error| format!("failed to stage the warm Cargo.lock: {error}"))
}

/// The jailed pipeline. Returns the JSON outcome; never panics.
fn run_project(project_dir: &Path, warm_dir: &Path) -> Outcome {
    if let Err(error) = check_project_layout(project_dir) {
        return Outcome::failure(error.to_string());
    }
    let warm_target = warm_dir.join("crate-target");
    let warm_lock = warm_dir.join("Cargo.lock");
    let warm_vendor = warm_dir.join("vendor");
    if !warm_vendor.is_dir() || !warm_target.is_dir() || !warm_lock.is_file() {
        return Outcome::failure(format!(
            "warm cache missing at {} — run `jail-runner prewarm` first",
            warm_dir.display()
        ));
    }
    if let Err(message) = stage_harness_files(project_dir, &warm_lock) {
        return Outcome::failure(message);
    }
    let vendor = match VendorSource::resolve(&warm_vendor) {
        Ok(vendor) => vendor,
        Err(defect) => return Outcome::failure(format!("warm vendor dir unusable: {defect}")),
    };
    if let Err(defect) = seed_target_dir(project_dir, &warm_target) {
        return Outcome::failure(format!("failed to seed target dir: {defect}"));
    }

    let caps = match probe_or_refuse() {
        Ok(caps) => caps,
        Err(refusal) => {
            if unsandboxed_override_set() {
                eprintln!(
                    "[jail-runner] WARNING: IPE_FFI_ALLOW_UNSANDBOXED=1 — running the \
                     submitted program WITHOUT a jail. This is a trust boundary breach; \
                     only use it on a throwaway host."
                );
                return run_unsandboxed(project_dir, &vendor);
            }
            return Outcome::failure(format!("sandbox unavailable: {}", refusal.reason));
        }
    };

    let build = match jailed_build(&caps, project_dir, &vendor) {
        Ok(build) => build,
        Err(defect) => return Outcome::failure(format!("jail build failed: {defect}")),
    };
    let build_json = PhaseJson::from(build.clone());
    if build.killed {
        return Outcome {
            ok: false,
            unsandboxed: false,
            build: Some(build_json),
            run: None,
            exit: None,
            error: Some("build phase hit its wall-clock limit".to_owned()),
        };
    }
    if build.status != Some(0) {
        return Outcome {
            ok: false,
            unsandboxed: false,
            build: Some(build_json),
            run: None,
            exit: None,
            error: Some("build phase failed (non-zero exit)".to_owned()),
        };
    }

    let binary = app_binary_path(project_dir);
    if !binary.is_file() {
        return Outcome {
            ok: false,
            unsandboxed: false,
            build: Some(build_json),
            run: None,
            exit: None,
            error: Some("build reported success but produced no `ipe-app` binary".to_owned()),
        };
    }

    let run = match jailed_run(&caps, project_dir, &binary) {
        Ok(run) => run,
        Err(defect) => return Outcome::failure(format!("jail run failed: {defect}")),
    };
    Outcome {
        ok: true,
        unsandboxed: false,
        build: Some(build_json),
        run: Some(PhaseJson::from(run.clone())),
        exit: run.status,
        error: None,
    }
}

/// The `IPE_FFI_ALLOW_UNSANDBOXED=1` escape hatch: same phases, plain
/// subprocesses, output capped, wall-clock still enforced by the watchdog.
fn run_unsandboxed(project_dir: &Path, vendor: &VendorSource) -> Outcome {
    let build = match run_captured(
        &mut cargo_build_cmd(project_dir, vendor),
        UNSANDBOXED_OUTPUT_CAP_BYTES,
    ) {
        Ok(build) => build,
        Err(message) => return Outcome::failure(message),
    };
    let build_json = PhaseJson {
        status: build.status,
        stdout: build.stdout.clone(),
        stderr: build.stderr.clone(),
        killed: false,
    };
    if build.status != Some(0) {
        return Outcome {
            ok: false,
            unsandboxed: true,
            build: Some(build_json),
            run: None,
            exit: None,
            error: Some("build phase failed (non-zero exit)".to_owned()),
        };
    }

    let binary = app_binary_path(project_dir);
    let run = match run_captured(&mut Command::new(&binary), UNSANDBOXED_OUTPUT_CAP_BYTES) {
        Ok(run) => run,
        Err(message) => return Outcome::failure(message),
    };
    Outcome {
        ok: true,
        unsandboxed: true,
        build: Some(build_json),
        run: Some(PhaseJson {
            status: run.status,
            stdout: run.stdout,
            stderr: run.stderr,
            killed: false,
        }),
        exit: run.status,
        error: None,
    }
}

struct Captured {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run `cmd` outside the jail with both streams drained concurrently under
/// `cap_bytes`, so a stream-heavy child never wedges and an oversize one is
/// killed and reported rather than buffered.
fn run_captured(cmd: &mut Command, cap_bytes: u64) -> Result<Captured, String> {
    ipe_sandbox::run_captured_bounded(cmd, cap_bytes)
        .map(|output| Captured {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
        .map_err(|defect| defect.to_string())
}

fn cargo_build_cmd(project_dir: &Path, vendor: &VendorSource) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--offline")
        .args(vendor.cargo_config_args())
        .arg("--manifest-path")
        .arg(project_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(project_dir.join("crate-target"))
        .env("CARGO_HOME", project_dir.join("cargo-home"))
        .env("CARGO_TERM_PROGRESS_WHEN", "never");
    cmd
}

/// A cargo invocation for prewarm over the scratch crate.
///
/// The environment is cleared down to what the jail also passes, and the
/// working directory is the scratch crate, so the toolchain rustup selects and
/// every build input match the jailed build that later reuses the artifacts.
fn prewarm_cargo(subcommand: &str, scratch: &Path, warm_cargo_home: &Path) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg(subcommand)
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(scratch.join("Cargo.toml"))
        .current_dir(scratch)
        .env_clear()
        .env("CARGO_HOME", warm_cargo_home)
        .env("CARGO_TERM_PROGRESS_WHEN", "never");
    for key in ["PATH", "HOME", "RUSTUP_HOME"] {
        if let Some(value) = ipe_env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd
}

/// Run one prewarm cargo step, turning a failure into its message.
fn prewarm_step(cmd: &mut Command, step: &str) -> Result<(), String> {
    match run_captured(cmd, PREWARM_OUTPUT_CAP_BYTES) {
        Ok(captured) if captured.status == Some(0) => Ok(()),
        Ok(captured) => Err(format!(
            "prewarm {step} failed: {}",
            tail(&captured.stderr, 2000)
        )),
        Err(message) => Err(format!("prewarm {step} error: {message}")),
    }
}

/// Fill the warm cache the jailed builds run `--offline` against.
///
/// Compiles [`PREWARM_PROGRAM`] through the same emit the page shows, stages
/// the runtime crate as `run` does, vendors every crate the resolved lock names
/// into the warm vendor dir, builds into the warm target against that vendor
/// dir exactly as the jailed build does, and saves the lock `run` stages into
/// each project.
fn prewarm(warm_dir: &Path) -> Outcome {
    match prewarm_into(warm_dir) {
        Ok(()) => Outcome {
            ok: true,
            unsandboxed: false,
            build: None,
            run: None,
            exit: None,
            error: None,
        },
        Err(message) => Outcome::failure(message),
    }
}

fn prewarm_into(warm_dir: &Path) -> Result<(), String> {
    let warm_cargo_home = warm_dir.join("cargo-home");
    let warm_target = warm_dir.join("crate-target");
    let warm_vendor = warm_dir.join("vendor");
    for dir in [&warm_cargo_home, &warm_target] {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("failed to create {}: {error}", dir.display()))?;
    }
    let scratch = ipe_sandbox::scratch::ScratchDir::new("ipe-playground-prewarm")
        .map_err(|error| format!("failed to create scratch dir: {error}"))?;
    let files = ipe_wasm::emit_files(PREWARM_PROGRAM)
        .map_err(|diagnostic| format!("prewarm program rejected: {diagnostic}"))?;
    write_emitted(scratch.path(), &files)
        .map_err(|error| format!("failed to stage prewarm crate: {error}"))?;
    write_runtime(scratch.path(), runtime_files()?)
        .map_err(|error| format!("failed to stage the runtime: {error}"))?;

    prewarm_step(
        &mut prewarm_cargo("fetch", scratch.path(), &warm_cargo_home),
        "fetch",
    )?;
    let mut vendor_cmd = prewarm_cargo("vendor", scratch.path(), &warm_cargo_home);
    vendor_cmd
        .arg("--locked")
        .arg("--offline")
        .arg(&warm_vendor);
    prewarm_step(&mut vendor_cmd, "vendor")?;
    let vendor = VendorSource::resolve(&warm_vendor)
        .map_err(|error| format!("warm vendor dir unusable: {error}"))?;

    // The second pass settles the build-script outputs whose recorded
    // modification times the first pass leaves older than their inputs, so the
    // warm target is fresh for every seeded copy.
    for pass in ["build", "settle"] {
        let mut build = prewarm_cargo("build", scratch.path(), &warm_cargo_home);
        build
            .arg("--offline")
            .args(vendor.cargo_config_args())
            .arg("--target-dir")
            .arg(&warm_target);
        prewarm_step(&mut build, pass)?;
    }

    let lock_src = scratch.path().join("Cargo.lock");
    let lock_dst = warm_dir.join("Cargo.lock");
    std::fs::copy(&lock_src, &lock_dst).map_err(|error| {
        format!(
            "failed to save warm Cargo.lock to {}: {error}",
            lock_dst.display()
        )
    })?;
    Ok(())
}

/// The last at most `max_bytes` bytes of `text`, cut on a character boundary.
fn tail(text: &str, max_bytes: usize) -> String {
    let wanted = text.len().saturating_sub(max_bytes);
    let start = (wanted..=text.len())
        .find(|&index| text.is_char_boundary(index))
        .unwrap_or(text.len());
    match text.get(start..) {
        Some(rest) if start > 0 => format!("…{rest}"),
        _ => text.to_owned(),
    }
}

/// Print `outcome` as the one JSON document on stdout; the exit code to report.
///
/// A document that cannot be serialised is reported on stderr with exit 1, so
/// the harness never exits 0 without a complete document.
fn print_json(outcome: &Outcome) -> u8 {
    match serde_json::to_string(outcome) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(error) => {
            eprintln!("[jail-runner] fatal: failed to serialize outcome: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_WARM_DIR, WarmDirError, resolve_warm_dir_from, run_captured, tail};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::process::Command;

    /// A child that writes `stderr_bytes` to stderr while its stdout stays open.
    fn stderr_heavy(stderr_bytes: u32) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("head -c {stderr_bytes} /dev/zero >&2; echo done"));
        cmd
    }

    #[test]
    fn a_stderr_heavy_child_is_drained_without_wedging() {
        let captured = run_captured(&mut stderr_heavy(256 * 1024), 1024 * 1024);
        assert!(matches!(
            captured,
            Ok(ref out) if out.status == Some(0) && out.stdout == "done\n" && out.stderr.len() == 256 * 1024
        ));
    }

    #[test]
    fn a_child_past_the_output_cap_is_refused() {
        assert!(run_captured(&mut stderr_heavy(256 * 1024), 64 * 1024).is_err());
    }

    #[test]
    fn a_missing_home_without_an_override_is_refused() {
        assert_eq!(
            resolve_warm_dir_from(None, None),
            Err(WarmDirError::HomeUnresolved)
        );
        assert_eq!(
            resolve_warm_dir_from(Some(OsString::new()), None),
            Err(WarmDirError::HomeUnresolved)
        );
        assert_eq!(
            resolve_warm_dir_from(None, Some(PathBuf::from("relative/home"))),
            Err(WarmDirError::HomeUnresolved)
        );
    }

    #[test]
    fn a_relative_override_is_refused() {
        assert_eq!(
            resolve_warm_dir_from(Some(OsString::from("warm")), Some(PathBuf::from("/home/u"))),
            Err(WarmDirError::RelativeOverride)
        );
    }

    #[test]
    fn a_tail_never_splits_a_character() {
        assert_eq!(tail("short", 10), "short");
        assert_eq!(tail("abcdef", 3), "…def");
        // `é` is two bytes; a cut landing inside it moves past it.
        assert_eq!(tail("aéb", 2), "…b");
    }

    #[test]
    fn an_absolute_override_or_home_resolves() {
        assert_eq!(
            resolve_warm_dir_from(Some(OsString::from("/srv/warm")), None),
            Ok(PathBuf::from("/srv/warm"))
        );
        assert_eq!(
            resolve_warm_dir_from(None, Some(PathBuf::from("/home/u"))),
            Ok(PathBuf::from("/home/u").join(DEFAULT_WARM_DIR))
        );
    }
}
