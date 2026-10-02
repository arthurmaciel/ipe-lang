//! The one owner of every `cargo build` child the CLI spawns.
//!
//! A cargo build is described as a typed value — the crate it builds, its
//! profile, its target and its output mode — and this module alone turns that
//! value into a `Command`: the `build` subcommand, the flags, the environment,
//! the hermetic lockfile, the pipe drains and the reap. No other module builds
//! a cargo `build` command (`tests/cargo_step_scan.rs` holds the inventory), so
//! every command path gets the same environment, the same drains and the same
//! lifetime for its cargo child.
//!
//! Both pipes of a child are drained on threads scoped to the call that spawned
//! it, so no drain outlives the build and the child is always reaped before the
//! call returns, on success and on every error.

use std::io::{BufReader, Read as _};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};

use ipe_backend_rust::static_build::StaticTriple;

use crate::output_dir::OwnedDir;
use crate::style::TerminalSafe;
use crate::watch::{BuildAccel, apply_build_accel_env};
use crate::{CliError, RuntimeContext};

/// The cargo build profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoProfile {
    /// cargo's default `dev` profile (a debug binary).
    Dev,
    /// `--release`.
    Release,
}

/// The target a cargo build compiles for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoTarget {
    /// The host triple: no `--target` flag.
    Host,
    /// A statically linked target triple (`--target <triple>`).
    Static(StaticTriple),
    /// The browser bundle, `wasm32-unknown-unknown`.
    WasmBrowser,
    /// A WASI module, `wasm32-wasip1`.
    ///
    /// The emitted crate ships its own `.cargo/config.toml` linker override for
    /// this target, which an ambient `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`
    /// would outrank, so both are cleared from the child.
    Wasip1,
}

impl CargoTarget {
    /// The `--target` triple, or `None` for the host.
    const fn triple(self) -> Option<&'static str> {
        match self {
            Self::Host => None,
            Self::Static(triple) => Some(triple.as_str()),
            Self::WasmBrowser => Some("wasm32-unknown-unknown"),
            Self::Wasip1 => Some("wasm32-wasip1"),
        }
    }
}

/// How much of cargo's own progress a build shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// cargo's terminal UI (colour and the progress bar when stderr is a
    /// terminal), plus the dependency-resolve stage.
    Progress,
    /// cargo's `-q`; no dependency-resolve stage.
    Quiet,
}

impl Verbosity {
    /// [`Self::Quiet`] when a command's `--quiet` is set, else [`Self::Progress`].
    #[must_use]
    pub const fn of_quiet(quiet: bool) -> Self {
        if quiet { Self::Quiet } else { Self::Progress }
    }
}

/// Where a build's stdout goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoOutput {
    /// stdout inherited; a `cargo build` writes only status, to stderr.
    Human(Verbosity),
    /// `--message-format=json`: stdout is captured and handed back, the one
    /// authoritative record of the artifacts cargo wrote.
    JsonStream(Verbosity),
}

impl CargoOutput {
    /// The verbosity of either mode.
    const fn verbosity(self) -> Verbosity {
        match self {
            Self::Human(verbosity) | Self::JsonStream(verbosity) => verbosity,
        }
    }
}

/// The app binary and profile a release wrapper embeds.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedApp<'a> {
    /// The built app binary.
    pub binary: &'a Path,
    /// The app's `ipe.profile`.
    pub profile: &'a Path,
}

/// The crate a blocking cargo build compiles.
#[derive(Debug, Clone, Copy)]
pub enum CargoCrate<'a> {
    /// A crate ipe emitted into a claimed directory, proven that directory
    /// before cargo starts and again after it succeeds: cargo reaches it by
    /// path, so a swap while it runs is detected, never trusted.
    Emitted(&'a OwnedDir),
    /// The `ipe_wrapper` package of the compiler workspace at
    /// `workspace_root`, a tree ipe does not own, so no claim is proven.
    ReleaseWrapper {
        /// The workspace root holding the `ipe_wrapper` package.
        workspace_root: &'a Path,
        /// The app the wrapper embeds, when the release is a single file.
        embed: Option<EmbeddedApp<'a>>,
    },
}

/// One blocking `cargo build`: [`CargoBuild::run`] spawns it and returns once
/// cargo has exited and both pipes are drained.
#[derive(Debug, Clone)]
pub struct CargoBuild<'a> {
    /// The resolved `cargo` ([`crate::toolchain::CargoBin::path`]).
    pub cargo: &'a Path,
    /// The crate to build; cargo runs with it as its working directory.
    pub krate: CargoCrate<'a>,
    /// The build profile.
    pub profile: CargoProfile,
    /// The compile target.
    pub target: CargoTarget,
    /// Where stdout goes, and how much progress shows.
    pub output: CargoOutput,
    /// What is built, named in the failure diagnostic.
    pub what: &'static str,
    /// The runtime crate the build links against, when resolved.
    pub runtime: Option<RuntimeContext>,
}

impl CargoBuild<'_> {
    /// The directory cargo runs in.
    fn dir(&self) -> &Path {
        match self.krate {
            CargoCrate::Emitted(dir) => dir.path(),
            CargoCrate::ReleaseWrapper { workspace_root, .. } => workspace_root,
        }
    }

    /// The `cargo build` command, before the lockfile flag and the pipes.
    fn command(&self) -> Command {
        let mut cmd = build_command(
            self.cargo,
            self.dir(),
            self.profile,
            self.target,
            self.output,
        );
        if let CargoCrate::ReleaseWrapper { embed, .. } = self.krate {
            cmd.args(["--package", "ipe_wrapper"]);
            if let Some(app) = embed {
                // The wrapper's `build.rs` copies these into `OUT_DIR` and
                // enables its `embed_mode` cfg.
                cmd.env("IPE_EMBED_APP", app.binary)
                    .env("IPE_EMBED_PROFILE", app.profile);
            }
        }
        cmd
    }

    /// Pin the dependency graph, build against exactly that lock, and return
    /// the captured stdout (empty for [`CargoOutput::Human`]).
    ///
    /// The resolve runs once into the crate's own `Cargo.lock` and the build
    /// passes `--locked`, so a transitive point release cannot change a build
    /// with no source change, and any lock-to-manifest drift fails at `ipe`
    /// time. stderr is relayed live, indented one shared column, and kept
    /// unindented for the failure diagnostic.
    ///
    /// # Errors
    /// - [`CliError::Io`] if cargo cannot be spawned, waited on, or read.
    /// - [`CliError::EmittedBuildFailed`] if the resolve or the build exits
    ///   non-zero.
    /// - [`CliError::OutputRefused`] if an [`CargoCrate::Emitted`] directory
    ///   was replaced before or while cargo ran.
    pub fn run(&self) -> Result<String, CliError> {
        if let CargoCrate::Emitted(dir) = self.krate {
            dir.verify()?;
        }
        let dir = self.dir();
        let io_err = |source: std::io::Error| CliError::Io {
            path: dir.to_path_buf(),
            source,
        };
        let mut cmd = self.command();
        lock_dependencies(&cmd, dir, self.output.verbosity())?;
        cmd.arg("--locked");
        let mut child = cmd.spawn().map_err(io_err)?;
        let pipes = CargoPipes::take(&mut child);
        let drained = pipes.drain_while(|| child.wait());
        let status = drained.waited.map_err(io_err)?;
        if let Some(e) = drained.stderr.error {
            return Err(io_err(e));
        }
        if let Some(e) = drained.stdout.error {
            return Err(io_err(e));
        }
        if !status.success() {
            return Err(CliError::EmittedBuildFailed {
                what: self.what,
                code: status.code().unwrap_or(1),
                stderr: TerminalSafe::sanitize(&drained.stderr.text),
                runtime: self.runtime.clone(),
            });
        }
        if let CargoCrate::Emitted(dir) = self.krate {
            dir.verify()?;
        }
        Ok(drained.stdout.text)
    }
}

/// One `ipe watch` rebuild: [`WatchBuild::spawn`] starts it and returns at
/// once, so the watch loop can kill a superseded build.
///
/// A watch rebuild resolves dependencies live (no `--locked` replay), so an
/// edit loop does not hit the registry index on every rebuild.
pub struct WatchBuild<'a> {
    /// The resolved `cargo` ([`crate::toolchain::CargoBin::path`]).
    pub cargo: &'a Path,
    /// The emitted crate; cargo runs with it as its working directory.
    pub crate_dir: &'a Path,
    /// A target directory overriding the inherited `CARGO_TARGET_DIR`.
    pub target_dir: Option<&'a Path>,
    /// The compiler acceleration for this rebuild.
    pub accel: &'a BuildAccel,
    /// How much of cargo's progress shows.
    pub verbosity: Verbosity,
}

impl WatchBuild<'_> {
    /// The rebuild's `cargo build --message-format=json` command.
    fn command(&self) -> Command {
        let mut cmd = build_command(
            self.cargo,
            self.crate_dir,
            CargoProfile::Dev,
            CargoTarget::Host,
            CargoOutput::JsonStream(self.verbosity),
        );
        if let Some(target) = self.target_dir {
            cmd.env("CARGO_TARGET_DIR", target);
        }
        apply_build_accel_env(&mut cmd, self.accel);
        cmd
    }

    /// Spawn the rebuild, its pipes taken for [`CargoPipes::drain_while`].
    ///
    /// # Errors
    /// The spawn error when cargo cannot be started.
    pub fn spawn(&self) -> std::io::Result<(Child, CargoPipes)> {
        let mut child = self.command().spawn()?;
        let pipes = CargoPipes::take(&mut child);
        Ok((child, pipes))
    }
}

/// The `cargo build` every build kind shares: program, subcommand, working
/// directory, profile, target, output mode and terminal UI.
fn build_command(
    cargo: &Path,
    dir: &Path,
    profile: CargoProfile,
    target: CargoTarget,
    output: CargoOutput,
) -> Command {
    let mut cmd = Command::new(cargo);
    cmd.arg("build").current_dir(dir);
    if profile == CargoProfile::Release {
        cmd.arg("--release");
    }
    if let Some(triple) = target.triple() {
        cmd.args(["--target", triple]);
    }
    if target == CargoTarget::Wasip1 {
        cmd.env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS");
    }
    match output {
        CargoOutput::Human(_) => {
            cmd.stdout(Stdio::inherit());
        }
        CargoOutput::JsonStream(_) => {
            cmd.arg("--message-format=json").stdout(Stdio::piped());
        }
    }
    cmd.stderr(Stdio::piped());
    match output.verbosity() {
        Verbosity::Quiet => {
            cmd.arg("-q");
        }
        Verbosity::Progress => crate::force_cargo_terminal_ui(&mut cmd),
    }
    cmd
}

/// Resolve the crate's dependency graph once into its own `Cargo.lock`, with
/// the build command's program, directory and environment, so the lock comes
/// from the toolchain that consumes it.
///
/// The resolve is the one silent gap before cargo's own progress starts, so a
/// [`Verbosity::Progress`] build covers it with a stage, settled before the
/// relay starts.
///
/// # Errors
/// [`CliError::Io`] if the resolve cannot be spawned; [`CliError::EmittedBuildFailed`]
/// if it exits non-zero (the registry unreachable, for one).
fn lock_dependencies(build: &Command, dir: &Path, verbosity: Verbosity) -> Result<(), CliError> {
    let stage = (verbosity == Verbosity::Progress).then(|| {
        crate::progress::Stage::start(
            std::io::stderr(),
            "resolving the emitted crate's dependencies…",
        )
    });
    let mut lock = Command::new(build.get_program());
    lock.arg("generate-lockfile").current_dir(dir);
    for (key, val) in build.get_envs() {
        match val {
            Some(v) => lock.env(key, v),
            None => lock.env_remove(key),
        };
    }
    let resolved = match lock.output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(CliError::EmittedBuildFailed {
            what: "the emitted crate's dependency lockfile",
            code: output.status.code().unwrap_or(1),
            stderr: TerminalSafe::sanitize(&String::from_utf8_lossy(&output.stderr)),
            runtime: None,
        }),
        Err(source) => Err(CliError::Io {
            path: dir.to_path_buf(),
            source,
        }),
    };
    if let Some(stage) = stage {
        if resolved.is_ok() {
            stage.success("dependencies resolved");
        } else {
            stage.failure("dependency resolution failed");
        }
    }
    resolved
}

/// The stdout and stderr pipes of a spawned cargo child.
#[derive(Debug)]
pub struct CargoPipes {
    /// stdout, piped only for [`CargoOutput::JsonStream`].
    stdout: Option<ChildStdout>,
    /// stderr, always piped.
    stderr: Option<ChildStderr>,
}

/// The text a pipe drain read, and the read error that ended it early.
#[derive(Debug, Default)]
pub struct Drain {
    /// Everything read before the end of the stream or the error.
    pub text: String,
    /// The read error that stopped the drain, if one did.
    pub error: Option<std::io::Error>,
}

/// What [`CargoPipes::drain_while`] returns: the waiter's value and both drains.
#[derive(Debug)]
pub struct Drained<T> {
    /// What the waiter returned.
    pub waited: T,
    /// The captured stdout (empty when stdout was inherited).
    pub stdout: Drain,
    /// The captured stderr, unindented.
    pub stderr: Drain,
}

impl CargoPipes {
    /// Take both pipes off `child`.
    const fn take(child: &mut Child) -> Self {
        Self {
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
        }
    }

    /// Drain both pipes on threads scoped to this call while `wait` runs on
    /// the calling thread, and return once all three are done.
    ///
    /// Draining both pipes at once keeps a full, unread pipe buffer from
    /// stalling cargo. The drains end at end-of-stream, which cargo's exit
    /// (or its kill) brings, so none outlives the build.
    pub fn drain_while<T>(self, wait: impl FnOnce() -> T) -> Drained<T> {
        let Self { stdout, stderr } = self;
        std::thread::scope(|scope| {
            let stdout = scope.spawn(move || stdout.map(read_to_end).unwrap_or_default());
            let stderr = scope.spawn(move || stderr.map(relay_stderr).unwrap_or_default());
            let waited = wait();
            Drained {
                waited,
                stdout: joined(stdout.join(), "stdout"),
                stderr: joined(stderr.join(), "stderr"),
            }
        })
    }
}

/// A drain thread's result, a panic in it reported as a read error.
fn joined(result: std::thread::Result<Drain>, pipe: &str) -> Drain {
    result.unwrap_or_else(|_| Drain {
        text: String::new(),
        error: Some(std::io::Error::other(format!(
            "cargo {pipe} drain panicked"
        ))),
    })
}

/// Read `pipe` to its end.
fn read_to_end(mut pipe: ChildStdout) -> Drain {
    let mut drain = Drain::default();
    if let Err(e) = pipe.read_to_string(&mut drain.text) {
        drain.error = Some(e);
    }
    drain
}

/// Relay `pipe` live to our stderr, one shared column off the edge, and keep
/// the unindented text.
///
/// Chunks end at a newline or a carriage return
/// ([`crate::read_progress_chunk`]), so cargo's in-place progress bar flows
/// without waiting for the next newline.
fn relay_stderr(pipe: ChildStderr) -> Drain {
    let mut reader = BufReader::new(pipe);
    let mut drain = Drain::default();
    let mut chunk = String::new();
    loop {
        chunk.clear();
        match crate::read_progress_chunk(&mut reader, &mut chunk) {
            Ok(0) => break,
            Ok(_) => {
                crate::screen::emit_machine(
                    crate::screen::Stream::Stderr,
                    &crate::screen::indent_relay_chunk(&chunk),
                );
                drain.text.push_str(&chunk);
            }
            Err(e) => {
                drain.error = Some(e);
                break;
            }
        }
    }
    drain
}

#[cfg(test)]
mod tests {
    use super::{
        CargoBuild, CargoCrate, CargoOutput, CargoProfile, CargoTarget, EmbeddedApp, Verbosity,
        WatchBuild,
    };
    use crate::watch::BuildAccel;
    use ipe_backend_rust::static_build::StaticTriple;
    use std::ffi::OsStr;
    use std::path::Path;
    use std::process::Command;

    /// The arguments of `cmd`, as text.
    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// What a `Command` does with one environment variable.
    #[derive(Debug, PartialEq, Eq)]
    enum Env<'c> {
        /// Inherited untouched.
        Inherited,
        /// Removed from the child.
        Removed,
        /// Set to a value.
        Set(&'c OsStr),
    }

    /// What `cmd` does with `key`.
    fn env<'c>(cmd: &'c Command, key: &str) -> Env<'c> {
        cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .map_or(Env::Inherited, |(_, v)| v.map_or(Env::Removed, Env::Set))
    }

    /// A release wrapper build over `root`.
    fn wrapper<'a>(root: &'a Path, embed: Option<EmbeddedApp<'a>>) -> CargoBuild<'a> {
        CargoBuild {
            cargo: Path::new("cargo"),
            krate: CargoCrate::ReleaseWrapper {
                workspace_root: root,
                embed,
            },
            profile: CargoProfile::Release,
            target: CargoTarget::Static(StaticTriple::X8664LinuxMusl),
            output: CargoOutput::Human(Verbosity::Quiet),
            what: "the release wrapper",
            runtime: None,
        }
    }

    #[test]
    fn every_build_runs_the_build_subcommand_in_its_crate() {
        let root = Path::new("/ws");
        let cmd = wrapper(root, None).command();
        assert_eq!(args(&cmd).first().map(String::as_str), Some("build"));
        assert_eq!(cmd.get_current_dir(), Some(root));
        let accel = BuildAccel::MachineDefault;
        let watch = WatchBuild {
            cargo: Path::new("cargo"),
            crate_dir: Path::new("/crate"),
            target_dir: None,
            accel: &accel,
            verbosity: Verbosity::Progress,
        }
        .command();
        assert_eq!(args(&watch).first().map(String::as_str), Some("build"));
        assert_eq!(watch.get_current_dir(), Some(Path::new("/crate")));
    }

    #[test]
    fn the_wrapper_build_names_its_package_profile_target_and_quiet_flag() {
        let a = args(&wrapper(Path::new("/ws"), None).command());
        for want in [
            "--release",
            "--package",
            "ipe_wrapper",
            "--target",
            "x86_64-unknown-linux-musl",
            "-q",
        ] {
            assert!(a.iter().any(|x| x == want), "{want} missing from {a:?}");
        }
        assert!(!a.iter().any(|x| x == "--message-format=json"), "{a:?}");
    }

    #[test]
    fn only_an_embedding_wrapper_carries_the_embed_env() {
        let plain = wrapper(Path::new("/ws"), None).command();
        assert_eq!(env(&plain, "IPE_EMBED_APP"), Env::Inherited);
        let app = EmbeddedApp {
            binary: Path::new("/app"),
            profile: Path::new("/app.profile"),
        };
        let embedding = wrapper(Path::new("/ws"), Some(app)).command();
        assert_eq!(
            env(&embedding, "IPE_EMBED_APP"),
            Env::Set(OsStr::new("/app"))
        );
        assert_eq!(
            env(&embedding, "IPE_EMBED_PROFILE"),
            Env::Set(OsStr::new("/app.profile"))
        );
    }

    #[test]
    fn only_a_wasip1_build_clears_the_ambient_rustflags() {
        let wasi = super::build_command(
            Path::new("cargo"),
            Path::new("/c"),
            CargoProfile::Release,
            CargoTarget::Wasip1,
            CargoOutput::JsonStream(Verbosity::Quiet),
        );
        assert_eq!(env(&wasi, "RUSTFLAGS"), Env::Removed);
        assert_eq!(env(&wasi, "CARGO_ENCODED_RUSTFLAGS"), Env::Removed);
        assert!(args(&wasi).iter().any(|a| a == "wasm32-wasip1"));
        assert!(args(&wasi).iter().any(|a| a == "--message-format=json"));
        for target in [CargoTarget::Host, CargoTarget::WasmBrowser] {
            let other = super::build_command(
                Path::new("cargo"),
                Path::new("/c"),
                CargoProfile::Dev,
                target,
                CargoOutput::Human(Verbosity::Quiet),
            );
            assert_eq!(env(&other, "RUSTFLAGS"), Env::Inherited, "{target:?}");
        }
    }

    #[test]
    fn a_host_dev_build_carries_no_target_and_no_release_flag() {
        let cmd = super::build_command(
            Path::new("cargo"),
            Path::new("/c"),
            CargoProfile::Dev,
            CargoTarget::Host,
            CargoOutput::Human(Verbosity::Quiet),
        );
        assert_eq!(args(&cmd), ["build", "-q"]);
    }

    #[test]
    fn a_watch_build_pins_its_target_dir_and_acceleration() {
        let accel = BuildAccel::WarmIncremental;
        let cmd = WatchBuild {
            cargo: Path::new("cargo"),
            crate_dir: Path::new("/crate"),
            target_dir: Some(Path::new("/t")),
            accel: &accel,
            verbosity: Verbosity::Quiet,
        }
        .command();
        assert_eq!(env(&cmd, "CARGO_TARGET_DIR"), Env::Set(OsStr::new("/t")));
        assert_eq!(env(&cmd, "CARGO_INCREMENTAL"), Env::Set(OsStr::new("1")));
        assert_eq!(args(&cmd), ["build", "--message-format=json", "-q"]);
    }
}
