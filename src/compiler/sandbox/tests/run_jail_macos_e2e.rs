//! End-to-end proof of the macOS run jail's Seatbelt containment — the SEAL's
//! security half on macOS, mirroring the Linux `run_jail_e2e`.
//!
//! These are REAL jailed runs (they spawn `sandbox-exec`), so they are gated
//! behind `IPE_E2E=1` and only compile on macOS. They drive the RUN jail through
//! the SAME [`ipe_sandbox::build_jail::sbpl_from_profile`] the production
//! `exec_in_run_jail` macOS arm uses — there is ONE SBPL source, so what these
//! assert is exactly what confines the shipped app at run time.
//!
//! The load-bearing property is the enforce-vs-control DUALITY: a
//! capability action DENIED under the jail AND the SAME action SUCCEEDING under
//! control (no jail). The control run rules out a false pass from an unreachable
//! host or an already-unwritable path — a denial under enforce can then only be
//! the jail's.
//!
//! - **Fail-closed on the undeclared.** A network connect and an out-of-scratch
//!   write from a maximally-isolated (network- and filesystem-absent) jail are
//!   both denied by the SBPL.
//! - **No false-deny.** The same network connect from a network-GRANTED jail is
//!   not denied; a write into the always-writable scratch succeeds under any
//!   profile.

#![cfg(target_os = "macos")]
// This is an integration test harness: `expect`/`unwrap` on setup steps make a
// mis-set-up test fail loudly (the correct behavior for a test).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::Path;
use std::process::Command;

use ipe_sandbox::build_jail::sbpl_from_profile;
use ipe_sandbox::run_jail::SandboxProfile;

/// Skip unless `IPE_E2E=1` and `sandbox-exec` is present. An absent primitive on
/// a macOS runner is a skip here (the CI job asserts its presence separately as a
/// hard, refuse-to-certify failure), never a silent green.
fn e2e_enabled() -> bool {
    if std::env::var_os("IPE_E2E").is_none_or(|v| v != "1") {
        return false;
    }
    which_sandbox_exec().is_some()
}

fn which_sandbox_exec() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("sandbox-exec"))
        .find(|candidate| candidate.is_file())
}

/// Write the profile's SBPL to a scratch file and run `sandbox-exec -f <sbpl> sh
/// -c <script>`, returning the exit code (`None` if signalled).
fn run_jailed(profile: &SandboxProfile, scratch: &Path, script: &str) -> Option<i32> {
    let sbpl = sbpl_from_profile(profile, scratch, scratch);
    let sbpl_file = scratch.join("ipe-run-e2e.sb");
    std::fs::write(&sbpl_file, sbpl.as_bytes()).expect("write sbpl");
    let sandbox_exec = which_sandbox_exec().expect("sandbox-exec present");
    Command::new(sandbox_exec)
        .arg("-f")
        .arg(&sbpl_file)
        .arg("sh")
        .arg("-c")
        .arg(script)
        .status()
        .expect("spawn sandbox-exec")
        .code()
}

/// Run the same script unjailed — the control half of the duality.
fn run_control(script: &str) -> Option<i32> {
    Command::new("sh")
        .arg("-c")
        .arg(script)
        .status()
        .expect("spawn control")
        .code()
}

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ipe-run-macos-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn isolated() -> SandboxProfile {
    SandboxProfile::maximally_isolated()
}

fn net_granted() -> SandboxProfile {
    SandboxProfile {
        network: true,
        ..SandboxProfile::maximally_isolated()
    }
}

#[test]
fn undeclared_network_is_denied_under_the_run_jail_but_succeeds_under_control() {
    if !e2e_enabled() {
        return;
    }
    let scratch = scratch_dir();
    // A TCP connect to a public resolver. `nc -z` returns non-zero when the
    // socket is denied. The control must succeed (proving the host has a route)
    // or the test is inconclusive and skips.
    let connect = "nc -z -G 5 1.1.1.1 53";
    if run_control(connect) != Some(0) {
        let _ = std::fs::remove_dir_all(&scratch);
        return; // no outbound route on this runner — nothing to prove.
    }
    let jailed = run_jailed(&isolated(), &scratch, connect);
    let _ = std::fs::remove_dir_all(&scratch);
    assert_ne!(
        jailed,
        Some(0),
        "a network-withholding run jail must DENY the connect (control succeeded)"
    );
}

#[test]
fn declared_network_reaches_the_network_under_the_run_jail() {
    if !e2e_enabled() {
        return;
    }
    let scratch = scratch_dir();
    let connect = "nc -z -G 5 1.1.1.1 53";
    if run_control(connect) != Some(0) {
        let _ = std::fs::remove_dir_all(&scratch);
        return;
    }
    let jailed = run_jailed(&net_granted(), &scratch, connect);
    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(
        jailed,
        Some(0),
        "a network-granted run jail must reach the network (no false-deny)"
    );
}

#[test]
fn an_out_of_scratch_write_is_denied_under_the_run_jail_but_succeeds_under_control() {
    if !e2e_enabled() {
        return;
    }
    let scratch = scratch_dir();
    // The escape target is a sibling of the scratch, NOT under it — the SBPL's
    // blanket file-write* denial withholds it under a filesystem-absent profile.
    let escape = scratch.with_extension("escape-probe");
    let escape_disp = escape.display();
    let write_escape = format!("echo x > '{escape_disp}'");
    // Control: the same write must succeed unjailed (proving the path is writable
    // and the denial under enforce is the jail's, not a pre-unwritable path).
    let control = run_control(&write_escape);
    let _ = std::fs::remove_file(&escape);
    if control != Some(0) {
        let _ = std::fs::remove_dir_all(&scratch);
        return;
    }
    let jailed = run_jailed(&isolated(), &scratch, &write_escape);
    let _ = std::fs::remove_file(&escape);
    let _ = std::fs::remove_dir_all(&scratch);
    assert_ne!(
        jailed,
        Some(0),
        "a filesystem-withholding run jail must DENY an out-of-scratch write (control succeeded)"
    );
}

#[test]
fn an_in_scratch_write_succeeds_under_the_run_jail() {
    if !e2e_enabled() {
        return;
    }
    let scratch = scratch_dir();
    let inside = scratch.join("ok.txt");
    let inside_disp = inside.display();
    let jailed = run_jailed(&isolated(), &scratch, &format!("echo x > '{inside_disp}'"));
    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(
        jailed,
        Some(0),
        "a write into the always-writable scratch must succeed (no false-deny)"
    );
}
