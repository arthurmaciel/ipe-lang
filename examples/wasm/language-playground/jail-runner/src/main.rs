//! `jail-runner` — jailed build+run harness for the Ipê playground `/run` surface.
//!
//! Process boundary: argv in, JSON out. The Ipê server stages a Rust crate
//! (Cargo.toml + src/main.rs, split from the client's banner-delimited
//! emitted Rust) under a scratch dir and execs this binary with the project
//! dir as the single positional argument. Every outcome is printed as one
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

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde::Serialize;

use ipe_sandbox::unsandboxed_override_set;
use playground_jail_runner::run_jailed::{
    self, app_binary_path, jailed_build, jailed_run, probe_or_refuse, seed_cargo_home,
    seed_target_dir,
};

const HARNESS_WALL_DEFAULT_SECS: u64 = 60;
const UNSANDBOXED_OUTPUT_CAP_BYTES: u64 = 64 * 1024;
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

fn main() {
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — playground binary `main`
    // process-boundary entry: argv in, JSON out, no other surface.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let code = match args.first().map(String::as_str) {
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
    std::process::exit(code);
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
         \x20   prewarm  Build the embedded hello project into the warm cache so jailed\n\
         \x20            builds can resolve dependencies with --offline.\n\
         \n\
         The warm cache defaults to $IPE_PLAYGROUND_WARM_DIR or ~/.cache/ipe/playground-warm."
    );
}

fn cmd_run(args: &[String]) -> i32 {
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
    print_json(&outcome);
    cleanup_project(&parsed.project_dir);
    0
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
    let warm_dir = warm_dir.unwrap_or_else(resolve_warm_dir);
    Ok(RunArgs {
        project_dir,
        wall_secs,
        warm_dir,
    })
}

fn cmd_prewarm(args: &[String]) -> i32 {
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
    let warm_dir = warm_dir.unwrap_or_else(resolve_warm_dir);
    let outcome = prewarm(&warm_dir);
    print_json(&outcome);
    0
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
        print_json(&outcome);
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

fn resolve_warm_dir() -> PathBuf {
    if let Some(value) = std::env::var_os(WARM_DIR_ENV) {
        return PathBuf::from(value);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(DEFAULT_WARM_DIR)
}

/// The jailed pipeline. Returns the JSON outcome; never panics.
/// Pin the dependency graph before building: with `Cargo.lock` present,
/// `cargo build --offline` never re-resolves from the registry index, so the
/// jail's cargo cannot prune sparse-index cache entries (hard-linked from the
/// warm cache during seeding — observed: entries for the direct dependencies
/// vanished from the project's seeded index after a jailed build, leaving
/// later runs with "no matching package named `X` found" resolution errors).
/// The lock is the warm template's own lock: the emitted `Cargo.toml` is a
/// fixed template, so the graph is identical for every submitted program.
fn provision_lock(project_dir: &Path, warm_dir: &Path) -> Option<Outcome> {
    if project_dir.join("Cargo.lock").is_file() {
        return None;
    }
    let warm_lock = warm_dir.join("Cargo.lock");
    if !warm_lock.is_file() {
        return Some(Outcome::failure(format!(
            "warm cache has no Cargo.lock at {} — re-run `jail-runner prewarm`",
            warm_lock.display()
        )));
    }
    if let Err(error) = std::fs::copy(&warm_lock, project_dir.join("Cargo.lock")) {
        return Some(Outcome::failure(format!(
            "failed to copy warm Cargo.lock: {error}"
        )));
    }
    None
}

fn run_project(project_dir: &Path, warm_dir: &Path) -> Outcome {
    let manifest = project_dir.join("Cargo.toml");
    let entry = project_dir.join("src/main.rs");
    if !manifest.is_file() || !entry.is_file() {
        return Outcome::failure("project dir is missing Cargo.toml or src/main.rs");
    }
    let warm_cargo_home = warm_dir.join("cargo-home");
    let warm_target = warm_dir.join("crate-target");
    if !warm_cargo_home.is_dir() || !warm_target.is_dir() {
        return Outcome::failure(format!(
            "warm cache missing at {} — run `jail-runner prewarm` first",
            warm_dir.display()
        ));
    }
    if let Some(outcome) = provision_lock(project_dir, warm_dir) {
        return outcome;
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
                return run_unsandboxed(project_dir);
            }
            return Outcome::failure(format!("sandbox unavailable: {}", refusal.reason));
        }
    };

    if let Err(defect) = seed_cargo_home(project_dir, &warm_cargo_home) {
        return Outcome::failure(format!("failed to seed cargo home: {defect}"));
    }
    if let Err(defect) = seed_target_dir(project_dir, &warm_target) {
        return Outcome::failure(format!("failed to seed target dir: {defect}"));
    }

    let build = match jailed_build(&caps, project_dir) {
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
fn run_unsandboxed(project_dir: &Path) -> Outcome {
    let build = match run_captured(
        &mut cargo_build_cmd(project_dir),
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

/// Bounded capture: each stream is read up to `cap_bytes + 1` so oversize
/// output is truncated but still distinguishable from an exact fit.
fn read_capped<R: std::io::Read>(stream: R, cap_bytes: u64) -> String {
    let mut buf = Vec::new();
    let _ = stream.take(cap_bytes + 1).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn run_captured(cmd: &mut Command, cap_bytes: u64) -> Result<Captured, String> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|error| {
        format!(
            "failed to spawn {}: {error}",
            cmd.get_program().to_string_lossy()
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .map_or_else(String::new, |stream| read_capped(stream, cap_bytes));
    let stderr = child
        .stderr
        .take()
        .map_or_else(String::new, |stream| read_capped(stream, cap_bytes));
    let status = child.wait().map_err(|error| {
        format!(
            "failed to wait on {}: {error}",
            cmd.get_program().to_string_lossy()
        )
    })?;
    Ok(Captured {
        status: status.code(),
        stdout,
        stderr,
    })
}

fn cargo_build_cmd(project_dir: &Path) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(project_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(project_dir.join("crate-target"))
        .env("CARGO_TERM_PROGRESS_WHEN", "never");
    cmd
}

/// Build the embedded hello project into the warm cache so `--offline`
/// jailed builds can resolve the crate template's dependency closure.
fn prewarm(warm_dir: &Path) -> Outcome {
    let warm_cargo_home = warm_dir.join("cargo-home");
    let warm_target = warm_dir.join("crate-target");
    if let Err(error) = std::fs::create_dir_all(&warm_cargo_home) {
        return Outcome::failure(format!(
            "failed to create warm cargo home {}: {error}",
            warm_cargo_home.display()
        ));
    }
    if let Err(error) = std::fs::create_dir_all(&warm_target) {
        return Outcome::failure(format!(
            "failed to create warm target {}: {error}",
            warm_target.display()
        ));
    }

    let scratch = match tempfile::tempdir() {
        Ok(scratch) => scratch,
        Err(error) => return Outcome::failure(format!("failed to create scratch dir: {error}")),
    };
    let src_dir = scratch.path().join("src");
    if let Err(error) = std::fs::create_dir_all(&src_dir) {
        return Outcome::failure(format!("failed to create scratch src dir: {error}"));
    }
    let manifest = include_str!("crate_template/Cargo.toml");
    let hello = include_str!("crate_template/main.rs");
    if let Err(error) = std::fs::write(scratch.path().join("Cargo.toml"), manifest) {
        return Outcome::failure(format!("failed to stage template Cargo.toml: {error}"));
    }
    if let Err(error) = std::fs::write(src_dir.join("main.rs"), hello) {
        return Outcome::failure(format!("failed to stage template main.rs: {error}"));
    }

    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(scratch.path().join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&warm_target)
        .env("CARGO_HOME", &warm_cargo_home)
        .env("CARGO_TERM_PROGRESS_WHEN", "never");
    match run_captured(&mut cmd, UNSANDBOXED_OUTPUT_CAP_BYTES) {
        Ok(captured) if captured.status == Some(0) => {
            // Save the resolved lockfile: `run` copies it into each project so
            // the jailed cargo never re-resolves from the registry index.
            let lock_src = scratch.path().join("Cargo.lock");
            let lock_dst = warm_dir.join("Cargo.lock");
            if !lock_src.is_file() {
                return Outcome::failure("prewarm build produced no Cargo.lock");
            }
            if let Err(error) = std::fs::copy(&lock_src, &lock_dst) {
                return Outcome::failure(format!(
                    "failed to save warm Cargo.lock to {}: {error}",
                    lock_dst.display()
                ));
            }
            Outcome {
                ok: true,
                unsandboxed: false,
                build: None,
                run: None,
                exit: None,
                error: None,
            }
        }
        Ok(captured) => Outcome::failure(format!(
            "prewarm build failed: {}",
            tail(&captured.stderr, 2000)
        )),
        Err(message) => Outcome::failure(format!("prewarm build error: {message}")),
    }
}

fn tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut result = text.to_owned();
    result.drain(..result.len() - max_bytes);
    format!("…{result}")
}

fn print_json(outcome: &Outcome) {
    match serde_json::to_string(outcome) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            // JSON cannot fail here (all fields are simple), but never exit 0
            // with a partial document: print the error on stderr and exit 1.
            eprintln!("[jail-runner] fatal: failed to serialize outcome: {error}");
            std::process::exit(1);
        }
    }
}
