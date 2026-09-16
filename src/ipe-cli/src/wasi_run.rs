//! Execute an emitted `wasm32-wasip1` module in an EMBEDDED wasmtime engine —
//! the run-side of `ipe run --target wasi` (issue #2461).
//!
//! ## Why embedded, and why deny-by-default
//!
//! Ipê's whole execution model is a fail-closed, deny-by-default capability
//! jail. The native run confines the emitted binary with seccomp + a bind-mount
//! namespace derived from the program's declared capability floor
//! ([`crate::run_sandbox`] → [`ipe_sandbox::run_jail`]). The WASI run enforces
//! the SAME floor, expressed as a [`wasmtime_wasi::WasiCtxBuilder`] context
//! instead: preopen ONLY the declared filesystem, pass ONLY the declared env
//! allowlist, forward args, and deny the network unless the floor grants it.
//! There is NO ambient authority — a capability the floor does not grant has no
//! representation in the built [`wasmtime_wasi::preview1::WasiP1Ctx`], so the
//! guest cannot reach it. This is defend-in-depth: one capability model, two
//! independent enforcement surfaces.
//!
//! Embedding (rather than shelling out to an external `wasmtime` binary) is what
//! lets us BUILD that context ourselves. An external binary would receive
//! capabilities only through coarse CLI flags, may be absent, and may drift in
//! version/behaviour — forcing us to trust its sandbox rather than derive ours.
//!
//! ## The `wasi_run` feature
//!
//! wasmtime is a large dependency (a full wasm engine + the preview1 shim), so
//! it sits behind the `wasi_run` cargo feature — OFF for the lean dev/test
//! build, ON for release packaging. With the feature OFF, [`ensure_available`]
//! returns a typed [`CliError::WasiRunFeatureDisabled`] naming the feature —
//! never a panic, never a silent fall-through to a native run.

use ipe_sandbox::run_jail::{FilesystemScope, SandboxProfile};

/// The forwarded module's `argv[0]` — a conventional program name for the guest
/// (the wasip1 module carries no host path of its own).
#[cfg(feature = "wasi_run")]
const MODULE_ARGV0: &str = "ipe-app";

/// The filesystem grant a WASI preopen mirrors from the declared floor.
///
/// Read straight off the SAME [`SandboxProfile`] the native jail lowers, and
/// kept as a tiny typed value (parse, don't validate) so the feature-on builder
/// and the feature-off path agree on what a "grant" is, and a test can assert
/// the deny-by-default mapping without linking the engine.
///
/// - [`Self::ScopedTmpOnly`] mirrors [`FilesystemScope::Isolated`]: the guest
///   gets ONE writable scratch dir and nothing of the host tree.
/// - [`Self::WorkingTreeReadWrite`] mirrors
///   [`FilesystemScope::WorkingTreeReadWrite`]: the working tree is preopened
///   read-write, exactly the coarse grant the native jail binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsGrant {
    /// Only a scoped writable scratch dir is preopened (the maximally-isolated
    /// view). No host path is reachable.
    ScopedTmpOnly,
    /// The working tree is preopened read-write.
    WorkingTreeReadWrite,
}

impl FsGrant {
    /// Derive the filesystem grant from the profile's scope — the single mapping
    /// from the capability floor's FS axis to what a WASI preopen may expose.
    #[must_use]
    pub const fn from_profile(profile: &SandboxProfile) -> Self {
        match profile.filesystem {
            FilesystemScope::Isolated => Self::ScopedTmpOnly,
            FilesystemScope::WorkingTreeReadWrite => Self::WorkingTreeReadWrite,
        }
    }
}

/// Whether the WASI context may open sockets — the SINGLE network decision.
///
/// Fail-closed: the network is allowed ONLY when the declared floor grants it;
/// the maximally-isolated floor (and any floor without the `network` axis)
/// denies it. Exposed as a pure predicate so the deny-by-default mapping is
/// pinned by a test without linking the engine (mirroring the native jail's
/// socket-deny proofs).
#[must_use]
pub const fn network_allowed(profile: &SandboxProfile) -> bool {
    profile.network
}

#[cfg(feature = "wasi_run")]
mod engine {
    use super::{FsGrant, MODULE_ARGV0};
    use crate::{CliError, Path};
    use ipe_sandbox::run_jail::SandboxProfile;
    use wasmtime::{Engine, Linker, Module, Store};
    use wasmtime_wasi::p2::WasiCtxBuilder;
    use wasmtime_wasi::preview1::{self, WasiP1Ctx};
    use wasmtime_wasi::{DirPerms, FilePerms, I32Exit};

    /// The `wasi_run` feature is compiled in: the embedded engine is available.
    ///
    /// # Errors
    /// Never — the engine is linked, so this always returns `Ok`. The `Result`
    /// shape matches the feature-off twin so callers are feature-agnostic.
    pub const fn ensure_available() -> Result<(), CliError> {
        Ok(())
    }

    /// Build the deny-by-default [`WasiP1Ctx`] from the declared capability
    /// floor. Every axis is granted ONLY when the floor grants it; nothing is
    /// ambient.
    ///
    /// - **args** — forwarded (the program's own argv; not a host capability).
    /// - **stdio** — inherited (stdin/stdout/stderr), so the run behaves like the
    ///   native run for a Direct script's I/O.
    /// - **env** — ONLY the floor's `env_allowlist` names, each read from the
    ///   host and passed through; an un-allowlisted var is never visible.
    /// - **filesystem** — a single scoped writable scratch dir is ALWAYS
    ///   preopened as the guest's `.` (the isolated view's sole writable mount);
    ///   the working tree is additionally preopened read-write ONLY when the
    ///   floor grants [`crate::wasi_run::FsGrant::WorkingTreeReadWrite`]. No other
    ///   host path is reachable.
    /// - **network** — DENIED unless the floor grants it (`allow_tcp`/`allow_udp`
    ///   stay false and the network is not inherited); a socket attempt then
    ///   fails inside the guest with no host reachability.
    /// - **clock/random** — the preview1 defaults (no capability axis governs
    ///   them in the floor; they carry no host authority to gate).
    fn build_ctx(
        profile: &SandboxProfile,
        scratch: &Path,
        working_tree: &Path,
        args: &[String],
    ) -> Result<WasiP1Ctx, CliError> {
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdio();
        // argv[0] is a conventional program name; the forwarded args follow
        // (mirroring the native run's `cmd.args(&bin_args)`).
        builder.arg(MODULE_ARGV0);
        for a in args {
            builder.arg(a);
        }

        // env: ONLY the floor's allowlisted names, and only those actually
        // present in the host environment. An absent allowlisted var is simply
        // not passed (never an empty-string surprise), and a var outside the
        // allowlist is never visible — the same subset the native jail scrubs to.
        for name in &profile.env_allowlist {
            if let Some(value) = std::env::var_os(name) {
                builder.env(name, value.to_string_lossy());
            }
        }

        // filesystem: the scoped scratch is the guest's `.` (always writable, the
        // isolated view's sole mount). Only a working-tree-rw grant additionally
        // exposes the host tree, read-write, mirroring the native bind-mount.
        builder
            .preopened_dir(scratch, ".", DirPerms::all(), FilePerms::all())
            .map_err(|e| CliError::WasiRunFailed {
                detail: format!("could not preopen the scoped scratch dir: {e}"),
            })?;
        if matches!(
            FsGrant::from_profile(profile),
            FsGrant::WorkingTreeReadWrite
        ) {
            builder
                .preopened_dir(working_tree, "/work", DirPerms::all(), FilePerms::all())
                .map_err(|e| CliError::WasiRunFailed {
                    detail: format!("could not preopen the working tree: {e}"),
                })?;
        }

        // network: fail-closed. Only a network grant enables sockets; without it
        // the context inherits no network and refuses TCP/UDP.
        if super::network_allowed(profile) {
            builder.inherit_network();
            builder.allow_tcp(true);
            builder.allow_udp(true);
        } else {
            builder.allow_tcp(false);
            builder.allow_udp(false);
        }

        Ok(builder.build_p1())
    }

    /// Instantiate and run the emitted `wasm32-wasip1` module under embedded
    /// wasmtime.
    ///
    /// The guest is confined by the floor-derived [`WasiP1Ctx`]; its WASI exit
    /// code is propagated, and a trap maps to a typed [`CliError`], never a host
    /// panic.
    ///
    /// `module_file` is the emitted `wasm32-wasip1` artifact, resolved by the
    /// caller from `cargo metadata` (the authoritative target dir).
    ///
    /// # Errors
    /// - [`CliError::WasiRunFailed`] when the scratch dir, module load, linker
    ///   wiring, instantiation, or `_start` lookup fails, or the guest traps
    ///   without a clean WASI exit.
    /// - [`CliError::WasiRunExited`] when the guest runs to completion and
    ///   returns a non-zero WASI exit code.
    pub fn run_wasi_module(
        module_file: &Path,
        profile: &SandboxProfile,
        working_tree: &Path,
        args: &[String],
    ) -> Result<(), CliError> {
        // A scoped scratch dir is the guest's sole always-writable mount — the
        // WASI analogue of the native run's scoped tempdir. Dropped when this
        // returns, so nothing persists past the run.
        let scratch = crate::scratch::ScratchDir::new("ipe-wasi-run").map_err(|e| {
            CliError::WasiRunFailed {
                detail: format!("could not create the scoped scratch dir: {e}"),
            }
        })?;

        let engine = Engine::default();
        let module =
            Module::from_file(&engine, module_file).map_err(|e| CliError::WasiRunFailed {
                detail: format!(
                    "could not load the module at {}: {e}",
                    module_file.display()
                ),
            })?;

        let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
        preview1::add_to_linker_sync(&mut linker, |t| t).map_err(|e| CliError::WasiRunFailed {
            detail: format!("could not wire the WASI preview1 imports: {e}"),
        })?;

        let ctx = build_ctx(profile, scratch.path(), working_tree, args)?;
        let mut store = Store::new(&engine, ctx);

        let instance =
            linker
                .instantiate(&mut store, &module)
                .map_err(|e| CliError::WasiRunFailed {
                    detail: format!("could not instantiate the module: {e}"),
                })?;

        // A wasip1 command module exports `_start` with signature `() -> ()`.
        let start = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .map_err(|e| CliError::WasiRunFailed {
                detail: format!("the module has no wasip1 `_start` entry point: {e}"),
            })?;

        match start.call(&mut store, ()) {
            Ok(()) => Ok(()),
            Err(trap) => {
                // A clean WASI exit surfaces as an `I32Exit` trap: exit 0 is
                // success, non-zero is the guest's own outcome (propagated as
                // `ipe run`'s non-zero exit). Any other trap is a genuine run
                // failure — a typed error, never a host panic.
                if let Some(exit) = trap.downcast_ref::<I32Exit>() {
                    let code = exit.0;
                    return if code == 0 {
                        Ok(())
                    } else {
                        Err(CliError::WasiRunExited { code })
                    };
                }
                Err(CliError::WasiRunFailed {
                    detail: format!("the module trapped during execution: {trap}"),
                })
            }
        }
    }
}

#[cfg(not(feature = "wasi_run"))]
mod engine {
    use crate::{CliError, Path};
    use ipe_sandbox::run_jail::SandboxProfile;

    /// The `wasi_run` feature is NOT compiled in: no embedded engine is linked.
    ///
    /// Fail closed with a typed refusal naming the feature — never a panic, never
    /// a silent native fallback.
    ///
    /// # Errors
    /// Always returns [`CliError::WasiRunFeatureDisabled`] — there is no engine to
    /// make available.
    pub const fn ensure_available() -> Result<(), CliError> {
        Err(CliError::WasiRunFeatureDisabled)
    }

    /// Unreachable at runtime: [`ensure_available`] gates every caller before a
    /// module is built, so this returns the same typed refusal rather than
    /// running anything.
    ///
    /// # Errors
    /// Always returns [`CliError::WasiRunFeatureDisabled`] — the engine that would
    /// run the module is not linked into this build.
    pub const fn run_wasi_module(
        _module_file: &Path,
        _profile: &SandboxProfile,
        _working_tree: &Path,
        _args: &[String],
    ) -> Result<(), CliError> {
        Err(CliError::WasiRunFeatureDisabled)
    }
}

pub use engine::{ensure_available, run_wasi_module};

#[cfg(test)]
mod tests {
    use super::{FsGrant, network_allowed};
    use ipe_sandbox::run_jail::{FilesystemScope, SandboxProfile};

    #[test]
    fn isolated_scope_maps_to_scoped_tmp_only() {
        // Deny-by-default filesystem: the maximally-isolated floor exposes no
        // host path — only the scoped scratch is preopened.
        let p = SandboxProfile::maximally_isolated();
        assert_eq!(FsGrant::from_profile(&p), FsGrant::ScopedTmpOnly);
    }

    #[test]
    fn working_tree_rw_scope_maps_to_working_tree_grant() {
        // A working-tree grant — and ONLY that grant — exposes the host tree
        // read-write, mirroring the native jail's bind-mount.
        let p = SandboxProfile {
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            ..SandboxProfile::maximally_isolated()
        };
        assert_eq!(FsGrant::from_profile(&p), FsGrant::WorkingTreeReadWrite);
    }

    #[test]
    fn network_is_denied_by_default() {
        // The refusal proof (mirroring the native jail's socket-deny): the
        // maximally-isolated floor denies the network, so the WASI ctx never
        // opens a socket. Fail-closed — network is reachable ONLY on an explicit
        // grant.
        let denied = SandboxProfile::maximally_isolated();
        assert!(
            !network_allowed(&denied),
            "an undeclared network capability MUST be denied by the WASI context",
        );
        let granted = SandboxProfile {
            network: true,
            ..SandboxProfile::maximally_isolated()
        };
        assert!(
            network_allowed(&granted),
            "an explicitly-granted network capability is the ONLY way sockets open",
        );
    }
}
