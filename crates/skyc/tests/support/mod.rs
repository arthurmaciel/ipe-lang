//! Shared end-to-end build/run support for the golden parity gate.
//!
//! Every golden's emitted Rust project is built into ONE shared cargo target —
//! the machine-global `~/.cache/sky-rust-target`, configured in the global
//! `~/.cargo/config.toml` (`target-dir = …`). cargo reads that file on every
//! invocation, including builds launched from an emitted project under
//! `std::env::temp_dir()`, so heavy dependencies (tokio / rsa / serde / …)
//! compile ONCE and are reused across all goldens and across runs. The target
//! is never deleted; reuse is the whole point.
//!
//! To let the per-golden root binaries coexist in that single target without
//! clobbering one another, this helper rewrites the emitted manifest to a unique
//! package (and therefore binary) name per golden before building.
//!
//! The produced binary is located ROBUSTLY by parsing
//! `cargo build --message-format=json` for the artifact's `executable` field —
//! never a hard-coded `target/debug/<name>` path, which would silently break the
//! moment the target dir moves (exactly the breakage the per-test
//! `CARGO_TARGET_DIR` override existed to paper over).
//!
//! Rigour: a build failure FAILS the test (the build assert carries cargo's
//! stderr); it is never skipped and never reported as a false green.
//!
//! The build/run plumbing and the cached-oracle format both live in the shared
//! [`oracle`] crate so the `refresh-oracle` tool and these tests cannot drift —
//! the tool WRITES the cache via the same code the tests READ it through.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Outcome of building and running an emitted Sky project.
// Fields are only accessed from E2E test functions (`build_and_run_emitted`
// callers), which are absent from test binaries that skip the E2E suite (e.g.
// `golden_mm`).  `build_and_run_emitted` itself carries its own allow;
// the struct-level allow keeps the field warnings silent in those binaries.
#[allow(dead_code)]
pub struct RunOutcome {
    /// The program's standard output, decoded lossily from UTF-8.
    pub stdout: String,
    /// The process exit code (`None` if the process was killed by a signal).
    pub exit_code: Option<i32>,
}

/// Build the emitted project at `emitted_dir` into the shared target and run the
/// resulting binary, returning its captured stdout and exit code.
///
/// Delegates to [`oracle::build_and_run_rust`] (the same core the refresh tool
/// uses) and wraps its `Result` in a test assertion.
///
/// # Panics
///
/// Fails the calling test (via `assert!`) if the manifest cannot be retargeted,
/// if `cargo build` fails (surfacing cargo's stderr), if the produced binary
/// cannot be located in the JSON output, or if the binary cannot be executed.
/// It never returns a placeholder on a green path — a broken golden cannot pass.
#[must_use]
#[allow(dead_code)] // not every golden test binary exercises every helper
pub fn build_and_run_emitted(golden_name: &str, emitted_dir: &Path) -> RunOutcome {
    let result = oracle::build_and_run_rust(golden_name, emitted_dir);
    assert!(
        result.is_ok(),
        "{}",
        result.as_ref().err().map_or("", String::as_str)
    );
    let Ok(result) = result else {
        // Unreachable on a green path: the assert above already failed the test.
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    RunOutcome {
        stdout: result.stdout,
        exit_code: result.exit_code,
    }
}

/// Build the emitted project and run its binary with `stdin_bytes` piped to its
/// stdin, then closed (signalling EOF), returning its captured stdout and exit
/// code.
///
/// Used by goldens that drive an interactive/line-oriented loop (e.g.
/// `Cli.program`) past its first stdin read, which [`build_and_run_emitted`]
/// cannot exercise since it runs the binary with stdin already at EOF.
///
/// # Panics
/// Fails the calling test if the binary cannot be located (surfacing cargo's
/// stderr), or cannot be spawned with a piped stdin.
#[must_use]
#[allow(dead_code)] // only stdin-driven goldens exercise this helper
pub fn build_and_run_emitted_with_stdin(
    golden_name: &str,
    emitted_dir: &Path,
    stdin_bytes: &[u8],
) -> RunOutcome {
    let exe = oracle::build_rust_binary(golden_name, emitted_dir);
    assert!(
        exe.is_ok(),
        "{}",
        exe.as_ref().err().map_or("", String::as_str)
    );
    let Ok(exe) = exe else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };

    let child = Command::new(&exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    assert!(
        child.is_ok(),
        "{golden_name}: failed to spawn `{exe}`: {:?}",
        child.as_ref().err()
    );
    let Ok(mut child) = child else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };

    let stdin = child.stdin.take();
    assert!(stdin.is_some(), "{golden_name}: child stdin must be piped");
    let Some(mut stdin) = stdin else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    let wrote = stdin.write_all(stdin_bytes);
    assert!(wrote.is_ok(), "{golden_name}: failed to write stdin: {wrote:?}");
    drop(stdin); // close stdin so the child's reader sees EOF after these bytes

    let output = child.wait_with_output();
    assert!(
        output.is_ok(),
        "{golden_name}: failed to wait on `{exe}`: {:?}",
        output.as_ref().err()
    );
    let Ok(output) = output else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    RunOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        exit_code: output.status.code(),
    }
}

/// Build the emitted project and run its binary with the MAIN-THREAD stack
/// capped to `stack_kib` KiB, via `bash -c 'ulimit -s <kib>; exec "$0"' <bin>`.
///
/// A deep non-TCO recursion then overflows deterministically at a few thousand
/// frames instead of needing ~10^6, so the constant-stack proof is fast and
/// robust. A stack overflow trips the guard page and `abort()`s (SIGABRT) — NOT
/// a catchable panic — so the child dies by signal and `exit_code` is `None`;
/// the TCO'd binary instead exits cleanly with `Some(0)`. Linux/macOS only (the
/// Rust backend's target).
///
/// # Panics
/// Fails the calling test if `cargo build` fails (surfacing cargo's stderr), the
/// binary cannot be located, or the `bash`/`ulimit` runner cannot be spawned.
#[must_use]
#[allow(dead_code)] // only the constant-stack golden exercises this helper
pub fn build_and_run_stack_limited(
    golden_name: &str,
    emitted_dir: &Path,
    stack_kib: u32,
) -> RunOutcome {
    let exe = oracle::build_rust_binary(golden_name, emitted_dir);
    assert!(
        exe.is_ok(),
        "{}",
        exe.as_ref().err().map_or("", String::as_str)
    );
    let Ok(exe) = exe else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    // `exec "$0"` replaces the shell after `ulimit -s` lowers the soft stack
    // limit, so the child binary runs under the capped main-thread stack.
    let output = Command::new("bash")
        .arg("-c")
        .arg(format!("ulimit -s {stack_kib}; exec \"$0\""))
        .arg(&exe)
        .output();
    assert!(
        output.is_ok(),
        "{golden_name}: failed to spawn stack-limited runner: {:?}",
        output.as_ref().err()
    );
    let Ok(output) = output else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    RunOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        exit_code: output.status.code(),
    }
}

/// Assert skyc's stdout matches the golden's CACHED Go oracle, with the
/// staleness gate enforced first.
///
/// This is the read side of the cached-oracle infra: it NEVER runs the Go
/// backend. It re-hashes `tests/golden/<name>/Main.sky` and, if the hash no
/// longer matches `oracle.meta`, fails loudly with "run refresh-oracle" rather
/// than diffing against a stale `expected_go.txt`. A missing oracle is likewise
/// a hard failure — never a skip. When the golden is marked `oracle_divergence`
/// (the Go oracle fails on this shape, or we follow a different target), the
/// comparison is against skyc's own recorded-correct output.
#[allow(dead_code)] // exercised by the goldens that opt into cached parity
pub fn assert_go_parity(golden_name: &str, golden_dir: &Path, skyc_stdout: &str) {
    let outcome = oracle::check_parity(golden_dir, golden_name, skyc_stdout);
    assert!(
        outcome.is_ok(),
        "{}",
        outcome.err().map_or(String::new(), |e| e.to_string())
    );
}
