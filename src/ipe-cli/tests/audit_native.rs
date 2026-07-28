#![forbid(unsafe_code)]
//! Tier-2 native-code capability enforcement (ADR 0046).
//!
//! Two layers:
//!
//! - **CLI-level** (always runs): a pure Ipê package skips Tier-2 (with a note)
//!   while Tier-1 still fully gates it; a native-bearing package with no
//!   probeable entrypoint is rejected by the Tier-2 check (fail-closed, never a
//!   silent clean).
//! - **Real-jail differential confinement** (gated on Linux + `IPE_E2E=1`):
//!   drives the reconciler through the REAL jail against the admission probe
//!   fixture, proving at the OS boundary that a used-but-undeclared axis rejects
//!   naming the axis, and a benign package declaring exactly its axes is
//!   accepted. Skips cleanly (never a false pass) where the jail cannot be
//!   established, mirroring the sandbox crate's `build_jail_e2e`.

// A failed `expect` in test setup IS the failure signal the harness reports.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

// ===========================================================================
// CLI-level: pure-Ipê skip (Tier-1 still gates) + native-bearing fail-closed
// ===========================================================================

fn temp_pkg(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipe-tier2-test-{}-{}-{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create temp src dir");
    dir
}

fn empty_index(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipe-tier2-index-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("packages")).expect("create index packages dir");
    dir
}

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn run_audit(pkg: &Path, index: &Path) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .arg("package")
        .arg("audit")
        .arg(pkg)
        .arg("--index")
        .arg(index)
        .current_dir(repo_root())
        .output()
        .expect("run ipe package audit");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const PURE_MAIN: &str = "module Main exposing (main)\n\
                         \n\
                         import Ipe.String as String\n\
                         import Ipe.Io as Io\n\
                         \n\
                         main : Task ()\n\
                         main =\n\
                         \x20   Io.println (String.toUpper \"hello\")\n";

#[test]
fn a_pure_ipe_package_skips_tier2_while_tier1_still_gates() {
    // A pure Ipê package: Tier-2 does not apply (skips), and Tier-1 still runs to
    // completion (the pass line is printed). The honest surface says Tier-2 does
    // not apply — it never claims a Tier-2 certification for a package it skipped.
    let pkg = temp_pkg("pure-skip");
    std::fs::write(
        pkg.join("ipe.toml"),
        "name = \"pure-pkg\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n",
    )
    .expect("write ipe.toml");
    std::fs::write(pkg.join("src").join("Main.ipe"), PURE_MAIN).expect("write Main");
    let index = empty_index("pure-skip");

    let (ok, stdout, stderr) = run_audit(&pkg, &index);
    assert!(
        ok,
        "a pure package must pass; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("all Tier-1 checks passed"),
        "Tier-1 still runs to completion; got:\n{stdout}"
    );
    assert!(
        stdout.contains("native Tier-2 does not apply"),
        "the honest surface says Tier-2 does not apply for a pure package; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("passed on: linux-x64"),
        "a pure package must never claim a Tier-2 certification; got:\n{stdout}"
    );
}

#[test]
fn a_native_bearing_package_with_no_probeable_entrypoint_fails_closed() {
    // A package binding a Rust dependency is native-bearing. With no per-package
    // probe entrypoint wired, Tier-2 refuses to certify it (fail-closed) rather
    // than admit it un-observed — the honest-surface rule (never claim a
    // certification the check did not actually earn).
    let pkg = temp_pkg("native-noprobe");
    std::fs::write(
        pkg.join("ipe.toml"),
        "name = \"native-pkg\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n\n\
         [rust.dependencies]\nlibc = \"0.2\"\n",
    )
    .expect("write ipe.toml");
    std::fs::write(pkg.join("src").join("Main.ipe"), PURE_MAIN).expect("write Main");
    let index = empty_index("native-noprobe");

    let (ok, stdout, stderr) = run_audit(&pkg, &index);
    assert!(
        !ok,
        "a native-bearing package with no probe entrypoint must fail closed; \
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("native Tier-2 capability enforcement"),
        "the reject names the Tier-2 check; got:\n{stderr}"
    );
    assert!(
        stderr.contains("no capability-probe entrypoint") || stderr.contains("cannot exercise"),
        "the diagnostic explains the un-exercised fail-closed reject; got:\n{stderr}"
    );
}

// ===========================================================================
// Real-jail differential confinement (wired platforms + IPE_E2E=1)
//
// On Linux/x86_64 the jail is bwrap+seccomp; on macOS it is sandbox-exec. The
// reconciler and the fixture are the SAME on both — only the jail primitive
// probed in `e2e_tools` differs.
// ===========================================================================

#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
mod real_jail {
    use super::PathBuf;
    use std::collections::BTreeSet;
    use std::ffi::OsString;

    use ipe::audit::Check;
    use ipe::audit_native::{
        CERTIFIED_PLATFORM, JailProbeRunner, ProbeExercise, ProbeRunner, StaticReachability,
        TightenableAxis, default_ro_binds, reconcile_native, scoped_profile,
    };
    use ipe_ir::Capability;
    use ipe_sandbox::build_jail::build_in_jail;
    use ipe_sandbox::run_jail::{RunJailTools, SandboxProfile};

    /// `build_in_jail` mutates the process-global fd table (a `memfd`) on Linux;
    /// serialize the jailed runs so parallel `--test-threads` cannot race.
    static JAIL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/admission/untrusted-build.sh")
    }

    /// Probe the host for the jail primitive, returning the [`RunJailTools`] the
    /// jail is built from (bwrap+prlimit on Linux; sandbox-exec on macOS, whose
    /// fields the jail ignores). `None` when the primitive is absent.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn probe_tools() -> Option<RunJailTools> {
        let caps = ipe_sandbox::probe();
        Some(RunJailTools {
            bwrap: caps.bwrap?,
            prlimit: caps.prlimit?,
            timeout: caps.timeout,
        })
    }

    /// macOS: the jail primitive is `sandbox-exec`; the `RunJailTools` fields are
    /// unused by the macOS `build_in_jail` (a present-primitive placeholder).
    #[cfg(target_os = "macos")]
    fn probe_tools() -> Option<RunJailTools> {
        let path = std::env::var_os("PATH")?;
        let sandbox_exec = std::env::split_paths(&path)
            .map(|d| d.join("sandbox-exec"))
            .find(|p| p.is_file())?;
        Some(RunJailTools {
            bwrap: sandbox_exec.clone(),
            prlimit: sandbox_exec,
            timeout: None,
        })
    }

    /// Skip unless `IPE_E2E=1`, the jail primitive is present, AND a jail can
    /// actually be established here (a `/bin/true` canary settles it once) —
    /// mirroring the sandbox crate's gate. Never a false pass.
    fn e2e_tools() -> Option<RunJailTools> {
        if std::env::var_os("IPE_E2E").is_none_or(|v| v != "1") {
            return None;
        }
        let tools = probe_tools()?;
        if !jail_can_establish(&tools) {
            return None;
        }
        Some(tools)
    }

    fn jail_can_establish(tools: &RunJailTools) -> bool {
        static CANARY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *CANARY.get_or_init(|| {
            let scoped = fresh_scratch("canary");
            let _guard = JAIL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let outcome = build_in_jail(
                tools,
                &SandboxProfile::maximally_isolated(),
                &scoped,
                &scoped,
                &default_ro_binds(),
                &[OsString::from("/bin/true")],
            );
            let _ = std::fs::remove_dir_all(&scoped);
            let established = outcome.is_clean();
            if !established {
                eprintln!("audit_native e2e: skipping — jail cannot be established ({outcome:?})");
            }
            established
        })
    }

    fn fresh_scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-tier2-e2e-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn set(caps: &[Capability]) -> BTreeSet<Capability> {
        caps.iter().copied().collect()
    }

    /// A static-reachability stub for the E2E cross-check.
    struct FixedScan {
        reaches: BTreeSet<Capability>,
    }
    impl StaticReachability for FixedScan {
        fn reaches(&self, axis: TightenableAxis) -> bool {
            self.reaches.contains(&axis.capability())
        }
    }

    /// Build a real jail-backed runner exercising `exercised`, holding the two
    /// scratch dirs alive for the run's duration.
    struct Harness {
        scoped_tmp: PathBuf,
        working_tree: PathBuf,
        wrapper: PathBuf,
    }

    impl Harness {
        fn new(tag: &str) -> Self {
            let scoped_tmp = fresh_scratch(&format!("{tag}-scratch"));
            let working_tree = fresh_scratch(&format!("{tag}-worktree"));
            let wrapper = scoped_tmp.join("untrusted-build.sh");
            std::fs::copy(fixture_path(), &wrapper).expect("copy fixture into scratch");
            Self {
                scoped_tmp,
                working_tree,
                wrapper,
            }
        }

        fn runner<'a>(
            &self,
            tools: &'a RunJailTools,
            exercised: Vec<TightenableAxis>,
        ) -> JailProbeRunner<'a> {
            JailProbeRunner::new(
                tools,
                self.wrapper.clone(),
                self.scoped_tmp.clone(),
                self.working_tree.clone(),
                default_ro_binds(),
                exercised,
                ProbeExercise::WrapperProbeOnly,
            )
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.scoped_tmp);
            let _ = std::fs::remove_dir_all(&self.working_tree);
        }
    }

    #[test]
    fn used_but_undeclared_network_rejects_naming_the_axis_standing_canary() {
        // THE SEAL'S RED CANARY: native code opens a socket (exercises network)
        // while declaring `[]`. Under the declared-scoped jail (network withheld)
        // the socket is denied → REJECT naming the network axis. This MUST stay
        // red if the admit predicate ever regresses.
        let Some(tools) = e2e_tools() else { return };
        let declared = BTreeSet::new();
        let scoped = scoped_profile(&declared).expect("lower profile");
        let harness = Harness::new("canary-net");
        let _guard = JAIL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runner = harness.runner(&tools, vec![TightenableAxis::Network]);
        let scan = FixedScan {
            reaches: BTreeSet::new(),
        };
        let verdict = reconcile_native(&declared, &runner, &scan, &scoped);
        let r = verdict.expect_err("a socket under a []-scoped jail must reject");
        assert_eq!(r.check, Check::NativeTier2);
        assert!(
            r.message.contains("network") && r.message.contains("hidden effect"),
            "the reject names the network axis as a hidden effect: {}",
            r.message
        );
    }

    #[test]
    fn used_but_undeclared_filesystem_rejects_naming_filesystem() {
        // Native code writes out-of-scratch (exercises filesystem) while declaring
        // `[]`. Under the declared-scoped jail (filesystem withheld) the write is
        // denied → REJECT naming filesystem.
        let Some(tools) = e2e_tools() else { return };
        let declared = BTreeSet::new();
        let scoped = scoped_profile(&declared).expect("lower profile");
        let harness = Harness::new("canary-fs");
        let _guard = JAIL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runner = harness.runner(&tools, vec![TightenableAxis::Filesystem]);
        let scan = FixedScan {
            reaches: BTreeSet::new(),
        };
        let r = reconcile_native(&declared, &runner, &scan, &scoped)
            .expect_err("an out-of-scratch write under a []-scoped jail must reject");
        assert!(r.message.contains("filesystem"), "{}", r.message);
    }

    #[test]
    fn a_benign_network_package_declaring_exactly_its_axis_is_accepted() {
        // POSITIVE control: declares `network`; native code opens a socket. The
        // declared-scoped jail GRANTS network → the socket reaches → clean. The
        // tightening run (network withheld) DENIES the socket → the axis is needed
        // → not over-broad → ACCEPT. The clean result requires the probe's
        // positive clean exit (not a broken jail), because the same jail denies
        // the socket when network is withheld.
        let Some(tools) = e2e_tools() else { return };
        let declared = set(&[Capability::Network]);
        let scoped = scoped_profile(&declared).expect("lower profile");
        let harness = Harness::new("benign-net");
        let _guard = JAIL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runner = harness.runner(&tools, vec![TightenableAxis::Network]);
        // The static scan reaches network too (the wrapper opens a socket), so a
        // needed axis is never mis-flagged unused.
        let scan = FixedScan {
            reaches: set(&[Capability::Network]),
        };
        reconcile_native(&declared, &runner, &scan, &scoped)
            .expect("a benign network package declaring exactly its axis must be accepted");
    }

    #[test]
    fn the_certified_platform_names_this_host_jail() {
        // The certify label names exactly the wired jail on THIS host, so a
        // certify never claims a platform whose jail did not run.
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(CERTIFIED_PLATFORM, "linux-x64");
        #[cfg(target_os = "macos")]
        assert_eq!(CERTIFIED_PLATFORM, "macos-arm64");
    }

    // ── real untrusted `cargo build` as the wrapper's child (the certify path) ──
    //
    // These drive the SAME reconciler production wires, but with a real
    // `ProbeExercise::RealBuild` running an actual `cargo build` inside the jail as
    // the wrapper's child — proving at the OS boundary that (i) a confined clean
    // build+link genuinely CERTIFIES (the first real certification) and (ii) a
    // build whose `build.rs` reaches a withheld axis REJECTS naming the axis. A
    // reduced purpose-built crate (design §7.2) keeps the always-run canary cheap:
    // no registry deps, so `--offline` needs no vendoring.

    /// Write a minimal, self-contained cargo crate into `dir`. When `net_reach`
    /// is set, its `build.rs` attempts a TCP connect at build time — a genuine,
    /// deterministic network capability demand. Otherwise the build is inert
    /// (reaches no axis) and must certify clean under any scoped jail.
    fn write_min_crate(dir: &std::path::Path, net_reach: bool) {
        std::fs::create_dir_all(dir.join("src")).expect("crate src");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"tier2min\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
             [[bin]]\nname = \"tier2_probe\"\npath = \"src/main.rs\"\n\n[workspace]\n",
        )
        .expect("Cargo.toml");
        std::fs::write(
            dir.join("src").join("main.rs"),
            "fn main() { std::hint::black_box(0u8); }\n",
        )
        .expect("main.rs");
        if net_reach {
            // A build-time network reach the build GENUINELY needs: it propagates
            // the connect error, so a jail withholding `network` (net namespace
            // unshared) makes the connect fail → build.rs errors → cargo exits
            // non-zero → the full run decodes BuildFailed (a real OS-boundary
            // reject). A `let _ =`-ignored connect would be a no-op (a caught denial
            // is no effect), which correctly reads as clean — so the negative must
            // propagate to prove the build truly demanded the withheld axis.
            std::fs::write(
                dir.join("build.rs"),
                "use std::net::TcpStream;\nuse std::time::Duration;\n\
                 fn main() {\n    \
                 let addr: std::net::SocketAddr = \"140.82.112.3:443\".parse().unwrap();\n    \
                 TcpStream::connect_timeout(&addr, Duration::from_secs(5))\n        \
                 .expect(\"tier2 negative fixture: build genuinely needs network\");\n}\n",
            )
            .expect("build.rs");
        }
    }

    /// The `cargo build` argv for the min crate, `--offline` with a scratch-local
    /// target dir (no registry deps, so offline needs no vendoring). Cargo is
    /// invoked by ABSOLUTE path: the in-jail PATH is a fixed `/usr/bin:/bin`, so a
    /// bare `cargo` is unfindable; the toolchain bind makes the absolute path
    /// executable.
    fn min_build_argv(crate_dir: &std::path::Path) -> Vec<OsString> {
        let cargo = which_cargo().map_or_else(|| OsString::from("cargo"), PathBuf::into_os_string);
        vec![
            cargo,
            OsString::from("build"),
            OsString::from("--offline"),
            OsString::from("--bin"),
            OsString::from("tier2_probe"),
            OsString::from("--manifest-path"),
            crate_dir.join("Cargo.toml").into_os_string(),
            OsString::from("--target-dir"),
            crate_dir.join("target").into_os_string(),
        ]
    }

    fn real_build_runner<'a>(
        harness: &Harness,
        tools: &'a RunJailTools,
        exercise: ProbeExercise,
    ) -> JailProbeRunner<'a> {
        // The real build needs the toolchain reachable inside the jail (read-only).
        let mut ro_binds = default_ro_binds();
        ro_binds.extend(ipe::audit_native::toolchain_ro_binds());
        JailProbeRunner::new(
            tools,
            harness.wrapper.clone(),
            harness.scoped_tmp.clone(),
            harness.working_tree.clone(),
            ro_binds,
            // Unused on the real-build path (full run is child-exit-only; tighten
            // probes the single declared axis under test).
            vec![TightenableAxis::Network, TightenableAxis::Filesystem],
            exercise,
        )
    }

    #[test]
    fn a_confined_clean_build_genuinely_certifies_the_first_real_certification() {
        // POSITIVE: a native crate whose build reaches NO withheld axis, built
        // under a jail scoped to `[network]` (which it declares but its build does
        // not need). The declared-scoped run is clean; the tighten run removes
        // network, still clean, but the static scan reaches network → not flagged
        // unused → reconcile Ok. This is the shape production turns into the single
        // `Certified` — proven here through the REAL jail with a REAL cargo build.
        let Some(tools) = e2e_tools() else { return };
        // `cargo` must be reachable inside the jail; skip cleanly if absent.
        if which_cargo().is_none() {
            eprintln!("audit_native e2e: skipping — cargo not found on PATH");
            return;
        }
        let declared = set(&[Capability::Network]);
        let scoped = scoped_profile(&declared).expect("lower profile");
        let harness = Harness::new("certify-clean");
        // The crate + its target live in the ALWAYS-writable scratch (`scoped_tmp`),
        // not the filesystem-axis-gated working tree — so a filesystem-withholding
        // jail (declared=[network]) can still build it. A withheld axis is withheld
        // by capability REMOVAL (net namespace), not by making the build unwritable.
        let crate_dir = harness.scoped_tmp.join("tier2min");
        write_min_crate(&crate_dir, false);
        let _guard = JAIL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let exercise =
            ProbeExercise::real_build(min_build_argv(&crate_dir)).expect("non-empty argv");
        assert!(
            exercise.is_real_build(),
            "the certify guard needs a real build"
        );
        let runner = real_build_runner(&harness, &tools, exercise);
        let scan = FixedScan {
            reaches: set(&[Capability::Network]),
        };
        let verdict = reconcile_native(&declared, &runner, &scan, &scoped);
        // Skip (not fail) if this environment cannot compile the crate under the
        // jail (no toolchain reachable inside bwrap): a BuildFailed here is an
        // environment gap, never a false pass. A real clean IS the certification.
        match verdict {
            Ok(()) => { /* the first genuine certification */ }
            Err(r) if r.message.contains("failed to build") => {
                eprintln!("audit_native e2e: skipping — cargo unavailable inside jail: {r}");
            }
            Err(r) => panic!("a confined clean build must certify, got: {r}"),
        }
    }

    #[test]
    fn a_build_that_reaches_network_under_a_withholding_jail_rejects_at_the_os_boundary() {
        // NEGATIVE (OS-boundary): a native crate whose `build.rs` opens a socket at
        // build time, built under a `[]`-scoped jail (network withheld). The
        // build's socket is denied by the jail; the wrapper's post-build network
        // probe under the same withheld jail is ALSO denied → Denied{network} →
        // REJECT naming the axis. A real OS denial, not a scripted-runner verdict.
        let Some(tools) = e2e_tools() else { return };
        if which_cargo().is_none() {
            eprintln!("audit_native e2e: skipping — cargo not found on PATH");
            return;
        }
        let declared = BTreeSet::new();
        let scoped = scoped_profile(&declared).expect("lower profile");
        let harness = Harness::new("reject-netbuild");
        // Build in the always-writable scratch (see the positive test).
        let crate_dir = harness.scoped_tmp.join("tier2min");
        write_min_crate(&crate_dir, true);
        let _guard = JAIL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let exercise =
            ProbeExercise::real_build(min_build_argv(&crate_dir)).expect("non-empty argv");
        let runner = real_build_runner(&harness, &tools, exercise);
        let scan = FixedScan {
            reaches: BTreeSet::new(),
        };
        let r = reconcile_native(&declared, &runner, &scan, &scoped)
            .expect_err("a build reaching network under a []-scoped jail must reject");
        assert_eq!(r.check, Check::NativeTier2);
        // On the full declared-scoped run the wrapper runs NO fabricated fixed axis
        // probe; the signal is the child build's own OS-boundary failure (the
        // net-namespace-unshared build.rs connect fails → cargo non-zero → decoded
        // BuildFailed). The reject is real and fail-closed; axis NAMING on the full
        // run is deliberately traded for not fabricating a demand the package never
        // made (the single-axis canaries above name the axis). If cargo is
        // unreachable inside the jail the build also fails closed — never a false
        // clean, never a certify.
        assert!(
            r.message.contains("failed to build") || r.message.contains("network"),
            "the reject is a real OS-boundary failure (BuildFailed or a named axis): {}",
            r.message
        );
        assert!(
            !r.message.contains("passed"),
            "a build reaching a withheld axis must never certify: {}",
            r.message
        );
    }

    fn which_cargo() -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|p| p.join("cargo"))
            .find(|p| p.is_file())
    }

    // Keep the imports honest under all cfgs.
    #[allow(dead_code)]
    fn _uses(_: &dyn ProbeRunner) {}
}
