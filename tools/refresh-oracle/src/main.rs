//! `refresh-oracle` — (re)capture the cached Go-oracle value for golden parity.
//!
//! Usage:
//!
//! ```text
//! refresh-oracle <name> [<name> ...]   # register / refresh specific goldens
//! refresh-oracle --all                 # refresh every already-registered golden
//! ```
//!
//! For each golden under `tests/golden/<name>/` it:
//!
//! 1. hashes `Main.sky` (the staleness key the read side re-checks);
//! 2. captures the Go `sky --version` string for provenance;
//! 3. runs the **Go** reference compiler on a private copy of `Main.sky`
//!    (`sky build` → run the produced binary) so the captured bytes are ONLY the
//!    program's stdout, never the compiler's progress chatter;
//! 4. on Go SUCCESS: writes `expected_go.txt` = the Go binary's stdout and
//!    `oracle.meta` with `oracle_divergence = false`;
//! 5. on Go FAILURE (panic / non-zero / build error): does NOT cache the Go
//!    failure as "correct". Instead it builds the SAME program with skyc, runs
//!    it, and records skyc's (correct) output as the expected with
//!    `oracle_divergence = true` and a reason — exactly the "Go oracle can be
//!    buggy" carve-out from the design doc.
//!
//! A golden may ALSO opt in to a **sanctioned divergence** by dropping a
//! `sanctioned.divergence` marker file (its contents are the reason) in its
//! directory. There the Go oracle SUCCEEDS but Sky-Rust is deliberately more
//! correct (e.g. full-Unicode case mapping). The refresh tool short-circuits to
//! skyc's output and records it with `oracle_divergence = true` and a
//! `sanctioned: <reason>` note — WITHOUT requiring Go to fail. See
//! `docs/architecture/divergence-policy.md`.
//!
//! `--all` refreshes only goldens that already carry an `oracle.meta`, so it
//! re-captures registered runnable goldens without trying to run the negative /
//! gate goldens (which have no program output). Register a new runnable golden
//! by naming it explicitly once.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use oracle::{EXPECTED_FILE, MAIN_SKY, META_FILE, Meta, RunResult, build_and_run_rust, sha256_hex};

/// Default location of the Go reference compiler. Override with `SKY_GO_ORACLE`.
const DEFAULT_GO_ORACLE: &str = "/home/arthur/Documentos/comp/sky/sky-out/sky";

fn go_oracle() -> String {
    std::env::var("SKY_GO_ORACLE").unwrap_or_else(|_| DEFAULT_GO_ORACLE.to_owned())
}

/// The `sky-rust` workspace root (two levels up from this crate's manifest:
/// `tools/refresh-oracle` → repo root).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_root(root: &Path) -> PathBuf {
    root.join("tests").join("golden")
}

/// Outcome of trying to obtain an oracle value for one golden.
enum Capture {
    /// Go produced a valid reference; cache it as the expected.
    Go { stdout: String, exit_code: i32 },
    /// Go failed on this shape; skyc's correct output is the expected instead.
    Divergence {
        stdout: String,
        exit_code: i32,
        reason: String,
    },
}

/// Read the Go `sky --version`, trimmed to its first line.
fn go_version(oracle_bin: &str) -> String {
    Command::new(oracle_bin)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .map(|s| s.lines().next().unwrap_or("").trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Run the Go reference compiler on a private copy of `main_sky` and return its
/// clean program stdout + exit code. `sky build` is used (not `sky run`) so the
/// compiler's progress chatter never lands on the captured stream: we run the
/// produced binary separately and read only its stdout.
///
/// Returns `Err` with a human-readable reason when the Go build fails, the
/// binary is missing, or the binary cannot be executed — all of which route the
/// caller to the divergence branch.
fn run_go_oracle(oracle_bin: &str, main_sky: &Path, scratch: &Path) -> Result<RunResult, String> {
    std::fs::create_dir_all(scratch).map_err(|e| format!("cannot create scratch dir: {e}"))?;
    let dest = scratch.join(MAIN_SKY);
    std::fs::copy(main_sky, &dest).map_err(|e| format!("cannot copy Main.sky: {e}"))?;

    // The Go `sky` binary ALSO honours `SKY_RUNTIME_DIR` — but that variable is
    // ours, pointing at the *Rust* runtime so skyc can resolve it on the
    // divergence path. If it leaks into the Go build the Go compiler vendors a
    // bogus single-file `rt.go` from the Rust tree and `go build` fails with a
    // wall of `undefined: rt.*`. Strip it for the Go subprocess only; the tool's
    // own env keeps it for the skyc fallback.
    let build = Command::new(oracle_bin)
        .arg("build")
        .arg(MAIN_SKY)
        .current_dir(scratch)
        .env_remove("SKY_RUNTIME_DIR")
        .output()
        .map_err(|e| format!("cannot spawn Go `sky build`: {e}"))?;
    if !build.status.success() {
        // The Go reference prints its formatted diagnostics to stdout (the parse
        // / type errors) AND plain `go build` errors to stderr. Capture both so
        // the recorded divergence note documents *why* Go could not produce a
        // reference, not just the exit code.
        let stdout = String::from_utf8_lossy(&build.stdout);
        let stderr = String::from_utf8_lossy(&build.stderr);
        let diag = format!("{stdout}{stderr}");
        let diag = diag.trim();
        return Err(format!(
            "Go `sky build` exited {:?}: {diag}",
            build.status.code(),
        ));
    }

    let app = scratch.join("sky-out").join("app");
    if !app.exists() {
        return Err(format!("Go build produced no binary at {}", app.display()));
    }
    let run = Command::new(&app)
        .output()
        .map_err(|e| format!("cannot run Go binary: {e}"))?;
    Ok(RunResult {
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        exit_code: run.status.code(),
    })
}

/// Build the golden with skyc and run it, returning its stdout + exit code.
/// Used only on the Go-divergence branch, to source the correct expected value.
fn run_skyc(name: &str, main_sky: &Path, scratch: &Path) -> Result<RunResult, String> {
    let runtime = skyc::resolve_runtime().map_err(|e| format!("runtime resolve failed: {e:?}"))?;
    let emitted = scratch.join("skyc-emit");
    let _ = std::fs::remove_dir_all(&emitted);
    skyc::build(main_sky, &emitted, &runtime).map_err(|e| format!("skyc build failed: {e:?}"))?;
    build_and_run_rust(name, &emitted)
}

/// Capture an oracle value for a single golden, routing a Go failure to the
/// divergence branch rather than caching it as correct.
fn capture(name: &str, golden_dir: &Path, oracle_bin: &str) -> Result<Capture, String> {
    let main_sky = golden_dir.join(MAIN_SKY);
    if !main_sky.exists() {
        return Err(format!("{name}: no {MAIN_SKY} at {}", main_sky.display()));
    }
    let scratch = std::env::temp_dir().join(format!("refresh_oracle_{name}"));
    let _ = std::fs::remove_dir_all(&scratch);

    // Sanctioned-divergence goldens deliberately differ from a SUCCEEDING Go
    // oracle (e.g. full-Unicode case mapping). Record skyc's output as the
    // reference without requiring — or even running — the Go oracle.
    if let Some(reason) = oracle::sanctioned_reason(golden_dir)? {
        let prefixed = format!("{}{reason}", oracle::SANCTIONED_PREFIX);
        return divergence_from_skyc(name, &main_sky, &scratch, prefixed);
    }

    match run_go_oracle(oracle_bin, &main_sky, &scratch) {
        Ok(go) if go.exit_code == Some(0) => Ok(Capture::Go {
            stdout: go.stdout,
            exit_code: 0,
        }),
        Ok(go) => {
            // Go BUILT but the program exited non-zero. Treat a non-zero exit as
            // a Go-side failure on this shape and fall back to skyc, so a buggy
            // Go runtime crash is never enshrined as the expected value.
            let reason = format!(
                "Go oracle program exited {:?} (non-zero); using skyc output as the reference",
                go.exit_code
            );
            divergence_from_skyc(name, &main_sky, &scratch, reason)
        }
        Err(go_err) => {
            let reason = format!("Go oracle failed: {go_err}; using skyc output as the reference");
            divergence_from_skyc(name, &main_sky, &scratch, reason)
        }
    }
}

fn divergence_from_skyc(
    name: &str,
    main_sky: &Path,
    scratch: &Path,
    reason: String,
) -> Result<Capture, String> {
    let skyc = run_skyc(name, main_sky, scratch).map_err(|e| {
        format!("{name}: Go oracle diverged AND skyc could not produce a reference: {e}")
    })?;
    let exit_code = skyc.exit_code.unwrap_or(-1);
    Ok(Capture::Divergence {
        stdout: skyc.stdout,
        exit_code,
        reason,
    })
}

/// Write `expected_go.txt` + `oracle.meta` for one golden.
fn write_oracle(
    golden_dir: &Path,
    main_sky_sha256: String,
    go_sky_version: String,
    cap: &Capture,
) -> Result<bool, String> {
    let (stdout, exit_code, oracle_divergence, divergence_reason) = match cap {
        Capture::Go { stdout, exit_code } => (stdout, *exit_code, false, None),
        Capture::Divergence {
            stdout,
            exit_code,
            reason,
        } => (stdout, *exit_code, true, Some(reason.clone())),
    };
    let meta = Meta {
        main_sky_sha256,
        go_sky_version,
        exit_code,
        oracle_divergence,
        divergence_reason,
    };
    std::fs::write(golden_dir.join(EXPECTED_FILE), stdout)
        .map_err(|e| format!("cannot write {EXPECTED_FILE}: {e}"))?;
    std::fs::write(golden_dir.join(META_FILE), meta.serialize())
        .map_err(|e| format!("cannot write {META_FILE}: {e}"))?;
    Ok(oracle_divergence)
}

/// Names of all already-registered runnable goldens (those carrying an
/// `oracle.meta`), sorted for deterministic processing.
fn registered_goldens(golden_root: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let entries =
        std::fs::read_dir(golden_root).map_err(|e| format!("cannot read golden root: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read golden entry: {e}"))?;
        let path = entry.path();
        if path.join(META_FILE).exists()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn refresh_one(name: &str, golden_root: &Path, oracle_bin: &str) -> Result<(), String> {
    let golden_dir = golden_root.join(name);
    let main_sky = golden_dir.join(MAIN_SKY);
    let source = std::fs::read(&main_sky)
        .map_err(|e| format!("{name}: cannot read {}: {e}", main_sky.display()))?;
    let sha = sha256_hex(&source);
    let version = go_version(oracle_bin);

    let cap = capture(name, &golden_dir, oracle_bin)?;
    let _ = write_oracle(&golden_dir, sha, version, &cap)?;
    match &cap {
        Capture::Go { .. } => eprintln!("  {name}: refreshed (Go oracle cached)"),
        Capture::Divergence { reason, .. } if reason.starts_with(oracle::SANCTIONED_PREFIX) => {
            eprintln!(
                "  {name}: refreshed (SANCTIONED divergence — Sky-Rust output cached deliberately; Go succeeds)"
            );
        }
        Capture::Divergence { .. } => eprintln!(
            "  {name}: refreshed (Go-bug divergence — skyc output cached, Go oracle is buggy here)"
        ),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: refresh-oracle <name> [<name> ...] | --all");
        return ExitCode::FAILURE;
    }

    let root = repo_root();
    let groot = golden_root(&root);
    let oracle_bin = go_oracle();

    let names: Vec<String> = if args.iter().any(|a| a == "--all") {
        match registered_goldens(&groot) {
            Ok(n) if n.is_empty() => {
                eprintln!(
                    "--all: no registered goldens (none carry {META_FILE}); register one by name first"
                );
                return ExitCode::FAILURE;
            }
            Ok(n) => n,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        args
    };

    eprintln!("refresh-oracle: {} golden(s)", names.len());
    let mut failures = 0u32;
    for name in &names {
        if let Err(e) = refresh_one(name, &groot, &oracle_bin) {
            eprintln!("  FAILED {name}: {e}");
            failures += 1;
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        eprintln!("refresh-oracle: {failures} golden(s) FAILED");
        ExitCode::FAILURE
    }
}
