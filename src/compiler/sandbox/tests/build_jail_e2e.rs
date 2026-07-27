//! End-to-end proof of the returning build jail + the per-axis differential
//! confinement contract — the Tier-2 observation primitive at the kernel
//! boundary.
//!
//! These are REAL jailed runs (they spawn `bwrap`) of the admission probe
//! fixture (`tests/fixtures/admission/untrusted-build.sh`, `PROBE_MODE=tier2`),
//! so they are gated behind `IPE_E2E=1` and skip cleanly when the jail cannot be
//! established here — either because bubblewrap / the cap helpers are absent, or
//! because the environment forbids establishing the jail (a locked-down runner
//! where `bwrap` is present but the kernel denies the namespace setup). A
//! one-time canary (`/bin/true` under the most-isolated profile) settles this;
//! the tests early-return exactly as they do when the tools are missing, never a
//! false pass.
//!
//! What they prove directly at the OS boundary, not by the payload's own choice:
//!
//! - **Denied names the axis.** Under a jail scoped to a DECLARED set that
//!   withholds `network`, the probe's socket attempt is denied → the fixture's
//!   wrapper-owned exit code decodes to [`JailOutcome::Denied`] naming
//!   `network`. Likewise an out-of-scratch write under a filesystem-withholding
//!   jail decodes to `Denied { filesystem }`.
//! - **Clean requires positive proof.** A benign in-scratch write under the
//!   declared-scoped jail exits 0 → [`JailOutcome::Clean`]. The one admit-eligible
//!   outcome only ever comes from the probe's clean exit.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
// Integration harness: `expect`/`unwrap` on setup make a mis-set-up test fail
// loudly (correct for a test); indexing mirrors the sibling `run_jail_e2e`.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::redundant_closure_for_method_calls
)]

use std::ffi::OsString;
use std::path::PathBuf;

use ipe_sandbox::build_jail::{CapabilityAxis, JailOutcome, build_in_jail};
use ipe_sandbox::run_jail::{FilesystemScope, RunJailTools, SandboxProfile};

/// The `build_in_jail` seccomp path creates a `memfd` and clears its
/// close-on-exec flag — a process-global fd-table mutation. Serialize the jailed
/// runs so parallel `--test-threads` cannot race on fd numbers.
static JAIL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The absolute path to the admission probe fixture, from the workspace root
/// this crate is built within.
fn fixture_path() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `.../src/compiler/sandbox`; the fixture lives at
    // the repo root under `tests/fixtures/admission/`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/admission/untrusted-build.sh")
}

/// Skip unless `IPE_E2E=1`, the jail tools are present, AND a jail can actually
/// be established here. Mirrors the sibling `run_jail_e2e` gate: tool presence
/// alone is insufficient (a runner may have `bwrap` but deny the namespace
/// setup), so a `/bin/true` canary under the isolated profile decides once.
fn e2e_tools() -> Option<RunJailTools> {
    if std::env::var_os("IPE_E2E").is_none_or(|v| v != "1") {
        return None;
    }
    let caps = ipe_sandbox::probe();
    let tools = RunJailTools {
        bwrap: caps.bwrap?,
        prlimit: caps.prlimit?,
        timeout: caps.timeout,
    };
    if !jail_can_establish(&tools) {
        return None;
    }
    Some(tools)
}

/// One-time canary: `/bin/true` under the most-isolated profile must exit
/// `Clean`. Any other outcome means the jail could not be established here → the
/// tests skip rather than fail on a broken environment.
fn jail_can_establish(tools: &RunJailTools) -> bool {
    static CANARY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CANARY.get_or_init(|| {
        let scoped = fresh_scratch("canary");
        let _guard = JAIL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = build_in_jail(
            tools,
            &SandboxProfile::maximally_isolated(),
            &scoped,
            &scoped,
            &ro_binds(),
            &[OsString::from("/bin/true")],
        );
        let _ = std::fs::remove_dir_all(&scoped);
        let established = outcome.is_clean();
        if !established {
            eprintln!("build_jail_e2e: skipping — jail cannot be established here ({outcome:?})");
        }
        established
    })
}

/// A fresh per-test scratch dir under the system temp root (the one writable
/// mount inside the jail).
fn fresh_scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipe-build-jail-e2e-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("scoped tmp");
    dir
}

/// The interpreters/tools the fixture needs, re-exposed read-only past the
/// home/tmp tmpfs masks.
fn ro_binds() -> Vec<PathBuf> {
    [
        PathBuf::from("/usr"),
        PathBuf::from("/bin"),
        PathBuf::from("/lib"),
    ]
    .into_iter()
    .filter(|p| p.exists())
    .collect()
}

/// Run the admission fixture in `tier2` mode inside a jail scoped to `profile`,
/// exercising exactly `axis`, and return the decoded outcome. `escape_path` is
/// the fs-escape probe target (inside the scratch for a Clean run, outside it
/// for a denial).
fn run_fixture(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped: &PathBuf,
    axis: &str,
    escape_path: &str,
) -> JailOutcome {
    // The env the fixture reads. `run_jail_argv` scrubs the env with
    // `--clearenv`, re-exporting only the profile's `env_allowlist` (plus
    // PATH/TMPDIR/LANG). So the probe contract vars must be on the allowlist to
    // survive the scrub — this test sets them via a profile whose env_allowlist
    // names them and a host_env-backed run. `build_in_jail` reads the real host
    // env, so export them into this process first.
    unsafe {
        std::env::set_var("PROBE_MODE", "tier2");
        std::env::set_var("TIER2_AXIS", axis);
        std::env::set_var("SCRATCH_DIR", scoped);
        std::env::set_var("ESCAPE_PATH", escape_path);
    }
    let profile = SandboxProfile {
        env_allowlist: vec![
            "PROBE_MODE".to_owned(),
            "TIER2_AXIS".to_owned(),
            "SCRATCH_DIR".to_owned(),
            "ESCAPE_PATH".to_owned(),
        ],
        ..profile.clone()
    };
    // The fixture lives in the repo tree, which the jail masks (only the scratch
    // mount and the ro tool binds are visible inside). Copy it into the scratch —
    // the one writable mount — so the jailed shell can read it.
    let jailed_fixture = scoped.join("untrusted-build.sh");
    std::fs::copy(fixture_path(), &jailed_fixture).expect("copy fixture into scratch");
    let payload: Vec<OsString> = vec![OsString::from("/bin/sh"), jailed_fixture.into_os_string()];
    let _guard = JAIL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    build_in_jail(tools, &profile, scoped, scoped, &ro_binds(), &payload)
}

/// The declared-scoped profile a real Tier-2 probe runs under: `subprocess`
/// granted so the probe can fork its helper (python3/nc/rm), with the axis under
/// test withheld. The withheld axis — not the ability to spawn a helper — is what
/// the probe must observe a denial on.
fn probe_profile(network: bool, filesystem: FilesystemScope) -> SandboxProfile {
    SandboxProfile {
        network,
        filesystem,
        subprocess: true,
        ..SandboxProfile::maximally_isolated()
    }
}

#[test]
fn a_socket_under_a_network_withholding_jail_is_denied_naming_network() {
    let Some(tools) = e2e_tools() else { return };
    // Declared set grants `subprocess` (the probe forks python3 to open a socket)
    // but WITHHOLDS `network` → the socket is denied by the empty net namespace →
    // Denied { network }.
    let scoped = fresh_scratch("net-denied");
    let outcome = run_fixture(
        &tools,
        &probe_profile(false, FilesystemScope::Isolated),
        &scoped,
        "network",
        &format!("{}/escape", scoped.display()),
    );
    let _ = std::fs::remove_dir_all(&scoped);
    assert_eq!(
        outcome,
        JailOutcome::Denied {
            axis: CapabilityAxis::Network
        },
        "a withheld-network jail must deny the socket and name the network axis"
    );
}

#[test]
fn an_out_of_scratch_write_under_a_filesystem_withholding_jail_is_denied_naming_filesystem() {
    let Some(tools) = e2e_tools() else { return };
    // Declared set grants nothing but the scratch mount (isolated filesystem) →
    // an out-of-scratch write (to a read-only host path) is denied → Denied
    // { filesystem }. `/usr/...` is read-only under `--ro-bind / /`.
    let scoped = fresh_scratch("fs-denied");
    let outcome = run_fixture(
        &tools,
        &SandboxProfile::maximally_isolated(),
        &scoped,
        "filesystem",
        "/usr/ipe-tier2-escape-probe",
    );
    let _ = std::fs::remove_dir_all(&scoped);
    assert_eq!(
        outcome,
        JailOutcome::Denied {
            axis: CapabilityAxis::Filesystem
        },
        "a withheld-filesystem jail must deny the out-of-scratch write and name the filesystem axis"
    );
}

#[test]
fn a_benign_in_scratch_write_is_clean() {
    let Some(tools) = e2e_tools() else { return };
    // The fs-escape target is INSIDE the scratch (the one writable mount), so the
    // write succeeds and no axis is demanded-but-withheld → Clean. This is the
    // only admit-eligible outcome and requires the probe's positive clean exit.
    let scoped = fresh_scratch("clean");
    let escape = format!("{}/in-scratch-write", scoped.display());
    let outcome = run_fixture(
        &tools,
        &probe_profile(false, FilesystemScope::Isolated),
        &scoped,
        "filesystem",
        &escape,
    );
    let _ = std::fs::remove_dir_all(&scoped);
    assert_eq!(
        outcome,
        JailOutcome::Clean,
        "a benign in-scratch build must be Clean (the only admit-eligible outcome)"
    );
    assert!(outcome.is_clean());
}

#[test]
fn a_missing_fixture_is_a_non_clean_outcome_fail_closed() {
    let Some(tools) = e2e_tools() else { return };
    // Running a payload that cannot exec (a path that does not exist) must never
    // be Clean — the jail establishes but the payload fails, so the outcome is a
    // non-clean BuildFailed (or the spawn refuses). Fail-closed either way.
    let scoped = fresh_scratch("missing");
    let outcome = {
        let _guard = JAIL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        build_in_jail(
            &tools,
            &SandboxProfile::maximally_isolated(),
            &scoped,
            &scoped,
            &ro_binds(),
            &[
                OsString::from("/bin/sh"),
                OsString::from("/nonexistent/probe.sh"),
            ],
        )
    };
    let _ = std::fs::remove_dir_all(&scoped);
    assert!(
        !outcome.is_clean(),
        "a payload that cannot run must never be Clean, got {outcome:?}"
    );
}
